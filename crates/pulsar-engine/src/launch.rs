//! The parent owns confinement, resource limits, deadlines and response fencing.
use crate::{artifacts, EngineConfig};
use pulsar_protocol::*;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub fn available() -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/usr/bin/bwrap").is_file() && crate::cgroup::delegated_root().is_ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}
fn failure(code: ErrorCode, error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(code, error.to_string())
}

pub fn execute(
    config: &EngineConfig,
    manifest: &WorkerRequest,
    cancel: &AtomicBool,
) -> Result<WorkerOutput, ProtocolError> {
    manifest.validate()?;
    if !available() {
        return Err(ProtocolError::unsupported(
            "native worker confinement on this platform",
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, cancel);
        return Err(ProtocolError::unsupported("worker confinement"));
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        let group = crate::cgroup::AttemptCgroup::create(
            manifest.attempt_id.as_str(),
            manifest.budget.memory_bytes,
            manifest.budget.cpu_threads,
        )
        .map_err(|e| failure(ErrorCode::Unavailable, e))?;
        let procs_fd = group.procs.as_raw_fd();
        let source = manifest
            .source
            .path
            .canonicalize()
            .map_err(|e| failure(ErrorCode::DependencyMismatch, e))?;
        if source != manifest.source.path {
            return Err(ProtocolError::invalid(
                "source manifest path must be canonical",
            ));
        }
        let mut command = Command::new("/usr/bin/bwrap");
        command.env_clear();
        command.args([
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
        ]);
        for path in ["/usr", "/lib", "/lib64", "/bin", "/etc/ld.so.cache"] {
            if Path::new(path).exists() {
                command.args(["--ro-bind", path, path]);
            }
        }
        command.args([
            "--proc", "/proc", "--dev", "/dev", "--size", "16777216", "--tmpfs", "/tmp",
        ]);
        command
            .arg("--ro-bind")
            .arg(&manifest.source.path)
            .arg(&manifest.source.path);
        let model = match &manifest.operation {
            WorkerOperation::Generate { model, .. } | WorkerOperation::Preview { model, .. } => model.as_ref(),
            WorkerOperation::GenerateInput { .. } => None,
        };
        if let Some(model) = model {
            command.arg("--ro-bind").arg(&model.path).arg(&model.path);
        }
        for tool in [&manifest.tools.ffmpeg, &manifest.tools.ffprobe]
            .into_iter()
            .chain(manifest.tools.onnx_runtime.iter())
        {
            if !tool.path.starts_with("/usr") && !tool.path.starts_with("/lib") {
                command.arg("--ro-bind").arg(&tool.path).arg(&tool.path);
            }
        }
        command
            .arg("--size")
            .arg(manifest.budget.output_bytes.to_string())
            .arg("--tmpfs")
            .arg(&manifest.output_dir)
            .arg("--ro-bind")
            .arg(&config.worker_executable)
            .arg("/pulsar")
            .arg("--chdir")
            .arg(&manifest.output_dir)
            .args([
                "--remount-ro",
                "/proc",
                "--remount-ro",
                "/dev",
                "--remount-ro",
                "/",
            ])
            .args([
                "--setenv",
                "PATH",
                "/usr/bin:/bin",
                "--setenv",
                "HOME",
                "/tmp",
                "--setenv",
                "OMP_NUM_THREADS",
                "2",
                "--setenv",
                "OPENBLAS_NUM_THREADS",
                "2",
                "--setenv",
                "PULSAR_WORKER_CONFINED",
                "1",
                "/pulsar",
                "worker",
            ]);
        let memory = manifest.budget.memory_bytes;
        let output = manifest.budget.output_bytes;
        let cpu_seconds = (manifest.budget.wall_time_ms / 1000).saturating_add(2);
        unsafe {
            command.pre_exec(move || {
                // Only async-signal-safe syscalls after fork. Writing zero moves
                // this child (and its future descendants), never the parent.
                if libc::write(procs_fd, b"0".as_ptr().cast(), 1) != 1 {
                    return Err(std::io::Error::last_os_error());
                }
                for (resource, value) in [
                    (libc::RLIMIT_AS, memory),
                    (libc::RLIMIT_FSIZE, output),
                    (libc::RLIMIT_CPU, cpu_seconds),
                    (libc::RLIMIT_NOFILE, 128),
                ] {
                    let limit = libc::rlimit {
                        rlim_cur: value as libc::rlim_t,
                        rlim_max: value as libc::rlim_t,
                    };
                    if libc::setrlimit(resource, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| failure(ErrorCode::Unavailable, e))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProtocolError::new(ErrorCode::Internal, "worker stdin unavailable"))?;
        if let Err(error) = write_message(&mut stdin, manifest) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(failure(ErrorCode::InvalidRequest, error));
        }
        drop(stdin);
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProtocolError::new(ErrorCode::Internal, "worker stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProtocolError::new(ErrorCode::Internal, "worker stderr unavailable"))?;
        let diagnostics = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.take(16384).read_to_end(&mut bytes);
            bytes
        });
        let expected = manifest.clone();
        let reader = std::thread::spawn(move || receive_artifacts(&mut stdout, &expected));
        let deadline = Instant::now() + Duration::from_millis(manifest.budget.wall_time_ms);
        let mut interruption = None;
        let status = loop {
            if cancel.load(Ordering::Acquire) {
                interruption = Some(ProtocolError::new(
                    ErrorCode::Cancelled,
                    "worker attempt cancelled",
                ));
                group.kill();
                let _ = child.kill();
                break child.wait();
            }
            if Instant::now() >= deadline {
                interruption = Some(ProtocolError::new(
                    ErrorCode::Unavailable,
                    "worker wall deadline exceeded",
                ));
                group.kill();
                let _ = child.kill();
                break child.wait();
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(error);
                }
            }
        }
        .map_err(|e| failure(ErrorCode::Unavailable, e))?;
        let response = reader.join().map_err(|_| {
            ProtocolError::new(ErrorCode::Internal, "worker response reader failed")
        })?;
        let diagnostics = diagnostics.join().unwrap_or_default();
        if let Some(error) = interruption {
            return Err(error);
        }
        if !status.success() {
            return Err(ProtocolError::new(
                ErrorCode::Unavailable,
                format!(
                    "confined worker exited {status}: {}",
                    String::from_utf8_lossy(&diagnostics)
                ),
            ));
        }
        let response = response?;
        if response.version != PROTOCOL_VERSION
            || response.job_id != manifest.job_id
            || response.attempt_id != manifest.attempt_id
        {
            return Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                "late or mismatched worker response identity",
            ));
        }
        // The guest has no writable host path. A size-limited tmpfs caps guest
        // output; only bounded, validated egress is published by the engine.
        let mut total = 0u64;
        for entry in std::fs::read_dir(&manifest.output_dir)
            .map_err(|e| failure(ErrorCode::Unavailable, e))?
        {
            let entry = entry.map_err(|e| failure(ErrorCode::Unavailable, e))?;
            let meta = entry
                .file_type()
                .map_err(|e| failure(ErrorCode::Unavailable, e))?;
            if !meta.is_file() {
                return Err(ProtocolError::new(
                    ErrorCode::Forbidden,
                    "worker published non-regular output",
                ));
            }
            total = total
                .checked_add(
                    entry
                        .metadata()
                        .map_err(|e| failure(ErrorCode::Unavailable, e))?
                        .len(),
                )
                .ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::ResourceExhausted,
                        "output byte accounting overflow",
                    )
                })?;
            if total > manifest.budget.output_bytes {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "aggregate worker output exceeded reservation",
                ));
            }
        }
        artifacts::sync_dir(&manifest.output_dir)
            .map_err(|e| failure(ErrorCode::Unavailable, e))?;
        response.result
    }
}

