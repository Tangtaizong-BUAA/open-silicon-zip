//! Cooperative cancellation token and lock-free progress counters.
use crate::error::{Error, ErrorKind, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self { Self::default() }
    /// CLI-only setup: signal handlers set the same atomic flag checked by workers.
    #[cfg(unix)]
    pub fn install_signal_handlers(&self) -> std::io::Result<()> {
        signal_hook::flag::register(signal_hook::consts::SIGINT, self.0.clone())?;
        signal_hook::flag::register(signal_hook::consts::SIGTERM, self.0.clone())?;
        Ok(())
    }

    pub fn cancel(&self) { self.0.store(true, Relaxed) }
    pub fn is_cancelled(&self) -> bool { self.0.load(Relaxed) }
    /// Call at chunk boundaries (<= 100 ms apart).
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() { Err(Error::new(ErrorKind::Cancelled, "cancelled")) } else { Ok(()) }
    }
}

#[derive(Default)]
struct Inner {
    bytes_read: AtomicU64,
    bytes_written: AtomicU64,
    files_done: AtomicU64,
    files_total: AtomicU64,
    bytes_total: AtomicU64,
    current: Mutex<String>,
    phase: Mutex<&'static str>,
}

/// Shared progress sink; cheap to clone, safe to poll at >= 5 Hz.
#[derive(Clone, Default)]
pub struct Progress(Arc<Inner>);

#[derive(Debug, Clone, Default)]
pub struct ProgressSnapshot {
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_total: u64,
    pub current: String,
    pub phase: &'static str,
}

impl Progress {
    pub fn new() -> Self { Self::default() }
    pub fn add_read(&self, n: u64) { self.0.bytes_read.fetch_add(n, Relaxed); }
    pub fn add_written(&self, n: u64) { self.0.bytes_written.fetch_add(n, Relaxed); }
    pub fn file_done(&self) { self.0.files_done.fetch_add(1, Relaxed); }
    pub fn set_totals(&self, files: u64, bytes: u64) {
        self.0.files_total.store(files, Relaxed);
        self.0.bytes_total.store(bytes, Relaxed);
    }
    pub fn set_current(&self, name: &str) {
        if let Ok(mut c) = self.0.current.try_lock() { c.clear(); c.push_str(name); }
    }
    pub fn set_phase(&self, p: &'static str) { *self.0.phase.lock().unwrap() = p; }
    pub fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            bytes_read: self.0.bytes_read.load(Relaxed),
            bytes_written: self.0.bytes_written.load(Relaxed),
            files_done: self.0.files_done.load(Relaxed),
            files_total: self.0.files_total.load(Relaxed),
            bytes_total: self.0.bytes_total.load(Relaxed),
            current: self.0.current.lock().unwrap().clone(),
            phase: *self.0.phase.lock().unwrap(),
        }
    }
}

/// io::Error used by cancel-aware readers/writers. Must NOT be `ErrorKind::Interrupted`: `write_all`,
/// `io::copy` and most decoders silently retry on that kind, which turns a cancel into a busy loop.
pub fn io_cancelled() -> std::io::Error { std::io::Error::other(CANCEL_MSG) }
pub(crate) const CANCEL_MSG: &str = "fz: operation cancelled";
