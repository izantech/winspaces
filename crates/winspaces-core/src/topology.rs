//! Stable display identity and topology signatures.
//!
//! `MONITORINFOEXW.szDevice` (`\\.\DISPLAY1`, `\\.\DISPLAY2`, ...) names a GDI
//! output *slot*, not a physical monitor. Windows recycles those slots by
//! attach order, so an RDP connect — which detaches every physical display and
//! attaches the RDP Indirect Display Driver — can hand the virtual display a
//! slot name a real monitor held moments earlier. Keying per-monitor state on
//! `szDevice` therefore grafts one monitor's spaces onto another's.
//!
//! `QueryDisplayConfig` exposes the monitor's device *path*
//! (`\\?\DISPLAY#BNQ805B#5&1f33c64f&0&UID4354#{...}`), which embeds the EDID
//! manufacturer/product code and the physical connector instance. That survives
//! reordering, docking and RDP, and the RDP IDD gets its own distinct path so
//! it can never be mistaken for a physical panel.

use std::collections::HashMap;

use windows_sys::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_REMOTESESSION};
use winspaces_win32::text::wide_to_string;

/// True while the desktop is being driven by a remote-desktop session. In that
/// state the physical monitors are detached and the topology is the RDP
/// Indirect Display Driver's, so any layout observed is disposable.
pub fn is_remote_session() -> bool {
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

/// Map every active GDI device name (`\\.\DISPLAYn`) to its monitor device
/// path. Returns an empty map when the display-config API is unavailable, in
/// which case callers fall back to the device name — the previous behavior.
pub fn stable_monitor_ids() -> HashMap<String, String> {
    let mut map = HashMap::new();
    unsafe {
        let mut path_count: u32 = 0;
        let mut mode_count: u32 = 0;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            != ERROR_SUCCESS
        {
            return map;
        }

        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = vec![std::mem::zeroed(); path_count as usize];
        let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![std::mem::zeroed(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            std::ptr::null_mut(),
        ) != ERROR_SUCCESS
        {
            return map;
        }
        paths.truncate(path_count as usize);

        for path in &paths {
            let mut source: DISPLAYCONFIG_SOURCE_DEVICE_NAME = std::mem::zeroed();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != ERROR_SUCCESS as i32 {
                continue;
            }

            let mut target: DISPLAYCONFIG_TARGET_DEVICE_NAME = std::mem::zeroed();
            target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            target.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            target.header.adapterId = path.targetInfo.adapterId;
            target.header.id = path.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut target.header) != ERROR_SUCCESS as i32 {
                continue;
            }

            let device = wide_to_string(&source.viewGdiDeviceName);
            let mut path_str = wide_to_string(&target.monitorDevicePath);
            if path_str.is_empty() {
                // Some indirect drivers leave the path blank; the EDID ids plus
                // the connector instance still separate them from each other.
                path_str = format!(
                    "edid:{:04X}-{:04X}-{}",
                    target.edidManufactureId, target.edidProductCodeId, target.connectorInstance
                );
            }
            if !device.is_empty() {
                map.insert(device, path_str);
            }
        }
    }
    map
}

/// Order-independent key for a set of monitors. Sorting means the same physical
/// desk always produces the same signature regardless of enumeration order.
pub fn signature_from_ids(ids: &[String]) -> String {
    let mut sorted: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
    sorted.sort_unstable();
    sorted.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_independent_of_monitor_order() {
        let a = vec!["\\\\?\\DISPLAY#BNQ805B#a".to_string(), "mon-b".to_string()];
        let b = vec!["mon-b".to_string(), "\\\\?\\DISPLAY#BNQ805B#a".to_string()];
        assert_eq!(signature_from_ids(&a), signature_from_ids(&b));
    }

    #[test]
    fn signature_distinguishes_different_monitor_sets() {
        // The RDP virtual display must never collide with the desk topology,
        // otherwise a remote session would be replayed over the real layout.
        let desk = vec!["mon-benq".to_string(), "mon-hp".to_string()];
        let rdp = vec!["mon-rdp".to_string()];
        assert_ne!(signature_from_ids(&desk), signature_from_ids(&rdp));
        // A subset is also distinct: one monitor asleep is its own topology.
        let one = vec!["mon-benq".to_string()];
        assert_ne!(signature_from_ids(&desk), signature_from_ids(&one));
    }

    #[test]
    fn wide_to_string_stops_at_the_nul_terminator() {
        let mut buf = [0u16; 32];
        for (i, c) in "\\\\.\\DISPLAY2".encode_utf16().enumerate() {
            buf[i] = c;
        }
        assert_eq!(wide_to_string(&buf), "\\\\.\\DISPLAY2");
    }

    #[test]
    fn wide_to_string_handles_an_unterminated_buffer() {
        let buf: Vec<u16> = "abc".encode_utf16().collect();
        assert_eq!(wide_to_string(&buf), "abc");
    }

    #[test]
    fn stable_ids_are_unique_per_attached_display() {
        // Headless CI has no active paths; an empty map is a valid answer and
        // callers fall back to the GDI device name.
        let map = stable_monitor_ids();
        for (device, id) in &map {
            println!("{device} -> {id}");
            assert!(device.starts_with("\\\\.\\DISPLAY"), "bad device {device}");
            assert!(!id.is_empty());
        }
        let mut ids: Vec<&String> = map.values().collect();
        ids.sort();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "two displays share a stable id");
    }
}
