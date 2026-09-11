//! Per-attempt cgroup v2 limits. Never modify a parent or move unrelated tasks.
#[cfg(target_os = "linux")]
use std::{fs, io, path::PathBuf};

#[cfg(target_os = "linux")]
pub fn delegated_root() -> io::Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let explicit = std::env::var_os("PULSAR_CGROUP_ROOT").map(PathBuf::from);
    let candidates = if let Some(root) = explicit {
        vec![root]
    } else {
        let membership = fs::read_to_string("/proc/self/cgroup")?;
        let relative = membership
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "cgroup v2 membership unavailable",
                )
            })?;
        let current = PathBuf::from("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
        current
            .ancestors()
            .take_while(|p| p.starts_with("/sys/fs/cgroup"))
            .map(|p| p.to_path_buf())
            .collect()
    };
    for root in candidates {
        let meta = match fs::symlink_metadata(&root) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o200 == 0
        {
            continue;
        }
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(root.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid cgroup path"))?;
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        if unsafe { libc::statfs(path.as_ptr(), filesystem.as_mut_ptr()) } != 0
            || unsafe { filesystem.assume_init() }.f_type != 0x63677270
        {
            continue;
        }
        let controllers =
            fs::read_to_string(root.join("cgroup.subtree_control")).unwrap_or_default();
        if ["cpu", "memory", "pids"]
            .iter()
            .all(|required| controllers.split_whitespace().any(|c| c == *required))
        {
            return root.canonicalize();
        }
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "no user-delegated cgroup with cpu,memory,pids controllers; worker refused",
    ))
}

#[cfg(target_os = "linux")]
pub struct AttemptCgroup {
    path: PathBuf,
    pub procs: fs::File,
}
#[cfg(target_os = "linux")]
impl AttemptCgroup {
    pub fn create(attempt: &str, memory: u64, cpu_threads: u16) -> io::Result<Self> {
        let path = delegated_root()?.join(format!("pulsar-worker-{attempt}"));
        fs::create_dir(&path)?;
        let configured = (|| {
            fs::write(path.join("memory.max"), memory.to_string())?;
            fs::write(path.join("memory.swap.max"), "0")?;
            fs::write(path.join("pids.max"), "64")?;
            fs::write(
                path.join("cpu.max"),
                format!("{} 100000", u64::from(cpu_threads) * 100000),
            )?;
            let procs = fs::OpenOptions::new()
                .write(true)
                .open(path.join("cgroup.procs"))?;
            Ok(Self {
                path: path.clone(),
                procs,
            })
        })();
        if configured.is_err() {
            let _ = fs::remove_dir(&path);
        }
        configured
    }
    pub fn kill(&self) {
        let _ = fs::write(self.path.join("cgroup.kill"), "1");
    }
}
#[cfg(target_os = "linux")]
impl Drop for AttemptCgroup {
    fn drop(&mut self) {
        self.kill();
        for _ in 0..20 {
            match fs::remove_dir(&self.path) {
                Ok(()) => return,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
    }
}
