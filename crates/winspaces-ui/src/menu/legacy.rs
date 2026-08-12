//! Classic `HMENU` tray menu for pre-Windows 11 builds (`win_build() <
//! 22000`), where the custom acrylic flyout in [`super`] has no DWM backdrop
//! to draw against. Renders the exact same [`MenuEntry`] tree the flyout
//! consumes, so the two renderers can never drift into different menu
//! structures — only the chrome differs.
//!
//! This path never executes on this machine (Windows 11 26100 always takes
//! the custom flyout), so it ships without manual verification. The
//! compensating coverage is this module's unit tests plus
//! `winspaces::tray_menu`'s, which both build the SAME `Vec<MenuEntry>`
//! fixture and check it against both this module's `HMENU` tree and the
//! custom renderer's own data structure.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, PostMessageW, SetForegroundWindow, TrackPopupMenu,
    HMENU, MF_CHECKED, MF_DISABLED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED,
    TPM_RIGHTBUTTON, WM_NULL,
};
use winspaces_win32::text::encode_wide;

use super::MenuEntry;

/// Show the classic system context menu built from `entries`.
///
/// Documented tray-menu quirk (KB135788): without the posted no-op after
/// `TrackPopupMenu` returns, the menu won't dismiss on the first click
/// outside it.
///
/// `owner` is an opaque Win32 handle, never dereferenced in Rust — it is only
/// ever forwarded to Win32 APIs, so this stays a safe fn despite carrying a
/// raw-pointer-typed parameter (matching `menu::show_custom`'s convention).
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn show(owner: HWND, entries: &[MenuEntry], anchor: POINT) {
    unsafe {
        let hmenu = build_hmenu(entries);
        SetForegroundWindow(owner);
        TrackPopupMenu(
            hmenu,
            TPM_RIGHTBUTTON,
            anchor.x,
            anchor.y,
            0,
            owner,
            std::ptr::null(),
        );
        PostMessageW(owner, WM_NULL, 0, 0);
        DestroyMenu(hmenu);
    }
}

/// Translate `entries` into a classic `HMENU` tree. Pure with respect to the
/// screen — no `TrackPopupMenu`, no window shown — which is what makes it the
/// part unit tests exercise directly. The caller owns the returned handle and
/// must `DestroyMenu` it (destroying a popup menu recursively destroys the
/// submenus popped into it).
pub fn build_hmenu(entries: &[MenuEntry]) -> HMENU {
    unsafe {
        let hmenu = CreatePopupMenu();
        for entry in entries {
            append_entry(hmenu, entry);
        }
        hmenu
    }
}

unsafe fn append_entry(hmenu: HMENU, entry: &MenuEntry) {
    match entry {
        MenuEntry::Header(text) => {
            AppendMenuW(
                hmenu,
                MF_DISABLED | MF_STRING,
                0,
                encode_wide(text).as_ptr(),
            );
        }
        MenuEntry::Separator => {
            AppendMenuW(hmenu, MF_SEPARATOR, 0, std::ptr::null());
        }
        MenuEntry::Item(it) => {
            let label = labeled(&it.label, it.shortcut.as_deref());
            if let Some(sub) = &it.submenu {
                let hsub = build_hmenu(sub);
                AppendMenuW(hmenu, MF_POPUP, hsub as usize, encode_wide(&label).as_ptr());
            } else {
                let flags = MF_STRING | if it.checked { MF_CHECKED } else { MF_UNCHECKED };
                AppendMenuW(hmenu, flags, it.id, encode_wide(&label).as_ptr());
            }
        }
    }
}

