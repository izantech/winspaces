//! Mouse/keyboard interaction, submenu open/close lifecycle, and the
//! hover-delay timers that drive it.

use super::layout::{hit_test, hit_test_window, step_selection, Hit};
use super::{
    close_menu, create_menu_window, MenuEntry, MenuItemData, MenuState, MenuWindow, MENU_STATE,
    PAD_V, TIMER_CLOSE_SUB, TIMER_OPEN_SUB,
};
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetCursorPos, KillTimer, PostMessageW, SetTimer, ShowWindow,
    SystemParametersInfoW, SPI_GETMENUSHOWDELAY, SW_SHOWNOACTIVATE, WM_COMMAND,
};
use winspaces_win32::dpi::px;

fn submenu_show_delay() -> u32 {
    let mut delay: u32 = 400;
    unsafe {
        SystemParametersInfoW(SPI_GETMENUSHOWDELAY, 0, &mut delay as *mut _ as _, 0);
    }
    delay.clamp(50, 1000)
}

unsafe fn cursor_pos() -> POINT {
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    pt
}

pub(crate) unsafe fn on_mouse_move() {
    let pt = cursor_pos();
    MENU_STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        let hit = hit_test(state, pt);

        let new_root_hover = if let Hit::Root(i) = hit {
            Some(i)
        } else {
            None
        };
        let new_sub_hover = if let Hit::Sub(i) = hit { Some(i) } else { None };

        if new_root_hover != state.root.hover {
            state.root.hover = new_root_hover;
            state.root.sel = None;
            InvalidateRect(state.root.hwnd, std::ptr::null(), 0);
        }
        if let Some(sub) = state.sub.as_mut() {
            if new_sub_hover != sub.hover {
                sub.hover = new_sub_hover;
                sub.sel = None;
                InvalidateRect(sub.hwnd, std::ptr::null(), 0);
            }
        }

        // Submenu open/close timers follow the system hover delay.
        match hit {
            Hit::Root(i) => {
                let has_sub =
                    matches!(&state.root.entries[i], MenuEntry::Item(it) if it.submenu.is_some());
                if has_sub {
                    if state.sub_parent == Some(i) {
                        KillTimer(state.root.hwnd, TIMER_CLOSE_SUB);
                    } else if state.pending_sub != Some(i) {
                        state.pending_sub = Some(i);
                        SetTimer(state.root.hwnd, TIMER_OPEN_SUB, submenu_show_delay(), None);
                    }
                } else {
                    state.pending_sub = None;
                    KillTimer(state.root.hwnd, TIMER_OPEN_SUB);
                    if state.sub.is_some() {
                        SetTimer(state.root.hwnd, TIMER_CLOSE_SUB, submenu_show_delay(), None);
                    }
                }
            }
            Hit::Sub(_) | Hit::SubBlank => {
                state.pending_sub = None;
                KillTimer(state.root.hwnd, TIMER_OPEN_SUB);
                KillTimer(state.root.hwnd, TIMER_CLOSE_SUB);
            }
            _ => {}
        }
    });
}

pub(crate) unsafe fn on_mouse_down() {
    let pt = cursor_pos();
    let outside = MENU_STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|state| hit_test(state, pt) == Hit::Outside)
            .unwrap_or(false)
    });
    if outside {
        close_menu();
    }
}

pub(crate) unsafe fn on_mouse_up() {
    let pt = cursor_pos();
    let mut command: Option<(HWND, usize)> = None;
    MENU_STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        match hit_test(state, pt) {
            Hit::Root(i) => {
                let has_sub =
                    matches!(&state.root.entries[i], MenuEntry::Item(it) if it.submenu.is_some());
                if has_sub {
                    if state.sub_parent != Some(i) {
                        open_submenu(state, i, false);
                    }
                } else if let MenuEntry::Item(it) = &state.root.entries[i] {
                    command = Some((state.owner, it.id));
                }
            }
            Hit::Sub(i) => {
                if let Some(sub) = &state.sub {
                    if let MenuEntry::Item(it) = &sub.entries[i] {
                        command = Some((state.owner, it.id));
                    }
                }
            }
            _ => {}
        }
    });
    if let Some((owner, id)) = command {
        PostMessageW(owner, WM_COMMAND, id, 0);
        close_menu();
    }
}

