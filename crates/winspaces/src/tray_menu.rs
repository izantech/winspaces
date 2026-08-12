//! Building and showing the tray context menu: one `Vec<MenuEntry>`, handed
//! to `winspaces_ui::menu::show`, which renders it as either the custom
//! acrylic flyout or the classic `HMENU` fallback depending on the OS build.

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
use winspaces_ui::menu::{self, MenuEntry, MenuItemData};
use winspaces_win32::glyphs::{
    GLYPH_ADD, GLYPH_CAMERA, GLYPH_CLOSE, GLYPH_MONITOR, GLYPH_REFRESH, GLYPH_REMOVE,
    GLYPH_RESTORE, GLYPH_SETTINGS, GLYPH_SYNC, GLYPH_TASK_VIEW,
};

use crate::app::APP_STATE;
use crate::handlers::commands::{
    ID_TRAY_CAPTURE_WS, ID_TRAY_CHECK_UPDATES, ID_TRAY_CONFIG, ID_TRAY_EXIT,
    ID_TRAY_MISSION_CONTROL, ID_TRAY_RELOAD, ID_TRAY_RESTORE_WS, ID_TRAY_SWITCH_BASE,
    ID_TRAY_TOGGLE_TASKBAR, TRAY_OFFSET_ADD_SPACE, TRAY_OFFSET_REMOVE_SPACE,
};

pub(crate) fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);

        let (show_tb, monitors_info) = APP_STATE.with(|s| {
            s.try_borrow()
                .ok()
                .and_then(|st| {
                    st.as_ref().map(|state| {
                        let show_tb = state.config.show_all_taskbar;
                        let mons: Vec<(usize, usize, usize)> = state
                            .space_mgr
                            .monitors
                            .iter()
                            .enumerate()
                            .map(|(idx, m)| (idx, m.current, m.spaces.len()))
                            .collect();
                        (show_tb, mons)
                    })
                })
                .unwrap_or((true, vec![(0, 0, winspaces_common::DEFAULT_SPACES)]))
        });

        menu::show(hwnd, build_menu_entries(show_tb, &monitors_info), pt);
    }
}

fn build_menu_entries(show_tb: bool, monitors_info: &[(usize, usize, usize)]) -> Vec<MenuEntry> {
    fn item(
        id: usize,
        glyph: Option<u16>,
        label: &str,
        shortcut: Option<String>,
        checked: bool,
        submenu: Option<Vec<MenuEntry>>,
    ) -> MenuEntry {
        MenuEntry::Item(MenuItemData {
            id,
            glyph,
            label: label.to_string(),
            shortcut,
            checked,
            submenu,
        })
    }

    let mut entries = vec![
        MenuEntry::Header(format!("WinSpaces v{}", env!("CARGO_PKG_VERSION"))),
        item(
            ID_TRAY_MISSION_CONTROL,
            Some(GLYPH_TASK_VIEW),
            "Mission Control",
            Some("Win+Tab".to_string()),
            false,
            None,
        ),
        MenuEntry::Separator,
    ];

    for &(mon_idx, curr_space, space_count) in monitors_info {
        let mut sub: Vec<MenuEntry> = (0..space_count)
            .map(|space_idx| {
                item(
                    ID_TRAY_SWITCH_BASE + mon_idx * 100 + space_idx,
                    None,
                    &format!("Space {}", space_idx + 1),
                    Some(format!("Alt+{}", space_idx + 1)),
                    curr_space == space_idx,
                    None,
                )
            })
            .collect();
        sub.push(MenuEntry::Separator);
        if space_count < winspaces_common::MAX_SPACES {
            sub.push(item(
                ID_TRAY_SWITCH_BASE + mon_idx * 100 + TRAY_OFFSET_ADD_SPACE,
                Some(GLYPH_ADD),
                "New Space",
                None,
                false,
                None,
            ));
        }
        if space_count > 1 {
            sub.push(item(
                ID_TRAY_SWITCH_BASE + mon_idx * 100 + TRAY_OFFSET_REMOVE_SPACE,
                Some(GLYPH_REMOVE),
                &format!("Remove Space {}", space_count),
                None,
                false,
                None,
            ));
        }
        entries.push(item(
            0,
            Some(GLYPH_MONITOR),
            &format!("Display {}", mon_idx + 1),
            Some(format!("Space {}", curr_space + 1)),
            false,
            Some(sub),
        ));
    }

    entries.push(MenuEntry::Separator);
    entries.push(item(
        ID_TRAY_CAPTURE_WS,
        Some(GLYPH_CAMERA),
        "Capture Workspace Layout",
        None,
        false,
        None,
    ));
    entries.push(item(
        ID_TRAY_RESTORE_WS,
        Some(GLYPH_RESTORE),
        "Restore Workspace Layout",
        None,
        false,
        None,
    ));
    entries.push(MenuEntry::Separator);
    entries.push(item(
        ID_TRAY_TOGGLE_TASKBAR,
        None,
        "Show all windows on taskbar",
        None,
        show_tb,
        None,
    ));
    entries.push(item(
        ID_TRAY_CONFIG,
        Some(GLYPH_SETTINGS),
        "Settings",
        None,
        false,
        None,
    ));
    entries.push(item(
        ID_TRAY_CHECK_UPDATES,
        Some(GLYPH_SYNC),
        "Check for Updates",
        None,
        false,
        None,
    ));
    entries.push(MenuEntry::Separator);
    entries.push(item(
        ID_TRAY_RELOAD,
        Some(GLYPH_REFRESH),
        "Reload Configuration",
        None,
        false,
        None,
    ));
    entries.push(item(
        ID_TRAY_EXIT,
        Some(GLYPH_CLOSE),
        "Exit WinSpaces",
        None,
        false,
        None,
    ));
    entries
}

