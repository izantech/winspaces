//! The machine and build stamps every report carries. A number without
//! them is unfalsifiable (`docs/benchmarks.md` §1).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use winspaces_win32::text::encode_wide;

use crate::report::repo_root;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Machine {
    pub cpu: String,
    pub mhz: u32,
    pub logical_cpus: u32,
    /// `10.0.<build>`.
    pub os_build: String,
    pub monitors: Vec<MonitorInfo>,
    pub harness_elevated: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct MonitorInfo {
    pub device: String,
    pub width: i32,
    pub height: i32,
    pub dpi: u32,
    pub primary: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Build {
    /// Short (7) sha of `HEAD`; empty when git is unavailable.
    pub git_sha: String,
    pub git_dirty: bool,
    /// `[workspace.package].version`, from this crate's manifest.
    pub version: String,
    /// The profile this tool was built with, which is the profile the
    /// in-process groups measure: `release` or `debug`.
    pub profile: String,
    /// The daemon exe the live and static groups look at.
    pub exe_path: String,
    /// 0 when it does not exist.
    pub exe_size: u64,
}

const CPU_KEY: &str = "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0";

fn read_hklm_string(subkey: &str, value: &str) -> Option<String> {
    let key = encode_wide(subkey);
    let name = encode_wide(value);
    let mut size: u32 = 0;
    unsafe {
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        ) != 0
            || size < 2
        {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut size,
        ) != 0
        {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]).trim().to_string())
    }
}

fn read_hklm_u32(subkey: &str, value: &str) -> Option<u32> {
    let key = encode_wide(subkey);
    let name = encode_wide(value);
    let mut data: u32 = 0;
    let mut size: u32 = std::mem::size_of::<u32>() as u32;
    unsafe {
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut data as *mut u32 as *mut _,
            &mut size,
        ) == 0
        {
            Some(data)
        } else {
            None
        }
    }
}

unsafe extern "system" fn collect_monitor(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> i32 {
    let out = &mut *(lparam as *mut Vec<MonitorInfo>);
    let mut info: MONITORINFOEXW = std::mem::zeroed();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut info as *mut _ as *mut _) != 0 {
        let len = info
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.szDevice.len());
        let mut dpi_x = 96u32;
        let mut dpi_y = 96u32;
        GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let r = info.monitorInfo.rcMonitor;
        out.push(MonitorInfo {
            device: String::from_utf16_lossy(&info.szDevice[..len]),
            width: r.right - r.left,
            height: r.bottom - r.top,
            dpi: dpi_x,
            primary: info.monitorInfo.dwFlags & 1 != 0,
        });
    }
    1
}

pub fn monitors() -> Vec<MonitorInfo> {
    let mut out: Vec<MonitorInfo> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitor),
            &mut out as *mut _ as LPARAM,
        );
    }
    out
}

impl Machine {
    pub fn detect() -> Machine {
        Machine {
            cpu: read_hklm_string(CPU_KEY, "ProcessorNameString").unwrap_or_default(),
            mhz: read_hklm_u32(CPU_KEY, "~MHz").unwrap_or(0),
            logical_cpus: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(0),
            os_build: format!("10.0.{}", winspaces_win32::module::win_build()),
            monitors: monitors(),
            harness_elevated: winspaces_win32::security::is_current_process_elevated(),
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The daemon exe the live and static groups default to: the release build
/// in this repository's `target`.
pub fn default_daemon_exe() -> PathBuf {
    repo_root()
        .join("target")
        .join("release")
        .join("winspaces.exe")
}

impl Build {
    pub fn detect(exe: &Path) -> Build {
        let root = repo_root();
        Build {
            git_sha: git(&root, &["rev-parse", "--short=7", "HEAD"]).unwrap_or_default(),
            git_dirty: git(&root, &["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|s| !s.is_empty()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            profile: if cfg!(debug_assertions) {
                "debug".to_string()
            } else {
                "release".to_string()
            },
            exe_path: exe.display().to_string(),
            exe_size: std::fs::metadata(exe).map(|m| m.len()).unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_stamp_is_populated() {
        let m = Machine::detect();
        assert!(!m.cpu.is_empty(), "CPU name from the registry");
        assert!(m.logical_cpus > 0);
        assert!(m.os_build.starts_with("10.0."));
        // A headless CI runner still has at least one display.
        assert!(!m.monitors.is_empty());
    }

    #[test]
    fn build_stamp_reads_the_workspace_version() {
        let b = Build::detect(Path::new("Z:\\does\\not\\exist.exe"));
        assert_eq!(b.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(b.exe_size, 0);
        assert!(b.profile == "debug" || b.profile == "release");
    }
}