unsafe fn open_submenu(state: &mut MenuState, parent_idx: usize, via_keyboard: bool) {
    if let Some(sub) = state.sub.take() {
        DestroyWindow(sub.hwnd);
        state.sub_parent = None;
    }
    let MenuEntry::Item(parent) = &state.root.entries[parent_idx] else {
        return;
    };
    let Some(sub_entries) = &parent.submenu else {
        return;
    };
    // Clone the submenu content into its own window state; submenus are one
    // level deep so the clone is a handful of small structs.
    let entries: Vec<MenuEntry> = sub_entries
        .iter()
        .map(|e| match e {
            MenuEntry::Header(t) => MenuEntry::Header(t.clone()),
            MenuEntry::Separator => MenuEntry::Separator,
            MenuEntry::Item(it) => MenuEntry::Item(MenuItemData {
                id: it.id,
                glyph: it.glyph,
                label: it.label.clone(),
                shortcut: it.shortcut.clone(),
                checked: it.checked,
                submenu: None,
            }),
        })
        .collect();

    let px = |v: i32| px(state.scale, v);
    let (rows, width, height) = super::layout::layout_window(&entries, &state.fonts, state.scale);

    let parent_row = state.root.rows[parent_idx];
    // Slight overlap like native menus; flip to the left edge when the
    // submenu would leave the work area.
    let mut x = state.root.x + state.root.width - px(4);
    if x + width > state.work.right {
        x = (state.root.x - width + px(4)).max(state.work.left);
    }
    let mut y = state.root.y + parent_row.top - px(PAD_V);
    if y + height > state.work.bottom {
        y = (state.work.bottom - height).max(state.work.top);
    }

    let hwnd = create_menu_window(
        state.root.hwnd,
        x,
        y,
        width,
        height,
        state.acrylic,
        state.light,
    );
    if hwnd.is_null() {
        return;
    }
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);

    let sel = if via_keyboard {
        entries.iter().position(|e| e.is_selectable())
    } else {
        None
    };
    state.sub = Some(MenuWindow {
        hwnd,
        x,
        y,
        width,
        height,
        entries,
        rows,
        hover: None,
        sel,
    });
    state.sub_parent = Some(parent_idx);
    state.pending_sub = None;
}

unsafe fn close_submenu(state: &mut MenuState) {
    if let Some(sub) = state.sub.take() {
        DestroyWindow(sub.hwnd);
    }
    state.sub_parent = None;
}

pub(crate) unsafe fn on_timer(id: usize) {
    MENU_STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        KillTimer(state.root.hwnd, id);
        match id {
            TIMER_OPEN_SUB => {
                if let Some(pending) = state.pending_sub.take() {
                    // Only open if the cursor is still on that row.
                    if state.root.hover == Some(pending) {
                        open_submenu(state, pending, false);
                    }
                }
            }
            TIMER_CLOSE_SUB => {
                // Don't close while the cursor sits inside the submenu.
                let pt = cursor_pos();
                let in_sub = state
                    .sub
                    .as_ref()
                    .map(|sub| hit_test_window(sub, pt).is_some())
                    .unwrap_or(false);
                if !in_sub {
                    close_submenu(state);
                }
            }
            _ => {}
        }
    });
}

pub(crate) unsafe fn on_key(vk: u32) {
    let mut command: Option<(HWND, usize)> = None;
    let mut close_all = false;
    MENU_STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        match vk as u16 {
            VK_ESCAPE => {
                if state.sub.is_some() {
                    close_submenu(state);
                    InvalidateRect(state.root.hwnd, std::ptr::null(), 0);
                } else {
                    close_all = true;
                }
            }
            VK_UP | VK_DOWN => {
                let dir = if vk as u16 == VK_DOWN { 1 } else { -1 };
                let win = state.sub.as_mut().unwrap_or(&mut state.root);
                win.sel = step_selection(&win.entries, win.sel, dir);
                win.hover = None;
                InvalidateRect(win.hwnd, std::ptr::null(), 0);
            }
            VK_LEFT => {
                if state.sub.is_some() {
                    close_submenu(state);
                    InvalidateRect(state.root.hwnd, std::ptr::null(), 0);
                }
            }
            VK_RIGHT => {
                if state.sub.is_none() {
                    if let Some(sel) = state.root.sel {
                        let has_sub = matches!(&state.root.entries[sel], MenuEntry::Item(it) if it.submenu.is_some());
                        if has_sub {
                            open_submenu(state, sel, true);
                        }
                    }
                }
            }
            VK_RETURN | VK_SPACE => {
                enum Action {
                    OpenSub(usize),
                    Command(usize),
                }
                let mut action = None;
                {
                    let (win, is_sub) = match state.sub.as_ref() {
                        Some(sub) => (sub, true),
                        None => (&state.root, false),
                    };
                    if let Some(sel) = win.sel {
                        if let MenuEntry::Item(it) = &win.entries[sel] {
                            action = if !is_sub && it.submenu.is_some() {
                                Some(Action::OpenSub(sel))
                            } else {
                                Some(Action::Command(it.id))
                            };
                        }
                    }
                }
                match action {
                    Some(Action::OpenSub(idx)) => open_submenu(state, idx, true),
                    Some(Action::Command(id)) => command = Some((state.owner, id)),
                    None => {}
                }
            }
            _ => {}
        }
    });
    if let Some((owner, id)) = command {
        PostMessageW(owner, WM_COMMAND, id, 0);
        close_all = true;
    }
    if close_all {
        close_menu();
    }
}