fn receive_artifacts(
    reader: &mut impl Read,
    manifest: &WorkerRequest,
) -> Result<WorkerResult, ProtocolError> {
    use sha2::{Digest, Sha256};
    let mut response: WorkerResult =
        read_message(reader).map_err(|e| failure(ErrorCode::InvalidRequest, e))?;
    if response.version != PROTOCOL_VERSION
        || response.job_id != manifest.job_id
        || response.attempt_id != manifest.attempt_id
    {
        return Err(ProtocolError::new(
            ErrorCode::DependencyMismatch,
            "worker response identity mismatch before artifact publication",
        ));
    }
    let mut references: Vec<(&mut ArtifactReference, &str)> = match &mut response.result {
        Ok(WorkerOutput::Generated {
            program, receipt, ..
        }) => vec![(program, "program.json"), (receipt, "receipt.json")],
        Ok(WorkerOutput::Preview(preview)) => preview
            .frame
            .as_mut()
            .map(|f| vec![(&mut f.artifact, "frame.raw")])
            .unwrap_or_default(),
        Err(_) => vec![],
    };
    let total = references
        .iter()
        .try_fold(0u64, |sum, (reference, _)| {
            sum.checked_add(reference.identity.byte_len)
        })
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::ResourceExhausted, "artifact total overflow")
        })?;
    if total > manifest.budget.output_bytes {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "worker declared aggregate output exceeds admitted budget",
        ));
    }
    for (reference, name) in &mut references {
        reference.identity.validate()?;
        let mut temporary = tempfile::NamedTempFile::new_in(&manifest.output_dir)
            .map_err(|e| failure(ErrorCode::Unavailable, e))?;
        let mut remaining = reference.identity.byte_len;
        let mut digest = Sha256::new();
        while remaining > 0 {
            let bytes =
                read_artifact_chunk(reader).map_err(|e| failure(ErrorCode::InvalidRequest, e))?;
            if bytes.len() as u64 > remaining {
                return Err(ProtocolError::invalid(
                    "worker chunk exceeds declared artifact length",
                ));
            }
            temporary
                .write_all(&bytes)
                .map_err(|e| failure(ErrorCode::Unavailable, e))?;
            digest.update(&bytes);
            remaining -= bytes.len() as u64;
        }
        if format!("{:x}", digest.finalize()) != reference.identity.sha256 {
            return Err(ProtocolError::new(
                ErrorCode::DependencyMismatch,
                "worker artifact egress digest mismatch",
            ));
        }
        temporary
            .as_file()
            .sync_all()
            .map_err(|e| failure(ErrorCode::Unavailable, e))?;
        let destination = manifest.output_dir.join(*name);
        let published = temporary
            .persist_noclobber(&destination)
            .map_err(|e| failure(ErrorCode::Unavailable, e.error))?;
        let mut permissions = published
            .metadata()
            .map_err(|e| failure(ErrorCode::Unavailable, e))?
            .permissions();
        permissions.set_readonly(true);
        published
            .set_permissions(permissions)
            .map_err(|e| failure(ErrorCode::Unavailable, e))?;
        reference.path = destination;
    }
    // Fixed egress sequence ends at EOF; extra undeclared bytes are malformed.
    let mut extra = [0u8; 1];
    if reader
        .read(&mut extra)
        .map_err(|e| failure(ErrorCode::InvalidRequest, e))?
        != 0
    {
        return Err(ProtocolError::invalid("unexpected trailing worker egress"));
    }
    artifacts::sync_dir(&manifest.output_dir).map_err(|e| failure(ErrorCode::Unavailable, e))?;
    Ok(response)
}
