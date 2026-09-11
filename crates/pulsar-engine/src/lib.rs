//! Sole authority for durable project state and brokered effects.
//! Native media/inference execution is confined to the worker process role.
pub mod artifacts;
mod authority;
mod bulk;
mod cgroup;
mod instance;
mod launch;
mod motion_artifacts;
pub mod resources;
pub mod worker;
use anyhow::{bail, Context, Result};
pub use authority::Engine;
pub use instance::instance_lock_is_held;
use fs2::FileExt;
use pulsar_protocol::{local_socket_prelude::*, *};
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub state_dir: PathBuf,
    pub worker_executable: PathBuf,
    pub max_ram_bytes: u64,
    pub max_snapshot_bytes: u64,
    pub max_jobs: usize,
    pub worker_timeout: Duration,
}
impl EngineConfig {
    pub fn new(state_dir: PathBuf, worker_executable: PathBuf) -> Self {
        Self {
            state_dir,
            worker_executable,
            max_ram_bytes: 4 * 1024 * 1024 * 1024,
            max_snapshot_bytes: 32 * 1024 * 1024 * 1024,
            max_jobs: 2,
            worker_timeout: Duration::from_secs(60 * 60),
        }
    }
    pub fn for_user() -> Result<Self> {
        let state_dir = if let Some(path) = std::env::var_os("PULSAR_STATE_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
            PathBuf::from(path).join("pulsar")
        } else if let Some(path) = std::env::var_os("HOME") {
            PathBuf::from(path).join(".local/state/pulsar")
        } else if let Some(path) = std::env::var_os("LOCALAPPDATA") {
            PathBuf::from(path).join("Pulsar")
        } else {
            bail!("cannot determine per-user engine state directory");
        };
        Ok(Self::new(state_dir, std::env::current_exe()?))
    }
    pub fn endpoint(&self) -> PathBuf {
        self.state_dir.join("engine.sock")
    }
    pub fn bulk_endpoint(&self) -> PathBuf {
        self.state_dir.join("bulk.sock")
    }
    pub fn bootstrap_path(&self) -> PathBuf {
        self.state_dir.join("pairing.token")
    }
}

/// Local IPC does not defend against arbitrary software with the user's full
/// OS privileges. Third-party clients still need explicit pairing and grants.
pub fn run_server(config: EngineConfig) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = config;
        bail!("secured current-user named-pipe server is unavailable on this build; no insecure listener was started");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
        artifacts::private_directory(&config.state_dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(config.state_dir.join("engine.lock"))?;
        lock.try_lock_exclusive()
            .context("another per-user engine already owns this state directory")?;
        let endpoint = config.endpoint();
        if let Ok(meta) = fs::symlink_metadata(&endpoint) {
            if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
                bail!("refusing to replace an untrusted engine endpoint");
            }
            fs::remove_file(&endpoint)?;
        }
        let engine = Engine::open(config)?;
        bulk::start(engine.clone())?;
        let listener = ListenerOptions::new()
            .name(endpoint_name(&endpoint)?)
            .create_sync()?;
        fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600))?;
        artifacts::sync_dir(endpoint.parent().context("missing endpoint parent")?)?;
        let active = Arc::new(AtomicUsize::new(0));
        for connection in listener.incoming() {
            let mut stream = connection?;
            if active.fetch_add(1, Ordering::AcqRel) >= 32 {
                active.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            let engine = engine.clone();
            let active = active.clone();
            std::thread::spawn(move || {
                struct Slot(Arc<AtomicUsize>);
                impl Drop for Slot {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                let _slot = Slot(active);
                loop {
                    let mut reader =
                        DeadlineStream::new(&mut stream, Instant::now() + Duration::from_secs(30));
                    let frame = match read_request(&mut reader) {
                        Ok(frame) => frame,
                        Err(_) => break,
                    };
                    let response = match frame {
                        RequestFrame::Current(request) => engine.handle(request),
                        RequestFrame::Rejected { request_id, error } => {
                            Response::failure(request_id, error)
                        }
                    };
                    let mut writer =
                        DeadlineStream::new(&mut stream, Instant::now() + Duration::from_secs(30));
                    if write_message(&mut writer, &response).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(())
    }
}
