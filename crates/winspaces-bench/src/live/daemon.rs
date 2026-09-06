//! Finding the daemon, telling whether it (or the harness) runs elevated,
//! and the `--own` lifecycle: stop the user's daemon, run a chosen exe,
//! restore their posture afterwards. `--own` is the only code path here
//! that may start or stop a daemon; nothing else in this crate does.

use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use winspaces_core::daemon::{find_daemon_window, is_daemon_running};

use crate::live::driver;

const EXIT_WAIT: Duration = Duration::from_secs(15);
const LOCK_POLL: Duration = Duration::from_millis(100);

/// Whether the process at `pid` runs elevated, queried through its own
/// token rather than the daemon's `PROP_ELEVATED` window prop — this works
/// for any pid, at any integrity level relative to the caller, as long as
/// `PROCESS_QUERY_LIMITED_INFORMATION` succeeds (it always does across
/// integrity levels; `TOKEN_QUERY` on the resulting token does too).
pub fn is_pid_elevated(pid: u32) -> bool {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut token = std::ptr::null_mut();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token) != 0;
        CloseHandle(process);
        if !opened {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        ) != 0;
        CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Any `winspaces.exe` invocation with `--exit` reaches the running
/// daemon's message window and passes its UIPI allow-list (`WM_COMMAND` is
/// on it), so this is legal to run from a lower-integrity harness even
/// against an elevated daemon.
fn exit_running_daemon(exe: &Path) -> Result<(), String> {
    let status = std::process::Command::new(exe)
        .arg("--exit")
        .status()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    if !status.success() {
        return Err(format!("{} --exit exited with {status}", exe.display()));
    }
    Ok(())
}

fn restart_daemon(exe: &Path) -> Result<(), String> {
    let status = std::process::Command::new(exe)
        .arg("--restart")
        .status()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    if !status.success() {
        return Err(format!("{} --restart exited with {status}", exe.display()));
    }
    Ok(())
}

fn wait_for_daemon_gone(timeout: Duration) -> bool {
    driver::wait_until(timeout, || find_daemon_window().is_null())
}

/// Waits until `exe` can be opened for exclusive write access — the same
/// test the OS applies when a running process holds the file open. Used
/// both to confirm the old daemon actually released the file and, for a
/// caller building a fresh binary, to know a stale lock is really gone.
fn wait_for_exclusive_write(exe: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let opened = OpenOptions::new().write(true).share_mode(0).open(exe);
        if opened.is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(LOCK_POLL);
    }
}

/// The `--own` lifecycle: stops whatever daemon is running (remembering
/// whether one was) so the caller can then start `exe` however it likes —
/// timed, as the `startup` scenario does, or plainly — and hands back
/// whether a daemon had to be stopped so [`Owned::restore`] can put it back.
pub struct Owned {
    had_daemon: bool,
    exe: std::path::PathBuf,
}

impl Owned {
    /// Stops any running daemon (any `winspaces.exe`, elevated or not — the
    /// `--exit` invocation always reaches it) and waits for the exe lock to
    /// clear. Does **not** start a new one: that is either the `startup`
    /// scenario's job (it is timing the spawn) or [`spawn_and_wait`]'s.
    pub fn stop_existing(exe: &Path) -> Result<Owned, String> {
        let had_daemon = is_daemon_running();
        if had_daemon {
            exit_running_daemon(exe)?;
            if !wait_for_daemon_gone(EXIT_WAIT) {
                return Err("daemon did not exit within 15s of --exit".to_string());
            }
            if !wait_for_exclusive_write(exe, EXIT_WAIT) {
                return Err(format!(
                    "{} is still locked 15s after the daemon exited",
                    exe.display()
                ));
            }
        }
        Ok(Owned {
            had_daemon,
            exe: exe.to_path_buf(),
        })
    }

    /// Stops the daemon this harness started, then — only if a daemon was
    /// running before `stop_existing` — restarts through `--restart`, which
    /// uses the elevated scheduled task when one is installed and so
    /// restores the user's posture rather than always coming back
    /// non-elevated. Called on every exit path, including error ones, so
    /// `--own` never leaves the user without their daemon.
    pub fn restore(self) -> Result<(), String> {
        exit_running_daemon(&self.exe)?;
        wait_for_daemon_gone(EXIT_WAIT);
        if self.had_daemon {
            // `--restart` spawns the exe it was invoked from when no
            // elevated task is installed, so invoke the repository's own
            // release daemon, not the exe under measurement: an `ab` base
            // run points `--exe` at a throwaway worktree build that must
            // never end up as the user's daemon.
            let default_exe = crate::stamp::default_daemon_exe();
            let restart_exe = if default_exe.exists() {
                default_exe
            } else {
                self.exe.clone()
            };
            restart_daemon(&restart_exe)?;
        }
        Ok(())
    }
}

/// Spawns `exe` detached and waits up to 15 s for its message window —
/// the plain (untimed) half of "start the daemon". The `startup` scenario
/// does this itself, instrumented, when it is in the scenario list.
pub fn spawn_and_wait(exe: &Path) -> Result<(), String> {
    std::process::Command::new(exe)
        .spawn()
        .map_err(|e| format!("spawning {}: {e}", exe.display()))?;
    if !driver::wait_until(EXIT_WAIT, || !find_daemon_window().is_null()) {
        return Err(format!(
            "{} did not create its message window within 15s",
            exe.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_pid_elevation_matches_the_direct_query() {
        // The harness querying its own pid through OpenProcess should agree
        // with winspaces_win32::security's same-process check.
        let pid = std::process::id();
        assert_eq!(
            is_pid_elevated(pid),
            winspaces_win32::security::is_current_process_elevated()
        );
    }
}
