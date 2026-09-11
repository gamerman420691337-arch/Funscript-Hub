//! Engine-owned durable artifacts. No client or worker can choose a project store path.
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub fn digest_file(path: &Path, limit: u64) -> Result<(String, u64)> {
    let mut file = open_regular(path)?;
    if file.metadata()?.len() > limit {
        bail!("artifact exceeds admitted byte limit");
    }
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .context("artifact size overflow")?;
        if total > limit {
            bail!("artifact grew beyond admitted byte limit");
        }
        hash.update(&buf[..n]);
    }
    Ok((format!("{:x}", hash.finalize()), total))
}

pub fn open_regular(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        bail!("expected a regular artifact file");
    }
    Ok(file)
}

pub fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Copies from an opened source, rejecting an observable concurrent modification.
/// The resulting content hash, not a mutable original path, is its identity.
pub fn snapshot(source: &Path, dir: &Path, max_bytes: u64) -> Result<(PathBuf, String, u64)> {
    snapshot_reserved(source, dir, max_bytes, 0)
}
pub fn snapshot_reserved(
    source: &Path,
    dir: &Path,
    max_bytes: u64,
    held_storage: u64,
) -> Result<(PathBuf, String, u64)> {
    let canonical = source.canonicalize()?;
    let mut input = open_regular(&canonical)?;
    let before = input.metadata()?;
    let size = before.len();
    if size > max_bytes {
        bail!("source exceeds configured snapshot limit");
    }
    let reserve = size
        .checked_add(16 * 1024 * 1024)
        .and_then(|size| size.checked_add(held_storage))
        .context("storage reservation overflow")?;
    if fs2::available_space(dir)? < reserve {
        bail!("insufficient storage for immutable source snapshot");
    }
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(not(target_os = "linux"))]
    let cloned = false;
    // FICLONE creates a copy-on-write extent snapshot; failure takes the copy path.
    #[cfg(target_os = "linux")]
    let cloned = {
        use std::os::fd::AsRawFd;
        (unsafe {
            libc::ioctl(
                temp.as_file().as_raw_fd(),
                0x40049409 as libc::c_ulong,
                input.as_raw_fd(),
            )
        }) == 0
    };
    if !cloned {
        let count = std::io::copy(
            &mut Read::by_ref(&mut input).take(size.saturating_add(1)),
            &mut temp,
        )?;
        if count != size {
            bail!("source changed while snapshotting");
        }
    }
    let after = input.metadata()?;
    if after.len() != size || before.modified().ok() != after.modified().ok() {
        bail!("source changed while snapshotting");
    }
    temp.as_file().sync_all()?;
    let (hash, actual_size) = digest_file(temp.path(), max_bytes)?;
    if actual_size != size {
        bail!("snapshot size differs from captured source");
    }
    let destination = dir.join(&hash);
    match temp.persist_noclobber(&destination) {
        Ok(file) => {
            let mut permissions = file.metadata()?.permissions();
            permissions.set_readonly(true);
            file.set_permissions(permissions)?;
            file.sync_all()?;
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let (existing, existing_size) = digest_file(&destination, max_bytes)?;
            if existing != hash || existing_size != size {
                bail!("existing snapshot failed content identity check");
            }
        }
        Err(error) => return Err(error.error.into()),
    }
    sync_dir(dir)?;
    Ok((destination, hash, size))
}

/// A standard export never overwrites an existing user file.
pub fn publish_export(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = parent.canonicalize()?;
    let name = path.file_name().context("export requires a filename")?;
    let destination = parent.join(name);
    if destination.exists() {
        bail!("export target already exists; choose a new path");
    }
    if fs2::available_space(&parent)? < bytes.len() as u64 + 1024 * 1024 {
        bail!("insufficient export storage");
    }
    let mut temp = tempfile::NamedTempFile::new_in(&parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(destination).map_err(|e| e.error)?;
    sync_dir(&parent)
}

pub fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)?;
        }
        #[cfg(not(unix))]
        fs::create_dir_all(path)?;
    }
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        bail!("engine state path must be a real private directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            bail!("engine state directory must be owned by current user with mode 0700");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immutable_copy_and_non_destructive_export() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("original");
        fs::write(&source, b"source").unwrap();
        let snapshots = dir.path().join("snapshots");
        fs::create_dir(&snapshots).unwrap();
        let (copy, hash, size) = snapshot(&source, &snapshots, 100).unwrap();
        fs::write(&source, b"changed").unwrap();
        assert_eq!(fs::read(&copy).unwrap(), b"source");
        assert_eq!(size, 6);
        assert_eq!(hash.len(), 64);
        let export = dir.path().join("out");
        publish_export(&export, b"first").unwrap();
        assert!(publish_export(&export, b"second").is_err());
        assert_eq!(fs::read(export).unwrap(), b"first");
    }
    #[test]
    fn refuses_snapshot_budget() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("source");
        fs::write(&p, b"123").unwrap();
        assert!(snapshot(&p, dir.path(), 2).is_err());
    }
}
