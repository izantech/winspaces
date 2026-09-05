//! Elevation management: scheduled task creation/deletion, daemon recycling,
//! and CLI helper entry points for `--enable-elevation` and `--disable-elevation`.

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::Command;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_COMMAND};
use winspaces_common::{log_error, log_info, log_warn};
use winspaces_ui::settings::autostart::{
    is_autostart_enabled, is_daemon_elevated, is_elevated_task_installed, is_run_key_enabled,
    set_run_key_enabled, ELEVATED_TASK_NAME,
};
use winspaces_win32::security::is_current_process_elevated;

use crate::app::find_daemon_window;
use crate::handlers::commands::ID_TRAY_EXIT;

const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Stop any currently running daemon gracefully and wait for its process to terminate.
pub(crate) fn stop_running_daemon() {
    unsafe {
        let hwnd = find_daemon_window();
        if !hwnd.is_null() {
            log_info!("Signaling running daemon to exit...");
            let mut pid = 0u32;
            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, &mut pid);

            const SYNCHRONIZE: u32 = 0x00100000;
            let hproc = if pid != 0 {
                windows_sys::Win32::System::Threading::OpenProcess(SYNCHRONIZE, 0, pid)
            } else {
                std::ptr::null_mut()
            };

            PostMessageW(hwnd, WM_COMMAND, ID_TRAY_EXIT as _, 0);

            if !hproc.is_null() {
                windows_sys::Win32::System::Threading::WaitForSingleObject(hproc, 5000);
                windows_sys::Win32::Foundation::CloseHandle(hproc);
            } else {
                for _ in 0..50 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    if find_daemon_window().is_null() {
                        break;
                    }
                }
            }
            log_info!("Running daemon stopped.");
        }
    }
}

/// The five characters XML reserves in text and attributes: an install path
/// such as `D:\Tools\Foo & Bar\` must not turn into a schtasks parse error.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Task Scheduler definition of the elevated daemon: logon trigger, highest
/// available run level, no time limit, one instance.
fn elevated_task_xml(exe_path: &std::path::Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>WinSpaces Per-Monitor Spaces Manager (Elevated Daemon)</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
    </Exec>
  </Actions>
</Task>"#,
        xml_escape(&exe_path.display().to_string())
    )
}

/// Create/register the elevated scheduled task pointing to the given executable.
pub(crate) fn install_elevated_task(exe_path: &std::path::Path) -> Result<(), String> {
    let xml = elevated_task_xml(exe_path);

    let temp_xml = std::env::temp_dir().join("winspaces_elevated_task.xml");
    let mut file = std::fs::File::create(&temp_xml)
        .map_err(|e| format!("Failed to create temp XML file: {e}"))?;

    // Write UTF-16LE with BOM (required by schtasks /xml parser)
    let mut u16_bytes = Vec::new();
    u16_bytes.extend_from_slice(&[0xFF, 0xFE]);
    for u in xml.encode_utf16() {
        u16_bytes.extend_from_slice(&u.to_le_bytes());
    }
    file.write_all(&u16_bytes)
        .map_err(|e| format!("Failed to write temp XML file: {e}"))?;
    drop(file);

    let output = Command::new("schtasks.exe")
        .args([
            "/create",
            "/tn",
            ELEVATED_TASK_NAME,
            "/xml",
            &temp_xml.to_string_lossy(),
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to execute schtasks: {e}"))?;

    let _ = std::fs::remove_file(&temp_xml);

    if output.status.success() {
        log_info!("Successfully registered scheduled task '{ELEVATED_TASK_NAME}'.");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("schtasks failed: {stderr}"))
    }
}

/// Remove the elevated scheduled task.
pub(crate) fn remove_elevated_task() -> Result<(), String> {
    let output = Command::new("schtasks.exe")
        .args(["/delete", "/tn", ELEVATED_TASK_NAME, "/f"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Failed to execute schtasks /delete: {e}"))?;

    if output.status.success() {
        log_info!("Successfully removed scheduled task '{ELEVATED_TASK_NAME}'.");
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("schtasks /delete failed: {stderr}"))
    }
}

/// Spawn the daemon detached in the background.
pub(crate) fn spawn_daemon_detached(exe_path: &std::path::Path) {
    match Command::new(exe_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            log_info!("Launched daemon in background (PID {})", child.id());
        }
        Err(e) => {
            log_error!("Failed to spawn daemon: {e}");
        }
    }
}

/// Handler for `--enable-elevation` / `--elevate-enable`.
pub(crate) fn handle_enable_elevation() {
    log_info!("--enable-elevation requested");
    if !is_current_process_elevated() {
        log_error!("Cannot enable elevation: helper process is not running elevated.");
        std::process::exit(1);
    }

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            log_error!("Failed to get current executable: {e}");
            std::process::exit(1);
        }
    };

    stop_running_daemon();

    if let Err(e) = install_elevated_task(&exe) {
        log_error!("Failed to install elevated task: {e}");
        std::process::exit(1);
    }

    // Clean up non-elevated HKCU Run value to prevent double start
    set_run_key_enabled(false);

    // Launch the new daemon elevated
    spawn_daemon_detached(&exe);
    log_info!("Administrator mode successfully enabled.");
}

