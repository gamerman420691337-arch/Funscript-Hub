//! Advisory read-only probe for a competing engine's existing instance lock.
use crate::EngineConfig;
use anyhow::{bail, Result};

/// Reports only current lock contention, not engine readiness or client authority.
/// The result can change immediately. Callers must retain their startup deadline
/// and still establish an authenticated engine connection.
pub fn instance_lock_is_held(config: &EngineConfig) -> Result<bool> {
    #[cfg(not(unix))]
    {
        let _ = config;
        bail!("secured instance-lock probing is unavailable on this build");
    }
    #[cfg(unix)]
    {
        use fs2::FileExt;
        use std::fs::{File, OpenOptions};
        use std::io::ErrorKind;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let directory = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&config.state_dir)
        {
            Ok(directory) => directory,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let metadata = directory.metadata()?;
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            bail!("engine state directory must be owned by current user with mode 0700");
        }
        // Anchor the child open to the checked directory, without creating files
        // or blocking on a malicious non-regular lock such as a FIFO.
        let raw = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                b"engine.lock\0".as_ptr().cast(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(error.into())
            };
        }
        let lock = unsafe { File::from_raw_fd(raw) };
        let metadata = lock.metadata()?;
        if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            bail!("engine instance lock must be a private regular file owned by current user");
        }
        match lock.try_lock_exclusive() {
            Ok(()) => {
                FileExt::unlock(&lock)?;
                Ok(false)
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(true),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use fs2::FileExt;
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt};

    fn fixture() -> (tempfile::TempDir, EngineConfig) {
        let dir = tempfile::tempdir().unwrap();
        let config = EngineConfig::new(dir.path().join("state"), dir.path().join("unused-worker"));
        (dir, config)
    }

    fn create_lock(config: &EngineConfig) -> File {
        crate::artifacts::private_directory(&config.state_dir).unwrap();
        let mut lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(config.state_dir.join("engine.lock"))
            .unwrap();
        lock.write_all(b"preserve existing lock bytes").unwrap();
        lock
    }

    #[test]
    fn absent_state_and_lock_are_not_created() {
        let (_dir, config) = fixture();
        assert!(!instance_lock_is_held(&config).unwrap());
        assert!(!config.state_dir.exists());
        crate::artifacts::private_directory(&config.state_dir).unwrap();
        assert!(!instance_lock_is_held(&config).unwrap());
        assert_eq!(fs::read_dir(&config.state_dir).unwrap().count(), 0);
    }

    #[test]
    fn held_and_unheld_probes_preserve_file_and_release_only_their_own_lock() {
        let (_dir, config) = fixture();
        let owner = create_lock(&config);
        let path = config.state_dir.join("engine.lock");
        let before = owner.metadata().unwrap();
        assert!(!instance_lock_is_held(&config).unwrap());
        owner.try_lock_exclusive().unwrap();
        assert!(instance_lock_is_held(&config).unwrap());
        assert!(instance_lock_is_held(&config).unwrap());
        FileExt::unlock(&owner).unwrap();
        assert!(!instance_lock_is_held(&config).unwrap());
        let after = fs::metadata(&path).unwrap();
        assert_eq!(
            (before.ino(), before.mode(), before.len()),
            (after.ino(), after.mode(), after.len())
        );
        assert_eq!(fs::read(&path).unwrap(), b"preserve existing lock bytes");
        owner.try_lock_exclusive().unwrap();
        FileExt::unlock(&owner).unwrap();
    }

    #[test]
    fn unsafe_state_symlink_type_and_permissions_are_errors_not_absence() {
        let (dir, config) = fixture();
        let target = dir.path().join("real-state");
        crate::artifacts::private_directory(&target).unwrap();
        symlink(&target, &config.state_dir).unwrap();
        assert!(instance_lock_is_held(&config).is_err());
        fs::remove_file(&config.state_dir).unwrap();
        fs::write(&config.state_dir, b"not a directory").unwrap();
        assert!(instance_lock_is_held(&config).is_err());
        fs::remove_file(&config.state_dir).unwrap();
        crate::artifacts::private_directory(&config.state_dir).unwrap();
        fs::set_permissions(&config.state_dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(instance_lock_is_held(&config).is_err());
    }

    #[test]
    fn unsafe_lock_symlink_type_and_permissions_are_rejected() {
        let (dir, config) = fixture();
        crate::artifacts::private_directory(&config.state_dir).unwrap();
        let path = config.state_dir.join("engine.lock");
        let target = dir.path().join("target");
        fs::write(&target, b"not followed").unwrap();
        symlink(&target, &path).unwrap();
        assert!(instance_lock_is_held(&config).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"not followed");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(instance_lock_is_held(&config).is_err());
        fs::remove_dir(&path).unwrap();
        let lock = create_lock(&config);
        lock.set_permissions(fs::Permissions::from_mode(0o644))
            .unwrap();
        assert!(instance_lock_is_held(&config).is_err());
    }

    #[test]
    fn nonregular_fifo_probe_never_waits_for_a_writer() {
        let (_dir, config) = fixture();
        crate::artifacts::private_directory(&config.state_dir).unwrap();
        use std::os::unix::ffi::OsStrExt;
        let path =
            std::ffi::CString::new(config.state_dir.join("engine.lock").as_os_str().as_bytes())
                .unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let start = std::time::Instant::now();
        assert!(instance_lock_is_held(&config).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
