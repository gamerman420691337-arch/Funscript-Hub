//! Separate bounded current-user bulk endpoint; no descriptor or lease is a capability.
use crate::{artifacts, Engine};
use anyhow::{bail, Result};
use pulsar_protocol::{local_socket_prelude::*, *};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
const IO_DEADLINE: Duration = Duration::from_secs(10);

pub(crate) fn start(engine: Arc<Engine>) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = engine;
        bail!("secured current-user bulk endpoint unavailable");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
        let endpoint = engine.config.bulk_endpoint();
        if let Ok(meta) = std::fs::symlink_metadata(&endpoint) {
            if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
                bail!("refusing to replace untrusted bulk endpoint");
            }
            std::fs::remove_file(&endpoint)?;
        }
        let listener = ListenerOptions::new()
            .name(endpoint_name(&endpoint)?)
            .create_sync()?;
        std::fs::set_permissions(&endpoint, std::fs::Permissions::from_mode(0o600))?;
        artifacts::sync_dir(&engine.config.state_dir)?;
        let active = Arc::new(AtomicUsize::new(0));
        std::thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut stream) = connection else {
                    break;
                };
                if active.fetch_add(1, Ordering::AcqRel) >= 16 {
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
                    let mut io = DeadlineStream::new(&mut stream, Instant::now() + IO_DEADLINE);
                    let handshake: BulkHandshake = match read_bulk_header(&mut io) {
                        Ok(value) => value,
                        Err(_) => return,
                    };
                    match engine.bulk_ready(&handshake) {
                        Ok(lease) => {
                            if write_bulk_header(&mut io, &BulkReply::Ready { lease }).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = write_bulk_header(&mut io, &BulkReply::Error(error));
                            return;
                        }
                    }
                    loop {
                        let mut io = DeadlineStream::new(&mut stream, Instant::now() + IO_DEADLINE);
                        let operation: BulkOperation = match read_bulk_header(&mut io) {
                            Ok(value) => value,
                            Err(_) => break,
                        };
                        if let Err(error) = operation.validate() {
                            let _ = write_bulk_header(&mut io, &BulkReply::Error(error));
                            break;
                        }
                        let result = match operation {
                            BulkOperation::Upload { offset, byte_len } => {
                                let bytes = match read_artifact_chunk(&mut io) {
                                    Ok(bytes) => bytes,
                                    Err(_) => break,
                                };
                                if bytes.len() != byte_len as usize {
                                    let _ = write_bulk_header(
                                        &mut io,
                                        &BulkReply::Error(ProtocolError::invalid(
                                            "bulk header and chunk length disagree",
                                        )),
                                    );
                                    break;
                                }
                                match engine.bulk_upload(&handshake, offset, &bytes) {
                                    Ok(accepted_prefix) => write_bulk_header(
                                        &mut io,
                                        &BulkReply::Uploaded { accepted_prefix },
                                    ),
                                    Err(error) => {
                                        let _ =
                                            write_bulk_header(&mut io, &BulkReply::Error(error));
                                        break;
                                    }
                                }
                            }
                            BulkOperation::Download { offset, byte_len } => {
                                match engine.bulk_download(&handshake, offset, byte_len) {
                                    Ok(bytes) => write_bulk_header(
                                        &mut io,
                                        &BulkReply::Download { offset, byte_len },
                                    )
                                    .and_then(|_| write_artifact_chunk(&mut io, &bytes)),
                                    Err(error) => {
                                        let _ =
                                            write_bulk_header(&mut io, &BulkReply::Error(error));
                                        break;
                                    }
                                }
                            }
                        };
                        if result.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Ok(())
    }
}