/// Append a shortcut suffix exactly like the pre-split legacy builder did:
/// two spaces, then the shortcut text in parentheses.
fn labeled(label: &str, shortcut: Option<&str>) -> String {
    match shortcut {
        Some(s) => format!("{}  ({})", label, s),
        None => label.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::MenuItemData;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMenuItemCount, GetMenuItemID, GetMenuState, GetMenuStringW, GetSubMenu, MF_BYPOSITION,
    };

    fn item(
        id: usize,
        label: &str,
        shortcut: Option<&str>,
        checked: bool,
        submenu: Option<Vec<MenuEntry>>,
    ) -> MenuEntry {
        MenuEntry::Item(MenuItemData {
            id,
            glyph: None,
            label: label.to_string(),
            shortcut: shortcut.map(str::to_string),
            checked,
            submenu,
        })
    }

    fn menu_text(hmenu: HMENU, pos: u32) -> String {
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

    /// A small self-contained fixture (this module cannot reach into the bin
    /// crate's real `tray_menu::build_menu_entries` — that would be an
    /// upward dependency): a header, a plain item, a checked item and a
    /// nested submenu, covering every `MenuEntry` variant this translator
    /// handles.
    fn fixture() -> Vec<MenuEntry> {
        vec![
            MenuEntry::Header("WinSpaces v0.1.0".to_string()),
            item(999, "Mission Control", Some("Win+Tab"), false, None),
            MenuEntry::Separator,
            item(
                0,
                "Display 1",
                None,
                false,
                Some(vec![
                    item(2000, "Space 1", Some("Alt+1"), true, None),
                    item(2001, "Space 2", Some("Alt+2"), false, None),
                    MenuEntry::Separator,
                    item(2099, "Remove Space 2", None, false, None),
                ]),
            ),
            item(1002, "Settings", None, false, None),
        ]
    }

    #[test]
    fn item_order_preserved() {
        let entries = fixture();
        let hmenu = build_hmenu(&entries);
        unsafe {
            assert_eq!(GetMenuItemCount(hmenu), 5);
            assert_eq!(menu_text(hmenu, 0), "WinSpaces v0.1.0");
            assert_eq!(menu_text(hmenu, 1), "Mission Control  (Win+Tab)");
            assert_eq!(menu_text(hmenu, 3), "Display 1");
            assert_eq!(menu_text(hmenu, 4), "Settings");
            DestroyMenu(hmenu);
        }
    }

    #[test]
    fn header_and_separator_carry_id_zero() {
        let entries = fixture();
        let hmenu = build_hmenu(&entries);
        unsafe {
            // position 0 = Header, position 2 = Separator.
            assert_eq!(GetMenuItemID(hmenu, 0), 0);
            assert_eq!(GetMenuItemID(hmenu, 2), 0);
            let header_state = GetMenuState(hmenu, 0, MF_BYPOSITION);
            assert_ne!(header_state & MF_DISABLED, 0);
            DestroyMenu(hmenu);
        }
    }

    #[test]
    fn submenu_nests_as_popup() {
        let entries = fixture();
        let hmenu = build_hmenu(&entries);
        unsafe {
            let sub = GetSubMenu(hmenu, 3);
            assert!(!sub.is_null());
            assert_eq!(GetMenuItemCount(sub), 4);
            assert_eq!(GetMenuItemID(sub, 0) as usize, 2000);
            assert_eq!(GetMenuItemID(sub, 1) as usize, 2001);
            assert_eq!(GetMenuItemID(sub, 2), 0); // the submenu's separator
            assert_eq!(GetMenuItemID(sub, 3) as usize, 2099);
            DestroyMenu(hmenu);
        }
    }

    #[test]
    fn checked_item_is_checked_only_the_checked_one() {
        let entries = fixture();
        let hmenu = build_hmenu(&entries);
        unsafe {
            let sub = GetSubMenu(hmenu, 3);
            let space1_state = GetMenuState(sub, 0, MF_BYPOSITION);
            let space2_state = GetMenuState(sub, 1, MF_BYPOSITION);
            assert_ne!(space1_state & MF_CHECKED, 0);
            assert_eq!(space2_state & MF_CHECKED, 0);
            DestroyMenu(hmenu);
        }
    }

    #[test]
    fn shortcut_suffix_format_matches_original() {
        let entries = fixture();
        let hmenu = build_hmenu(&entries);
        unsafe {
            let sub = GetSubMenu(hmenu, 3);
            // Two spaces then a parenthesized shortcut; an item with no
            // shortcut carries its bare label.
            assert_eq!(menu_text(sub, 0), "Space 1  (Alt+1)");
            assert_eq!(menu_text(sub, 3), "Remove Space 2");
            DestroyMenu(hmenu);
        }
    }
}
