//! Memory tuning budget and thread count (design doc 3.3). Worker counts, batch sizes,
//! spill thresholds and resolver cache limits derive from this budget. It is not an RSS cap.

use crate::brand::env;

#[derive(Clone, Copy, Debug)]
pub struct MemBudget {
    /// Total budget in bytes: available memory x 0.6, or `CGR_MEMORY_MB`.
    pub total: u64,
    /// In-flight extraction data (<= 25%).
    pub extract: u64,
    /// Resolution temporary tables (<= 35%).
    pub resolve: u64,
    /// Synthesis caches (<= 15%).
    pub synth: u64,
    /// Indexing worker threads.
    pub threads: usize,
}

const MB: u64 = 1 << 20;
/// A worker's own working set (parser, walker state, a few files) in bytes.
const PER_THREAD_BYTES: u64 = 96 * MB;

impl MemBudget {
    pub fn from_total(total: u64, threads: usize) -> MemBudget {
        let total = total.max(256 * MB);
        let by_mem = (total / 4 / PER_THREAD_BYTES).max(1) as usize;
        MemBudget { total, extract: total / 4, resolve: total / 100 * 35, synth: total / 100 * 15, threads: threads.min(by_mem).max(1) }
    }

    /// Detect from the environment, honouring `CGR_MEMORY_MB` / `CGR_THREADS`.
    pub fn detect() -> MemBudget {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        let threads = env::get(env::THREADS, None).and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0).unwrap_or(cores);
        let total = env::get(env::MEMORY_MB, None)
            .and_then(|v| v.parse::<u64>().ok())
            .map(|mb| mb.saturating_mul(MB))
            .unwrap_or_else(|| available_memory().map(|a| a * 6 / 10).unwrap_or(2048 * MB));
        MemBudget::from_total(total, threads)
    }

    /// Apply command-line overrides without allowing an explicit thread count to bypass
    /// the memory-derived worker limit.
    pub fn configured(memory_mb: Option<u64>, threads: Option<usize>) -> MemBudget {
        let detected = Self::detect();
        Self::from_total(memory_mb.map(|mb| mb.saturating_mul(MB)).unwrap_or(detected.total), threads.unwrap_or(detected.threads))
    }

    /// Source bytes one extraction batch may hold (a fraction of the extract share, since a
    /// file's tables are several times its source).
    pub fn extract_batch_bytes(&self) -> u64 {
        (self.extract / 8).clamp(8 * MB, 256 * MB)
    }

    /// One worker's optional resolver cache share. Clearing a cache only recomputes a
    /// lookup; it never truncates candidate sets or changes resolution semantics.
    pub fn worker_cache_bytes(&self) -> usize {
        (self.resolve / self.threads.max(1) as u64 / 8).clamp(64 << 10, 16 << 20) as usize
    }
}

