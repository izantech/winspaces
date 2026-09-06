//! The in-process runner for the `micro` and `primitives` groups.
//!
//! Every benchmark is a closure the runner calls `iters` times per sample.
//! Setup belongs outside the closure: build the inputs first and borrow them
//! inside, so the sample measures only the call. Results carry two units:
//! wall nanoseconds per iteration (`Instant`, which is `QueryPerformanceCounter`
//! on Windows) and thread cycles per iteration (`QueryThreadCycleTime`), the
//! same unit `docs/benchmarks.md` prices the daemon in. Cycles exclude time
//! the thread was descheduled; wall time does not, which is why both are kept.

use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::GetCurrentThread;
use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;

use crate::report::BenchEntry;
use crate::stats::{fmt_cycles, fmt_ns, Summary};

/// Samples per benchmark outside smoke mode.
pub const SAMPLES: usize = 30;
/// Iterations are calibrated so one sample lasts about this long.
pub const TARGET_SAMPLE: Duration = Duration::from_millis(5);
pub const MAX_ITERS: u64 = 1_000_000;

/// Cycles retired by the calling thread so far, user and kernel mode.
pub fn thread_cycles() -> u64 {
    let mut cycles = 0u64;
    unsafe {
        QueryThreadCycleTime(GetCurrentThread(), &mut cycles);
    }
    cycles
}

pub struct Runner {
    smoke: bool,
    filter: Option<String>,
    pub verbose: bool,
    pub entries: Vec<BenchEntry>,
    /// Benchmarks that could not run on this machine (a missing window, a
    /// denied API); recorded so a report never mistakes absence for zero.
    pub skipped: Vec<(String, String)>,
}

impl Runner {
    pub fn new(smoke: bool, filter: Option<String>, verbose: bool) -> Runner {
        Runner {
            smoke,
            filter,
            verbose,
            entries: Vec::new(),
            skipped: Vec::new(),
        }
    }

    /// `--filter <substr>` selects by name.
    pub fn selected(&self, name: &str) -> bool {
        self.filter.as_deref().is_none_or(|f| name.contains(f))
    }

    pub fn skip(&mut self, name: &str, reason: &str) {
        if !self.selected(name) {
            return;
        }
        eprintln!("  skip  {name}: {reason}");
        self.skipped.push((name.to_string(), reason.to_string()));
    }

    /// Time `f`, calibrated so a sample lasts about `TARGET_SAMPLE`, then
    /// `SAMPLES` samples (one iteration, one sample in smoke mode).
    pub fn bench<F: FnMut()>(&mut self, name: &str, mut f: F) {
        if !self.selected(name) {
            return;
        }
        let (iters, samples) = if self.smoke {
            (1u64, 1usize)
        } else {
            // Calibrate on one warm call, then run one full warm-up sample
            // so caches and allocator state look like steady state.
            f();
            let started = Instant::now();
            f();
            let one = started.elapsed().as_nanos().max(1);
            let iters = (TARGET_SAMPLE.as_nanos() / one).clamp(1, MAX_ITERS as u128) as u64;
            for _ in 0..iters {
                f();
            }
            (iters, SAMPLES)
        };

        let mut ns = Vec::with_capacity(samples);
        let mut cycles = Vec::with_capacity(samples);
        for _ in 0..samples {
            let c0 = thread_cycles();
            let t0 = Instant::now();
            for _ in 0..iters {
                f();
            }
            let elapsed = t0.elapsed();
            let c1 = thread_cycles();
            let ns_per_iter = elapsed.as_nanos() as f64 / iters as f64;
            let cycles_per_iter = c1.saturating_sub(c0) as f64 / iters as f64;
            if self.verbose {
                eprintln!(
                    "    sample {:>2}: {} / {}",
                    ns.len() + 1,
                    fmt_ns(ns_per_iter),
                    fmt_cycles(cycles_per_iter)
                );
            }
            ns.push(ns_per_iter);
            cycles.push(cycles_per_iter);
        }
        let entry = BenchEntry {
            name: name.to_string(),
            iters,
            samples,
            ns: Summary::of(&ns),
            cycles: Summary::of(&cycles),
        };
        eprintln!(
            "  bench {:<44} median {:>10}  {:>10}  (x{} iters, {} samples)",
            entry.name,
            fmt_ns(entry.ns.median),
            fmt_cycles(entry.cycles.median),
            iters,
            samples
        );
        self.entries.push(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_runs_each_benchmark_once() {
        let mut r = Runner::new(true, None, false);
        let mut calls = 0;
        r.bench("t/one", || calls += 1);
        assert_eq!(calls, 1);
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].iters, 1);
        assert_eq!(r.entries[0].samples, 1);
    }

    #[test]
    fn filter_selects_by_substring() {
        let mut r = Runner::new(true, Some("keep".into()), false);
        r.bench("a/keep/me", || {});
        r.bench("a/drop/me", || {});
        assert_eq!(r.entries.len(), 1);
        assert_eq!(r.entries[0].name, "a/keep/me");
    }

    #[test]
    fn thread_cycles_advance() {
        use std::hint::black_box;
        let a = thread_cycles();
        let mut x = 0u64;
        for i in 0..100_000u64 {
            x = black_box(x.wrapping_add(i));
        }
        let b = thread_cycles();
        assert!(b > a, "cycle counter did not advance ({a} -> {b})");
    }
}
