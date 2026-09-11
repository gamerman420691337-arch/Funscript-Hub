//! Admission is aggregate and explicit; inability to reserve is not a quality downgrade.
use pulsar_protocol::{ErrorCode, ProtocolError};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct ResourcePool {
    inner: Arc<Mutex<Usage>>,
    max_memory: u64,
    max_jobs: usize,
}
#[derive(Default)]
struct Usage {
    memory: u64,
    jobs: usize,
    storage: u64,
}
pub struct Reservation {
    pool: ResourcePool,
    memory: u64,
    storage: u64,
    worker: bool,
}
impl ResourcePool {
    pub fn new(max_memory: u64, max_jobs: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Usage::default())),
            max_memory,
            max_jobs,
        }
    }
    pub fn reserve(&self, memory: u64) -> Result<Reservation, ProtocolError> {
        self.reserve_kind(memory, true)
    }
    pub fn reserve_memory(&self, memory: u64) -> Result<Reservation, ProtocolError> {
        self.reserve_kind(memory, false)
    }
    fn reserve_kind(&self, memory: u64, worker: bool) -> Result<Reservation, ProtocolError> {
        let mut used = self.inner.lock().map_err(|_| {
            ProtocolError::new(ErrorCode::Internal, "resource accounting unavailable")
        })?;
        let next = used.memory.checked_add(memory).ok_or_else(|| {
            ProtocolError::new(ErrorCode::ResourceExhausted, "resource budget overflow")
        })?;
        if memory == 0 || (worker && used.jobs >= self.max_jobs) || next > self.max_memory {
            return Err(ProtocolError::new(
                ErrorCode::ResourceExhausted,
                "aggregate worker admission budget exhausted",
            ));
        }
        #[cfg(target_os = "linux")]
        {
            if let Ok(info) = std::fs::read_to_string("/proc/meminfo") {
                let available = info
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("MemAvailable:")
                            .and_then(|s| s.split_whitespace().next())
                            .and_then(|s| s.parse::<u64>().ok())
                    })
                    .unwrap_or(0)
                    .saturating_mul(1024);
                if available < memory.saturating_add(512 * 1024 * 1024) {
                    return Err(ProtocolError::new(
                        ErrorCode::ResourceExhausted,
                        "insufficient available memory including control reserve",
                    ));
                }
            } else {
                return Err(ProtocolError::new(
                    ErrorCode::Unavailable,
                    "cannot establish available worker memory",
                ));
            }
        }
        used.memory = next;
        used.jobs += usize::from(worker);
        Ok(Reservation {
            pool: self.clone(),
            memory,
            storage: 0,
            worker,
        })
    }
    pub fn reserved_storage(&self) -> u64 {
        self.inner
            .lock()
            .map(|used| used.storage)
            .unwrap_or(u64::MAX)
    }
}
impl Reservation {
    pub fn with_storage(mut self, bytes: u64, available: u64) -> Result<Self, ProtocolError> {
        {
            let mut used = self.pool.inner.lock().map_err(|_| {
                ProtocolError::new(ErrorCode::Internal, "storage accounting unavailable")
            })?;
            let next = used.storage.checked_add(bytes).ok_or_else(|| {
                ProtocolError::new(ErrorCode::ResourceExhausted, "storage reservation overflow")
            })?;
            if available < next.saturating_add(16 * 1024 * 1024) {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceExhausted,
                    "insufficient unreserved output storage",
                ));
            }
            used.storage = next;
            self.storage = bytes;
        }
        Ok(self)
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut used) = self.pool.inner.lock() {
            used.memory = used.memory.saturating_sub(self.memory);
            used.jobs = used.jobs.saturating_sub(usize::from(self.worker));
            used.storage = used.storage.saturating_sub(self.storage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregate_limits_and_release() {
        let pool = ResourcePool::new(1024, 1);
        let first = pool.reserve(512).unwrap();
        assert!(pool.reserve(1).is_err());
        drop(first);
        assert!(pool.reserve(512).is_ok());
        assert!(pool.reserve(2048).is_err());
    }
    #[test]
    fn storage_reservations_are_aggregate_and_release() {
        let pool = ResourcePool::new(2048, 2);
        let margin = 16 * 1024 * 1024;
        let first = pool
            .reserve(1)
            .unwrap()
            .with_storage(1024, margin + 1536)
            .unwrap();
        assert_eq!(pool.reserved_storage(), 1024);
        assert!(pool
            .reserve(1)
            .unwrap()
            .with_storage(1024, margin + 1536)
            .is_err());
        drop(first);
        assert_eq!(pool.reserved_storage(), 0);
    }
}