/// Memory this process could still use, in bytes (design doc 3.3): the smaller of the
/// cgroup limit headroom (counting reclaimable file cache as free) and `MemAvailable`.
pub fn available_memory() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        linux_available()
    }
    #[cfg(target_os = "macos")]
    {
        macos_available()
    }
    #[cfg(windows)]
    {
        windows_available()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_u64(path: &str) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn linux_available() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = |key: &str| -> Option<u64> { meminfo.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse::<u64>().ok() };
    let mut avail = kb("MemAvailable:")? * 1024;
    // cgroup v2: memory.max - memory.current, plus inactive_file (page cache counts as used).
    if let Ok(max) = std::fs::read_to_string("/sys/fs/cgroup/memory.max") {
        if let Ok(max) = max.trim().parse::<u64>() {
            let cur = read_u64("/sys/fs/cgroup/memory.current").unwrap_or(0);
            let inactive = std::fs::read_to_string("/sys/fs/cgroup/memory.stat")
                .ok()
                .and_then(|s| s.lines().find(|l| l.starts_with("inactive_file ")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
                .unwrap_or(0);
            avail = avail.min(max.saturating_sub(cur.saturating_sub(inactive)));
        }
    } else if let Some(limit) = read_u64("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
        // cgroup v1; "unlimited" shows up as a huge number.
        if limit < (1 << 60) {
            let usage = read_u64("/sys/fs/cgroup/memory/memory.usage_in_bytes").unwrap_or(0);
            avail = avail.min(limit.saturating_sub(usage));
        }
    }
    Some(avail)
}

#[cfg(target_os = "macos")]
#[allow(deprecated)] // libc still exposes the Mach VM ABI used here; avoid another runtime dependency.
fn macos_available() -> Option<u64> {
    extern "C" {
        fn mach_port_deallocate(task: libc::mach_port_t, name: libc::mach_port_t) -> libc::kern_return_t;
    }
    // Use Mach directly: detecting a budget must not spawn vm_stat on a hot build path.
    // SAFETY: the kernel fills the correctly sized plain VM statistics struct we own.
    unsafe {
        let host = libc::mach_host_self();
        let mut vm: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let result = libc::host_statistics64(host, libc::HOST_VM_INFO64, (&mut vm as *mut libc::vm_statistics64).cast(), &mut count);
        let page = libc::sysconf(libc::_SC_PAGESIZE);
        mach_port_deallocate(libc::mach_task_self(), host);
        if result != libc::KERN_SUCCESS || page <= 0 {
            return None;
        }
        Some((vm.free_count as u64 + vm.inactive_count as u64 + vm.speculative_count as u64) * page as u64)
    }
}

#[cfg(windows)]
fn windows_available() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatus {
        length: u32,
        load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page: u64,
        avail_page: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
    }
    // SAFETY: Windows checks length, then fills this documented MEMORYSTATUSEX layout.
    unsafe {
        let mut status: MemoryStatus = std::mem::zeroed();
        status.length = std::mem::size_of::<MemoryStatus>() as u32;
        (GlobalMemoryStatusEx(&mut status) != 0).then_some(status.avail_phys)
    }
}

/// Peak resident set size of this process in MiB (best effort, 0 if unknown).
pub fn peak_rss_mb() -> u64 {
    #[cfg(unix)]
    {
        // SAFETY: getrusage fills a plain struct we own.
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } == 0 {
            let v = ru.ru_maxrss as u64;
            // Linux reports KiB, macOS bytes
            return if cfg!(target_os = "macos") { v >> 20 } else { v >> 10 };
        }
    }
    0
}

/// Current resident set size in MiB (Linux `/proc/self/statm`; 0 elsewhere).
pub fn current_rss_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(pages) = s.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()) {
                return (pages * 4096) >> 20;
            }
        }
    }
    0
}

/// Lower only the indexing worker's scheduling priority, leaving the MCP / query threads
/// responsive. macOS uses thread QoS because process-wide nice would also lower readers.
pub fn lower_thread_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: on Linux, who=0 changes the calling thread's priority.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative: i32) -> i32;
        }
        // QOS_CLASS_UTILITY, from the macOS pthread/qos.h ABI.
        // SAFETY: changes the scheduling class of this worker only.
        unsafe {
            pthread_set_qos_class_self_np(0x11, 0);
        }
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> *mut std::ffi::c_void;
            fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
        }
        // SAFETY: the pseudo-handle is valid for this thread; -1 is BELOW_NORMAL.
        unsafe {
            SetThreadPriority(GetCurrentThread(), -1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_never_exceed_total() {
        let b = MemBudget::from_total(4096 * MB, 8);
        assert!(b.extract + b.resolve + b.synth < b.total);
        assert!(b.threads >= 1 && b.threads <= 8);
    }

    #[test]
    fn small_budget_limits_threads() {
        let b = MemBudget::from_total(256 * MB, 16);
        assert_eq!(b.threads, 1);
    }
    #[test]
    fn explicit_threads_still_respect_the_memory_share() {
        let b = MemBudget::configured(Some(256), Some(1024));
        assert_eq!(b.total, 256 * MB);
        assert_eq!(b.threads, 1);
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        assert!(available_memory().is_some());
    }
}