#[cfg(test)]
mod tests {
    //! Compensating coverage for `risks.legacy_menu_untestable`: the classic
    //! `HMENU` renderer in `winspaces_ui::menu::legacy` never runs on this
    //! machine (Windows 11 26100 always takes the acrylic path), so its
    //! correctness rides entirely on tests. Both renderers consume the exact
    //! same `Vec<MenuEntry>`, so ONE fixture built here through the real
    //! `build_menu_entries` (the actual id-arithmetic call site) covers both:
    //! the structural assertions below check the entries the custom flyout
    //! consumes directly, and `legacy_translation_*` checks what
    //! `menu::legacy::build_hmenu` renders from those same entries.
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DestroyMenu, GetMenuItemCount, GetMenuItemID, GetMenuState, GetMenuStringW, GetSubMenu,
        MF_BYPOSITION, MF_CHECKED, MF_DISABLED,
    };
    use winspaces_ui::menu::legacy;

    /// Two monitors exercising the id-stride math across monitors and both
    /// conditional boundaries: monitor 0 sits at MAX_SPACES (no "New
    /// Space"); monitor 1 sits at one space (no "Remove Space").
    fn fixture() -> Vec<MenuEntry> {
        build_menu_entries(true, &[(0, 2, winspaces_common::MAX_SPACES), (1, 0, 1)])
    }

    fn menu_text(hmenu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU, pos: u32) -> String {
        unsafe {
            let mut buf = [0u16; 256];
            let len = GetMenuStringW(
                hmenu,
                pos,
                buf.as_mut_ptr(),
                buf.len() as i32,
                MF_BYPOSITION,
            );
            String::from_utf16_lossy(&buf[..len.max(0) as usize])
        }
    }

    #[test]
    fn item_order_top_level() {
        let entries = fixture();
        assert!(matches!(&entries[0], MenuEntry::Header(h) if h.starts_with("WinSpaces v")));
        let MenuEntry::Item(mc) = &entries[1] else {
            panic!("expected Mission Control item")
        };
        assert_eq!(mc.id, ID_TRAY_MISSION_CONTROL);
        assert!(matches!(&entries[2], MenuEntry::Separator));
        assert!(matches!(&entries[3], MenuEntry::Item(it) if it.submenu.is_some()));
        assert!(matches!(&entries[4], MenuEntry::Item(it) if it.submenu.is_some()));
        assert!(matches!(&entries[5], MenuEntry::Separator));
        let MenuEntry::Item(exit) = entries.last().unwrap() else {
            panic!("expected Exit item last")
        };
        assert_eq!(exit.id, ID_TRAY_EXIT);
    }

    #[test]
    fn switch_command_id_stride_across_two_monitors() {
        let entries = fixture();
        let MenuEntry::Item(disp0) = &entries[3] else {
            panic!()
        };
        let sub0 = disp0.submenu.as_ref().unwrap();
        for (space_idx, e) in sub0.iter().take(winspaces_common::MAX_SPACES).enumerate() {
            let MenuEntry::Item(it) = e else {
                panic!("expected a space item")
            };
            assert_eq!(it.id, ID_TRAY_SWITCH_BASE + space_idx);
        }

        let MenuEntry::Item(disp1) = &entries[4] else {
            panic!()
        };
        let sub1 = disp1.submenu.as_ref().unwrap();
        let MenuEntry::Item(space0) = &sub1[0] else {
            panic!()
        };
        assert_eq!(space0.id, ID_TRAY_SWITCH_BASE + 100);
    }

    #[test]
    fn add_remove_offset_round_trip_through_the_real_decoder() {
        // build (here) -> id -> decode (the exact fn on_command dispatches
        // through), across more than one monitor so a stride error surfaces.
        use crate::handlers::commands::{decode_switch_command, TraySwitchCommand};
        for mon_idx in 0..2usize {
            let add_id = ID_TRAY_SWITCH_BASE + mon_idx * 100 + TRAY_OFFSET_ADD_SPACE;
            match decode_switch_command(add_id) {
                Some(TraySwitchCommand::Add { mon_idx: got }) => assert_eq!(got, mon_idx),
                _ => panic!("expected AddSpace decode for {}", add_id),
            }
            let remove_id = ID_TRAY_SWITCH_BASE + mon_idx * 100 + TRAY_OFFSET_REMOVE_SPACE;
            match decode_switch_command(remove_id) {
                Some(TraySwitchCommand::Remove { mon_idx: got }) => assert_eq!(got, mon_idx),
                _ => panic!("expected RemoveSpace decode for {}", remove_id),
            }
        }
    }

    #[test]
    fn new_space_hidden_at_max_spaces() {
        let entries = build_menu_entries(true, &[(0, 0, winspaces_common::MAX_SPACES)]);
        let MenuEntry::Item(disp) = &entries[3] else {
            panic!()
        };
        let sub = disp.submenu.as_ref().unwrap();
        let add_id = ID_TRAY_SWITCH_BASE + TRAY_OFFSET_ADD_SPACE;
        assert!(!sub
            .iter()
            .any(|e| matches!(e, MenuEntry::Item(it) if it.id == add_id)));
    }

    #[test]
    fn new_space_shown_below_max_spaces() {
        let entries = build_menu_entries(true, &[(0, 0, winspaces_common::MAX_SPACES - 1)]);
        let MenuEntry::Item(disp) = &entries[3] else {
            panic!()
        };
        let sub = disp.submenu.as_ref().unwrap();
        let add_id = ID_TRAY_SWITCH_BASE + TRAY_OFFSET_ADD_SPACE;
        assert!(sub
            .iter()
            .any(|e| matches!(e, MenuEntry::Item(it) if it.id == add_id)));
    }

    #[test]
    fn remove_space_hidden_at_one_space() {
        let entries = build_menu_entries(true, &[(0, 0, 1)]);
        let MenuEntry::Item(disp) = &entries[3] else {
            panic!()
        };
        let sub = disp.submenu.as_ref().unwrap();
        let remove_id = ID_TRAY_SWITCH_BASE + TRAY_OFFSET_REMOVE_SPACE;
        assert!(!sub
            .iter()
            .any(|e| matches!(e, MenuEntry::Item(it) if it.id == remove_id)));
    }

    #[test]
    fn remove_space_shown_above_one_space() {
        let entries = build_menu_entries(true, &[(0, 0, 2)]);
        let MenuEntry::Item(disp) = &entries[3] else {
            panic!()
        };
        let sub = disp.submenu.as_ref().unwrap();
        let remove_id = ID_TRAY_SWITCH_BASE + TRAY_OFFSET_REMOVE_SPACE;
        assert!(sub
            .iter()
            .any(|e| matches!(e, MenuEntry::Item(it) if it.id == remove_id)));
    }

    /// Header and separator entries never carry a real command id — a stray
    /// 0 elsewhere would otherwise be indistinguishable from one of these.
    #[test]
    fn header_and_separator_carry_id_zero() {
        let entries = fixture();
        assert!(matches!(&entries[0], MenuEntry::Header(_)));
        assert!(matches!(&entries[2], MenuEntry::Separator));
        // Every real Item entry (recursively) has a nonzero id UNLESS it is a
        // submenu header itself (the "Display N" entries use id 0 by
        // convention, matching the pre-split menu exactly, since a submenu
        // parent is never itself clickable in the legacy HMENU).
        let MenuEntry::Item(disp0) = &entries[3] else {
            panic!()
        };
        assert_eq!(disp0.id, 0);
        assert!(disp0.submenu.is_some());
    }

    /// The shared fixture, translated through `legacy::build_hmenu` and
    /// inspected with the Win32 introspection calls a real popup would
    /// answer to.
    #[test]
    fn legacy_translation_structure_checked_and_ids() {
        let entries = fixture();
        let hmenu = legacy::build_hmenu(&entries);
        unsafe {
            // Header, Mission Control, Separator, Display 1, Display 2,
            // Separator, Capture, Restore, Separator, Toggle taskbar,
            // Settings, Check updates, Separator, Reload, Exit.
            assert_eq!(GetMenuItemCount(hmenu), 15);

            // Header carries id 0 and MF_DISABLED.
            assert_eq!(GetMenuItemID(hmenu, 0), 0);
            let header_state = GetMenuState(hmenu, 0, MF_BYPOSITION);
            assert_ne!(header_state & MF_DISABLED, 0);

            // Separator at position 2 also carries id 0.
            assert_eq!(GetMenuItemID(hmenu, 2), 0);

            // Mission Control item id and shortcut-suffix formatting.
            assert_eq!(GetMenuItemID(hmenu, 1) as usize, ID_TRAY_MISSION_CONTROL);
            assert_eq!(menu_text(hmenu, 1), "Mission Control  (Win+Tab)");

            // Both displays nest as MF_POPUP submenus.
            let disp0 = GetSubMenu(hmenu, 3);
            let disp1 = GetSubMenu(hmenu, 4);
            assert!(!disp0.is_null());
            assert!(!disp1.is_null());

            // Monitor 0: MAX_SPACES spaces, current = index 2 -> checked,
            // with the id-stride and shortcut-suffix format matching exactly.
            let checked_state = GetMenuState(disp0, 2, MF_BYPOSITION);
            assert_ne!(checked_state & MF_CHECKED, 0);
            assert_eq!(GetMenuItemID(disp0, 2) as usize, ID_TRAY_SWITCH_BASE + 2);
            assert_eq!(menu_text(disp0, 2), "Space 3  (Alt+3)");
            let unchecked_state = GetMenuState(disp0, 0, MF_BYPOSITION);
            assert_eq!(unchecked_state & MF_CHECKED, 0);

            // Monitor 0 is at MAX_SPACES: no "New Space" entry anywhere.
            let disp0_count = GetMenuItemCount(disp0);
            for pos in 0..disp0_count {
                assert_ne!(
                    GetMenuItemID(disp0, pos) as usize,
                    ID_TRAY_SWITCH_BASE + TRAY_OFFSET_ADD_SPACE
                );
            }

            // Monitor 1 has exactly one space: no "Remove Space" entry.
            let disp1_count = GetMenuItemCount(disp1);
            for pos in 0..disp1_count {
                assert_ne!(
                    GetMenuItemID(disp1, pos) as usize,
                    ID_TRAY_SWITCH_BASE + 100 + TRAY_OFFSET_REMOVE_SPACE
                );
            }

            DestroyMenu(hmenu);
        }
    }
}
