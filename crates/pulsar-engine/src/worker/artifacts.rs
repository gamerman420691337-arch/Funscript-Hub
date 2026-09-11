//! Content-bound worker resources and append-only attempt artifacts.

use anyhow::{bail, Context, Result};
use pulsar_protocol::{ArtifactId, ArtifactIdentity, ArtifactReference, WorkerTools};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

pub fn pin_artifact(path: &Path) -> Result<ArtifactReference> {
    let path = path
        .canonicalize()
        .with_context(|| format!("resource unavailable: {}", path.display()))?;
    let (sha256, byte_len) = hash_file(&path, None)?;
    Ok(ArtifactReference {
        path,
        identity: ArtifactIdentity {
            artifact_id: ArtifactId::new(sha256.clone())?,
            sha256,
            byte_len,
        },
    })
}

pub fn verify_artifact(path: &Path, identity: &ArtifactIdentity, deadline: Instant) -> Result<()> {
    let (digest, byte_len) = hash_file(path, Some(deadline))?;
    if digest != identity.sha256 || byte_len != identity.byte_len {
        return Err(pulsar_protocol::ProtocolError::new(
            pulsar_protocol::ErrorCode::DependencyMismatch,
            "worker dependency bytes differ from the immutable attempt identity",
        )
        .into());
    }
    Ok(())
}

fn hash_file(path: &Path, deadline: Option<Instant>) -> Result<(String, u64)> {
    let mut file = File::open(path).context("cannot open declared worker resource")?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("worker resource must be a regular file");
    }
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    let mut count = 0u64;
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            bail!("worker dependency validation deadline exceeded");
        }
        let length = file.read(&mut bytes)?;
        if length == 0 {
            break;
        }
        count = count
            .checked_add(length as u64)
            .context("resource size overflow")?;
        hash.update(&bytes[..length]);
    }
    if count != metadata.len() || file.metadata()?.len() != count {
        bail!("worker resource changed while being read");
    }
    Ok((format!("{:x}", hash.finalize()), count))
}

/// Discovery only pins existing files. It never downloads or executes anything.
/// PATH fallback is a transitional development adapter, not a portable-package
/// qualification claim. Production bundles can provide explicit resource paths.
pub fn discover_tools() -> Result<WorkerTools> {
    Ok(WorkerTools {
        ffmpeg: pin_artifact(&discover("PULSAR_FFMPEG", "ffmpeg")?)?,
        ffprobe: pin_artifact(&discover("PULSAR_FFPROBE", "ffprobe")?)?,
        onnx_runtime: std::env::var_os("PULSAR_ORT_DYLIB")
            .map(|path| pin_artifact(Path::new(&path)))
            .transpose()?,
    })
}

fn discover(variable: &str, name: &str) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(variable) {
        return Ok(PathBuf::from(path));
    }
    let executable_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    if let Some(parent) = std::env::current_exe()?.parent() {
        let bundled = parent.join("runtimes").join(&executable_name);
        if bundled.is_file() {
            return Ok(bundled);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(&executable_name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    bail!("required bundled/development media tool {name} unavailable; no automatic installation is permitted")
}

struct ArtifactWriter {
    file: File,
    hash: Sha256,
    count: u64,
    limit: u64,
}

impl Write for ArtifactWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.limit.saturating_sub(self.count) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "attempt artifact budget exceeded",
            ));
        }
        let written = self.file.write(bytes)?;
        self.hash.update(&bytes[..written]);
        self.count += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn new_writer(directory: &Path, name: &str, limit: u64) -> Result<(PathBuf, ArtifactWriter)> {
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        bail!("artifact name must be a single host-owned path component");
    }
    let path = directory.join(name);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .context("attempt artifact exists or its approved directory is inaccessible")?;
    Ok((
        path,
        ArtifactWriter {
            file,
            hash: Sha256::new(),
            count: 0,
            limit,
        },
    ))
}

fn finish(path: PathBuf, writer: ArtifactWriter) -> Result<ArtifactReference> {
    writer.file.sync_all()?;
    let sha256 = format!("{:x}", writer.hash.finalize());
    Ok(ArtifactReference {
        path,
        identity: ArtifactIdentity {
            artifact_id: ArtifactId::new(sha256.clone())?,
            sha256,
            byte_len: writer.count,
        },
    })
}

pub fn write_json<T: Serialize>(
    directory: &Path,
    name: &str,
    value: &T,
    limit: u64,
) -> Result<ArtifactReference> {
    let (path, mut writer) = new_writer(directory, name, limit)?;
    serde_json::to_writer(&mut writer, value).map_err(|error| {
        if error.io_error_kind() == Some(std::io::ErrorKind::OutOfMemory) {
            anyhow::Error::from(pulsar_protocol::ProtocolError::new(
                pulsar_protocol::ErrorCode::ResourceExhausted,
                "attempt artifact budget exceeded",
            ))
        } else {
            anyhow::Error::from(error)
        }
    })?;
    finish(path, writer)
}

pub fn write_bytes(
    directory: &Path,
    name: &str,
    value: &[u8],
    limit: u64,
) -> Result<ArtifactReference> {
    if value.len() as u64 > limit {
        return Err(pulsar_protocol::ProtocolError::new(
            pulsar_protocol::ErrorCode::ResourceExhausted,
            "attempt artifact budget exceeded",
        )
        .into());
    }
    let (path, mut writer) = new_writer(directory, name, limit)?;
    writer.write_all(value)?;
    finish(path, writer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_writer_refuses_path_escape_before_open() {
        assert!(new_writer(Path::new("/not-used"), "../escape", 1024).is_err());
        assert!(new_writer(Path::new("/not-used"), "/absolute", 1024).is_err());
    }
}