/// Handler for `--disable-elevation` / `--elevate-disable`.
pub(crate) fn handle_disable_elevation() {
    log_info!("--disable-elevation requested");
    stop_running_daemon();

    if let Err(e) = remove_elevated_task() {
        log_warn!("Note when removing elevated task: {e}");
    }

    // Restore non-elevated HKCU Run entry so autostart is retained in standard mode
    set_run_key_enabled(true);

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            log_error!("Failed to get current executable: {e}");
            std::process::exit(1);
        }
    };

    // Launch standard non-elevated daemon
    spawn_daemon_detached(&exe);
    log_info!("Standard mode successfully restored.");
}

/// Handler for `--restart` / `--restart-daemon`.
pub(crate) fn handle_restart_daemon() {
    log_info!("--restart-daemon requested");
    stop_running_daemon();

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            log_error!("Failed to get current executable: {e}");
            std::process::exit(1);
        }
    };

    if is_elevated_task_installed() {
        let output = Command::new("schtasks.exe")
            .args(["/run", "/tn", ELEVATED_TASK_NAME])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        match output {
            Ok(out) if out.status.success() => {
                log_info!("Restarted elevated daemon via scheduled task.");
            }
            _ => {
                log_warn!("Failed to restart via scheduled task; spawning process directly.");
                spawn_daemon_detached(&exe);
            }
        }
    } else {
        spawn_daemon_detached(&exe);
    }
}

/// Handler for `--elevation-status`.
pub(crate) fn handle_elevation_status() {
    let proc_elevated = is_current_process_elevated();
    let task_installed = is_elevated_task_installed();
    let daemon_running = unsafe { !find_daemon_window().is_null() };
    let daemon_elevated = is_daemon_elevated();
    let run_key = is_run_key_enabled();
    let autostart = is_autostart_enabled();

    println!("WinSpaces Elevation Status:");
    println!("  Current Process Elevated: {}", proc_elevated);
    println!("  Daemon Running:           {}", daemon_running);
    println!("  Daemon Elevated:          {}", daemon_elevated);
    println!("  Elevated Task Installed:  {}", task_installed);
    println!("  HKCU Run Key Present:     {}", run_key);
    println!("  Effective Autostart:      {}", autostart);

    log_info!(
        "Status: proc_elevated={}, daemon_running={}, daemon_elevated={}, task_installed={}, run_key={}, autostart={}",
        proc_elevated,
        daemon_running,
        daemon_elevated,
        task_installed,
        run_key,
        autostart
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_escapes_the_command_path() {
        let xml = elevated_task_xml(std::path::Path::new(r"D:\Tools\Foo & Bar\winspaces.exe"));
        assert!(xml.contains(r"<Command>D:\Tools\Foo &amp; Bar\winspaces.exe</Command>"));
        assert!(!xml.contains("Foo & Bar"));
    }

    #[test]
    fn xml_escape_covers_every_reserved_character() {
        assert_eq!(
            xml_escape(r#"a&b<c>d"e'f"#),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
        assert_eq!(xml_escape(r"C:\plain\path.exe"), r"C:\plain\path.exe");
    }
}
