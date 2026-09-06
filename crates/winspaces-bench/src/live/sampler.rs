//! Reads the daemon's own counters: cycles, times, memory, GDI/USER
//! objects, handles, threads. Everything here is read-only against a
//! process opened with `PROCESS_QUERY_LIMITED_INFORMATION` — the same
//! handle `docs/benchmarks.md` §1 uses for `GetGuiResources`, which is the
//! only access right that survives the harness running at a lower
//! integrity level than an elevated daemon.

use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::Threading::{
    GetGuiResources, GetProcessHandleCount, GetProcessIoCounters, GetProcessTimes, OpenProcess,
    GR_GDIOBJECTS, GR_USEROBJECTS, IO_COUNTERS, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::System::WindowsProgramming::QueryProcessCycleTime;

use crate::report::Sample;

/// `PROCESS_QUERY_LIMITED_INFORMATION` (0x1000). The one access right every
/// counter here works through, elevated daemon or not.
const QUERY_LIMITED: u32 = PROCESS_QUERY_LIMITED_INFORMATION;

/// Reads one process's counters, holding an open handle across the whole
/// scenario so `cycles_delta` can be computed between calls.
pub struct Sampler {
    process: HANDLE,
    pid: u32,
    prev_cycles: Option<u64>,
}

// The handle is only ever touched from the thread that owns the Sampler;
// nothing here is shared across threads.
unsafe impl Send for Sampler {}

impl Sampler {
    pub fn new(pid: u32) -> Result<Sampler, String> {
        let process = unsafe { OpenProcess(QUERY_LIMITED, 0, pid) };
        if process.is_null() {
            return Err(format!(
                "OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) failed for pid {pid}"
            ));
        }
        Ok(Sampler {
            process,
            pid,
            prev_cycles: None,
        })
    }

    /// One reading. `t` is the caller's elapsed-seconds-in-phase clock.
    pub fn sample(&mut self, t: f64) -> Sample {
        let mut cycles = 0u64;
        unsafe { QueryProcessCycleTime(self.process, &mut cycles) };
        let cycles_delta = match self.prev_cycles {
            Some(prev) => cycles.saturating_sub(prev),
            None => 0,
        };
        self.prev_cycles = Some(cycles);

        let (mut creation, mut exit, mut kernel, mut user): (
            FILETIME,
            FILETIME,
            FILETIME,
            FILETIME,
        ) = unsafe { (zeroed(), zeroed(), zeroed(), zeroed()) };
        unsafe {
            GetProcessTimes(
                self.process,
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            );
        }

        let mut mem: PROCESS_MEMORY_COUNTERS_EX = unsafe { zeroed() };
        mem.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        unsafe {
            GetProcessMemoryInfo(
                self.process,
                (&mut mem as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
                mem.cb,
            );
        }

        let gdi = unsafe { GetGuiResources(self.process, GR_GDIOBJECTS) };
        let user_objects = unsafe { GetGuiResources(self.process, GR_USEROBJECTS) };

        let mut handles = 0u32;
        unsafe { GetProcessHandleCount(self.process, &mut handles) };

        let mut io: IO_COUNTERS = unsafe { zeroed() };
        let write_bytes = if unsafe { GetProcessIoCounters(self.process, &mut io) } != 0 {
            Some(io.WriteTransferCount)
        } else {
            None
        };

        Sample {
            t,
            cycles_delta,
            kernel_ms: filetime_to_ms(kernel),
            user_ms: filetime_to_ms(user),
            private_bytes: mem.PrivateUsage as u64,
            working_set: mem.WorkingSetSize as u64,
            peak_working_set: mem.PeakWorkingSetSize as u64,
            page_faults: mem.PageFaultCount,
            gdi,
            user_objects,
            handles,
            threads: count_threads(self.pid),
            write_bytes,
        }
    }

    /// A single fast GDI-object reading, independent of the cycle, handle
    /// and memory counters `sample` gathers. Used for the tight in-paint
    /// peak polling loop (`docs/benchmarks.md` §4.3), where a full `sample`
    /// call's extra syscalls would themselves widen the window in which the
    /// transient paint DIB could be missed.
    pub fn peek_gdi(&self) -> u32 {
        unsafe { GetGuiResources(self.process, GR_GDIOBJECTS) }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.process);
        }
    }
}

/// A `FILETIME` is 100 ns units since 1601; kernel/user time from
/// `GetProcessTimes` is a duration in that unit, converted to whole
/// milliseconds of CPU time.
fn filetime_to_ms(ft: FILETIME) -> f64 {
    let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    ticks as f64 / 10_000.0
}

/// Threads currently owned by `pid`, via a Toolhelp snapshot. `0` when the
/// snapshot itself fails rather than propagating an error — a thread count
/// is a nice-to-have field, not a scenario blocker.
fn count_threads(pid: u32) -> u32 {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return 0;
        }
        let mut count = 0u32;
        let mut entry: THREADENTRY32 = zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        if Thread32First(snap, &mut entry) != 0 {
            loop {
                if entry.th32OwnerProcessID == pid {
                    count += 1;
                }
                entry.dwSize = size_of::<THREADENTRY32>() as u32;
                if Thread32Next(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_reads_its_own_process() {
        // The harness's own pid is always a legal, non-elevated-crossing
        // target, so this proves the counter plumbing without a daemon.
        let pid = std::process::id();
        let mut sampler = Sampler::new(pid).expect("open self");
        let s1 = sampler.sample(0.0);
        assert_eq!(s1.cycles_delta, 0, "first sample has no prior reading");
        assert!(s1.private_bytes > 0);
        assert!(s1.threads > 0);

        // Burn some cycles so the second reading is provably a delta.
        let mut x = 0u64;
        for i in 0..5_000_000u64 {
            x = std::hint::black_box(x.wrapping_add(i));
        }
        let s2 = sampler.sample(1.0);
        assert!(s2.cycles_delta > 0, "cycles should have advanced");
    }

    #[test]
    fn filetime_conversion_is_100ns_units() {
        // 10_000_000 ticks == 1 second == 1000 ms.
        let ft = FILETIME {
            dwLowDateTime: 10_000_000,
            dwHighDateTime: 0,
        };
        assert_eq!(filetime_to_ms(ft), 1000.0);
    }
}
