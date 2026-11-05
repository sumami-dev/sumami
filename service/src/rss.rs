//! How much memory this process is using.
//!
//! The DuckDB engine reports its own usage (`duckdb_memoryBytes`), but that is
//! only *part* of the answer. What a small VPS runs out of is the **whole
//! process's resident set**: the binary's code pages, the writer's WAL buffer,
//! the rows the allocator has not handed back, and the engine's share.
//!
//! Two numbers, two jobs:
//!
//! * `peak_rss()` — the high-water mark, the number the memory budget is
//!   asserted against. Linux reads `VmHWM` from `/proc/self/status`.
//! * `current_rss()` — what it is right now, for the health endpoint.
//!
//! On Linux both come from `/proc/self/status`. Everywhere else there is no
//! portable way to ask without a syscall crate, so they return `None` and the
//! assertions that use them are skipped — the budget is asserted on **Linux CI**,
//! where the number is both available and meaningful (macOS counts the shared
//! pages of every mapped binary differently, so the number there is not the
//! number the VPS sees).

/// The resident set size of this process, in bytes.
///
/// `None` when the platform does not tell us — see the module documentation.
pub fn current_rss() -> Option<u64> {
    status_field("VmRSS")
}

/// The **peak** resident set size of this process, in bytes.
///
/// This is the number the memory budget is about: a process that allocated a
/// lot and gave it back still used the memory at the time.
pub fn peak_rss() -> Option<u64> {
    status_field("VmHWM")
}

/// Read one `kB` field out of `/proc/self/status`.
fn status_field(name: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        let rest = rest.trim_start_matches(':').trim();
        // The format is "1234 kB".
        let number: u64 = rest.split_whitespace().next()?.parse().ok()?;
        return Some(number * 1024);
    }
    None
}

/// The peak RSS in megabytes, for a human.
pub fn peak_rss_mb() -> Option<u64> {
    Some(peak_rss()? / (1024 * 1024))
}

/// The current resident set, best effort, on any platform.
///
/// Linux reads `/proc`. Everywhere else this shells out to `ps`, which is a
/// subprocess but not a syscall — the crate forbids `unsafe`.
///
/// The number is **not** the same as Linux's: macOS counts the shared pages of
/// every mapped binary differently, so it is for information, not for the
/// budget assertion.
pub fn current_rss_portable() -> Option<u64> {
    if let Some(bytes) = current_rss() {
        return Some(bytes);
    }
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let kilobytes: u64 = text.split_whitespace().next()?.parse().ok()?;
    Some(kilobytes * 1024)
}

/// Watch this process's resident set and remember the highest it saw.
///
/// Linux has the exact high-water mark (`VmHWM`). Everywhere else this samples
/// the current size and keeps the largest — close enough to be informative,
/// and it is what the local macOS run reports.
pub fn watch_peak(interval: std::time::Duration) -> PeakWatcher {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let peak = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let handle = std::thread::spawn({
        let stop = stop.clone();
        let peak = peak.clone();
        move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some(bytes) = current_rss_portable() {
                    peak.fetch_max(bytes, std::sync::atomic::Ordering::Relaxed);
                }
                std::thread::sleep(interval);
            }
            // One last look, in case it grew right at the end.
            if let Some(bytes) = current_rss_portable() {
                peak.fetch_max(bytes, std::sync::atomic::Ordering::Relaxed);
            }
        }
    });

    PeakWatcher {
        stop,
        peak,
        handle: Some(handle),
    }
}

/// A running memory watcher. Dropping it stops the sampling.
pub struct PeakWatcher {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    peak: std::sync::Arc<std::sync::atomic::AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PeakWatcher {
    /// The highest resident set seen so far, in bytes.
    pub fn peak(&self) -> u64 {
        self.peak.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for PeakWatcher {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_peak_is_at_least_the_current() {
        if let (Some(peak), Some(current)) = (peak_rss(), current_rss()) {
            assert!(
                peak >= current,
                "the peak ({peak}) cannot be below the current ({current})"
            );
        }
    }

    #[test]
    fn the_peak_is_plausible() {
        if let Some(peak) = peak_rss() {
            assert!(peak > 0, "a running process has a resident set");
            assert!(
                peak < 64 * 1024 * 1024 * 1024,
                "a 64 GB resident set is not plausible"
            );
        }
    }
}
