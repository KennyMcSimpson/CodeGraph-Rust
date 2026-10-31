//! Memory budget and thread count (design doc 3.3). Every cache, queue and batch size in
//! the engine derives from `MemBudget`; nothing grows without a bound.

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
        MemBudget {
            total,
            extract: total / 4,
            resolve: total * 35 / 100,
            synth: total * 15 / 100,
            threads: threads.min(by_mem).max(1),
        }
    }

    /// Detect from the environment, honouring `CGR_MEMORY_MB` / `CGR_THREADS`.
    pub fn detect() -> MemBudget {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        let threads = env::get(env::THREADS, None)
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(cores);
        let total = env::get(env::MEMORY_MB, None)
            .and_then(|v| v.parse::<u64>().ok())
            .map(|mb| mb * MB)
            .unwrap_or_else(|| available_memory().map(|a| a * 6 / 10).unwrap_or(2048 * MB));
        MemBudget::from_total(total, threads)
    }

    /// Source bytes one extraction batch may hold (a fraction of the extract share, since a
    /// file's tables are several times its source).
    pub fn extract_batch_bytes(&self) -> u64 {
        (self.extract / 8).clamp(8 * MB, 256 * MB)
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
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
    let kb = |key: &str| -> Option<u64> {
        meminfo.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse::<u64>().ok()
    };
    let mut avail = kb("MemAvailable:")? * 1024;
    // cgroup v2: memory.max - memory.current, plus inactive_file (page cache counts as used).
    if let Ok(max) = std::fs::read_to_string("/sys/fs/cgroup/memory.max") {
        if let Ok(max) = max.trim().parse::<u64>() {
            let cur = read_u64("/sys/fs/cgroup/memory.current").unwrap_or(0);
            let inactive = std::fs::read_to_string("/sys/fs/cgroup/memory.stat")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find(|l| l.starts_with("inactive_file "))
                        .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
                })
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
fn macos_available() -> Option<u64> {
    // vm_stat reports pages; free + inactive + speculative is what the system can hand out.
    let out = std::process::Command::new("vm_stat").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let page: u64 = text
        .lines()
        .next()
        .and_then(|l| l.split("page size of ").nth(1))
        .and_then(|l| l.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(16384);
    let pages = |key: &str| -> u64 {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().trim_end_matches('.').parse().ok())
            .unwrap_or(0)
    };
    let free = pages("Pages free") + pages("Pages inactive") + pages("Pages speculative");
    Some(free * page)
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
}
