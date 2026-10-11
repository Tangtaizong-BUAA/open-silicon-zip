//! Cached hardware queries, without shell subprocesses on the compression path.
use std::sync::OnceLock;

#[cfg(target_os = "macos")]
fn sysctl_value(name: &std::ffi::CStr) -> Option<u64> {
    unsafe extern "C" {
        fn sysctlbyname(name: *const std::ffi::c_char, oldp: *mut std::ffi::c_void,
            oldlenp: *mut usize, newp: *mut std::ffi::c_void, newlen: usize) -> std::ffi::c_int;
    }
    let mut value = 0u64;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: the writable buffer is eight bytes; sysctl receives its exact capacity.
    let result = unsafe { sysctlbyname(name.as_ptr(), (&mut value as *mut u64).cast(),
        &mut size, std::ptr::null_mut(), 0) };
    (result == 0 && (size == 4 || size == 8)).then_some(value)
}

pub fn perf_cores() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        #[cfg(target_os = "macos")]
        if let Some(n) = sysctl_value(c"hw.perflevel0.physicalcpu").filter(|n| *n > 0) { return n as usize; }
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    })
}

pub fn total_memory() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| {
        #[cfg(target_os = "macos")]
        if let Some(n) = sysctl_value(c"hw.memsize").filter(|n| *n > 0) { return n; }
        #[cfg(target_os = "linux")]
        if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
            for line in s.lines() {
                if let Some(kb) = line.strip_prefix("MemTotal:").and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse::<u64>().ok()) { return kb.saturating_mul(1024); }
            }
        }
        4 << 30
    })
}

pub fn default_memory_budget() -> u64 { (total_memory() / 4).min(4 << 30) }

pub fn worker_count(threads: usize) -> usize {
    if threads == 0 { perf_cores().clamp(1, 64) } else { threads.clamp(1, 256) }
}

/// One CPU-bound worker per performance core. Explicit counts remain bounded.
pub fn build_pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(worker_count(threads))
        .thread_name(|i| format!("fz-codec-{i}")).build()
        .or_else(|_| rayon::ThreadPoolBuilder::new().num_threads(1).use_current_thread().build())
        .expect("unable to initialize even a single-thread codec pool")
}
