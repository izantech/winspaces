use crate::config::{Config, Hotkey, NUM_DESKTOPS};
use crate::tray::encode_wide;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SetFocus, VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
    GetWindowLongPtrW, MessageBoxW, RegisterClassW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, BS_PUSHBUTTON, CREATESTRUCTW, GWLP_USERDATA,
    GWL_EXSTYLE, GWL_STYLE, HMENU, MB_ICONEXCLAMATION, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    SW_SHOWNORMAL, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_EX_TOOLWINDOW, WS_MINIMIZEBOX,
    WS_OVERLAPPED, WS_SYSMENU, WS_THICKFRAME, WS_VISIBLE,
};

pub const IDC_SWITCH_BASE: i32 = 200;
pub const IDC_MOVE_BASE: i32 = IDC_SWITCH_BASE + NUM_DESKTOPS as i32;
pub const IDC_PREV: i32 = 280;
pub const IDC_NEXT: i32 = 281;
pub const IDC_MOVE_PREV: i32 = 282;
pub const IDC_MOVE_NEXT: i32 = 283;
pub const IDC_APPLY: i32 = 300;
pub const IDC_CLOSE: i32 = 301;

pub struct ConfigWindow {
    pub hwnd: HWND,
}

struct GuiState {
    config: Config,
    pending_config: Config,
    capturing_id: Option<i32>,
    switch_buttons: [HWND; NUM_DESKTOPS],
    move_buttons: [HWND; NUM_DESKTOPS],
    button_prev: HWND,
    button_next: HWND,
    button_move_prev: HWND,
    button_move_next: HWND,
}

impl ConfigWindow {
    pub fn show(config: &Config) -> Self {
        let class_name = encode_wide("WinSpacesConfigWindow");
        let title = encode_wide("WinSpaces Hotkeys");

        unsafe {
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(config_wndproc),
                cbClsExtra: 0,
                cbWndExtra: std::mem::size_of::<*mut GuiState>() as i32,
                hInstance: windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null_mut()),
                hIcon: null_mut(),
                hCursor: null_mut(),
                hbrBackground: 6 as _, // COLOR_WINDOW
                lpszMenuName: std::ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            RegisterClassW(&wc);

            let state = Box::into_raw(Box::new(GuiState {
                config: config.clone(),
                pending_config: config.clone(),
                capturing_id: None,
                switch_buttons: [null_mut(); NUM_DESKTOPS],
                move_buttons: [null_mut(); NUM_DESKTOPS],
                button_prev: null_mut(),
                button_next: null_mut(),
                button_move_prev: null_mut(),
                button_move_next: null_mut(),
            }));

            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX,
                windows_sys::Win32::UI::WindowsAndMessaging::CW_USEDEFAULT,
                windows_sys::Win32::UI::WindowsAndMessaging::CW_USEDEFAULT,
                420,
                420,
                null_mut(),
                null_mut(),
                wc.hInstance,
                state as *const _,
            );

            ShowWindow(hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(hwnd);

            Self { hwnd }
        }
    }
}

extern "system" fn config_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiState;

        match msg {
            windows_sys::Win32::UI::WindowsAndMessaging::WM_CREATE => {
                let cs = lparam as *const CREATESTRUCTW;
                let state_ptr = (*cs).lpCreateParams as *mut GuiState;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as _);
                let state = &mut *state_ptr;

                create_label(hwnd, "Click a button and press the new hotkey combination.", 10, 10, 380, 18);
                create_label(hwnd, "Switch desktop", 120, 40, 120, 18);
                create_label(hwnd, "Move window", 260, 40, 120, 18);

                for i in 0..NUM_DESKTOPS {
                    let y = 70 + (i as i32) * 40;
                    create_label(hwnd, &format!("Desktop {}", i + 1), 10, y + 6, 90, 18);

                    state.switch_buttons[i] = create_button(hwnd, "", 120, y, 130, 26, IDC_SWITCH_BASE + i as i32);
                    state.move_buttons[i] = create_button(hwnd, "", 260, y, 130, 26, IDC_MOVE_BASE + i as i32);
                }

                let mut y = 70 + (NUM_DESKTOPS as i32) * 40;
                create_label(hwnd, "Previous desktop", 10, y + 6, 130, 18);
                state.button_prev = create_button(hwnd, "", 120, y, 130, 26, IDC_PREV);
                state.button_move_prev = create_button(hwnd, "", 260, y, 130, 26, IDC_MOVE_PREV);

                y += 40;
                create_label(hwnd, "Next desktop", 10, y + 6, 130, 18);
                state.button_next = create_button(hwnd, "", 120, y, 130, 26, IDC_NEXT);
                state.button_move_next = create_button(hwnd, "", 260, y, 130, 26, IDC_MOVE_NEXT);

                y += 40;
                create_button(hwnd, "Apply", 180, y, 90, 28, IDC_APPLY);
                create_button(hwnd, "Close", 280, y, 90, 28, IDC_CLOSE);

                let content_bottom = y + 28 + 10;
                let mut client = std::mem::zeroed::<RECT>();
                if GetClientRect(hwnd, &mut client) != 0 && content_bottom > client.bottom {
                    let mut desired = RECT { left: 0, top: 0, right: client.right, bottom: content_bottom };
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                    let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                    AdjustWindowRectEx(&mut desired, style, 0, exstyle);
                    SetWindowPos(
                        hwnd, null_mut(), 0, 0,
                        desired.right - desired.left,
                        desired.bottom - desired.top,
                        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }

                update_all_button_texts(state);
                0
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_COMMAND => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    let id = (wparam & 0xffff) as i32;

                    if id == IDC_CLOSE {
                        DestroyWindow(hwnd);
                    } else if id == IDC_APPLY {
                        cancel_capture(state);
                        let new_cfg = state.pending_config.clone();
                        let applied = crate::apply_config(new_cfg);
                        if !applied {
                            state.pending_config = crate::get_config();
                        }
                        state.config = state.pending_config.clone();
                        update_all_button_texts(state);
                    } else if (id >= IDC_SWITCH_BASE && id < IDC_SWITCH_BASE + NUM_DESKTOPS as i32)
                        || (id >= IDC_MOVE_BASE && id < IDC_MOVE_BASE + NUM_DESKTOPS as i32)
                        || id == IDC_PREV || id == IDC_NEXT || id == IDC_MOVE_PREV || id == IDC_MOVE_NEXT {
                        begin_capture(state, hwnd, id);
                    }
                }
                0
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_KEYDOWN
            | windows_sys::Win32::UI::WindowsAndMessaging::WM_SYSKEYDOWN => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    if state.capturing_id.is_some() {
                        let vk = wparam as u32;
                        let modifiers = get_current_modifiers();

                        if vk == 0x1b && modifiers == 0 {
                            cancel_capture(state);
                            return 0;
                        }
                        if vk == VK_CONTROL as u32
                            || vk == VK_MENU as u32
                            || vk == VK_SHIFT as u32
                            || vk == VK_LWIN as u32
                            || vk == VK_RWIN as u32
                        {
                            return 0;
                        }
                        if modifiers == 0 {
                            MessageBoxW(
                                hwnd,
                                encode_wide(
                                    "Please include at least one modifier (Ctrl, Alt, Shift, Win).",
                                )
                                .as_ptr(),
                                encode_wide("WinSpaces").as_ptr(),
                                MB_ICONEXCLAMATION,
                            );
                            return 0;
                        }
                        if let Some(capturing_id) = state.capturing_id {
                            let hk = Hotkey { modifiers, vk };
                            assign_pending_hotkey(state, capturing_id, hk);
                            cancel_capture(state);
                            update_all_button_texts(state);
                        }
                    }
                }
                0
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_KEYUP
            | windows_sys::Win32::UI::WindowsAndMessaging::WM_SYSKEYUP
            | windows_sys::Win32::UI::WindowsAndMessaging::WM_CHAR
            | windows_sys::Win32::UI::WindowsAndMessaging::WM_SYSCHAR => {
                if !state_ptr.is_null() && (*state_ptr).capturing_id.is_some() {
                    return 0;
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_KILLFOCUS => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    cancel_capture(state);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            windows_sys::Win32::UI::WindowsAndMessaging::WM_DESTROY => {
                if !state_ptr.is_null() {
                    let _ = Box::from_raw(state_ptr);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn create_label(hwnd: HWND, text: &str, x: i32, y: i32, w: i32, h: i32) -> HWND {
    unsafe {
        CreateWindowExW(
            0, encode_wide("STATIC").as_ptr(), encode_wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE, x, y, w, h, hwnd, null_mut(), null_mut(), std::ptr::null(),
        )
    }
}

fn create_button(hwnd: HWND, text: &str, x: i32, y: i32, w: i32, h: i32, id: i32) -> HWND {
    unsafe {
        CreateWindowExW(
            0, encode_wide("BUTTON").as_ptr(), encode_wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON as u32, x, y, w, h, hwnd, id as HMENU, null_mut(), std::ptr::null(),
        )
    }
}

fn get_button_by_id(state: &GuiState, id: i32) -> Option<HWND> {
    if id >= IDC_SWITCH_BASE && id < IDC_SWITCH_BASE + NUM_DESKTOPS as i32 {
        Some(state.switch_buttons[(id - IDC_SWITCH_BASE) as usize])
    } else if id >= IDC_MOVE_BASE && id < IDC_MOVE_BASE + NUM_DESKTOPS as i32 {
        Some(state.move_buttons[(id - IDC_MOVE_BASE) as usize])
    } else if id == IDC_PREV {
        Some(state.button_prev)
    } else if id == IDC_NEXT {
        Some(state.button_next)
    } else if id == IDC_MOVE_PREV {
        Some(state.button_move_prev)
    } else if id == IDC_MOVE_NEXT {
        Some(state.button_move_next)
    } else {
        None
    }
}

fn update_all_button_texts(state: &GuiState) {
    unsafe {
        for i in 0..NUM_DESKTOPS {
            SetWindowTextW(state.switch_buttons[i], encode_wide(&hotkey_to_string(&state.pending_config.switch_desktops[i])).as_ptr());
            SetWindowTextW(state.move_buttons[i], encode_wide(&hotkey_to_string(&state.pending_config.move_desktops[i])).as_ptr());
        }
        SetWindowTextW(state.button_prev, encode_wide(&hotkey_to_string(&state.pending_config.prev)).as_ptr());
        SetWindowTextW(state.button_next, encode_wide(&hotkey_to_string(&state.pending_config.next)).as_ptr());
        SetWindowTextW(state.button_move_prev, encode_wide(&hotkey_to_string(&state.pending_config.move_prev)).as_ptr());
        SetWindowTextW(state.button_move_next, encode_wide(&hotkey_to_string(&state.pending_config.move_next)).as_ptr());
    }
}

fn assign_pending_hotkey(state: &mut GuiState, id: i32, hk: Hotkey) {
    if id >= IDC_SWITCH_BASE && id < IDC_SWITCH_BASE + NUM_DESKTOPS as i32 {
        state.pending_config.switch_desktops[(id - IDC_SWITCH_BASE) as usize] = hk;
    } else if id >= IDC_MOVE_BASE && id < IDC_MOVE_BASE + NUM_DESKTOPS as i32 {
        state.pending_config.move_desktops[(id - IDC_MOVE_BASE) as usize] = hk;
    } else if id == IDC_PREV {
        state.pending_config.prev = hk;
    } else if id == IDC_NEXT {
        state.pending_config.next = hk;
    } else if id == IDC_MOVE_PREV {
        state.pending_config.move_prev = hk;
    } else if id == IDC_MOVE_NEXT {
        state.pending_config.move_next = hk;
    }
}

fn get_current_modifiers() -> u32 {
    let mut mods = 0;
    unsafe {
        if (GetAsyncKeyState(VK_CONTROL as i32) as u16) & 0x8000 != 0 {
            mods |= 0x0002;
        }
        if (GetAsyncKeyState(VK_MENU as i32) as u16) & 0x8000 != 0 {
            mods |= 0x0001;
        }
        if (GetAsyncKeyState(VK_SHIFT as i32) as u16) & 0x8000 != 0 {
            mods |= 0x0004;
        }
        if (GetAsyncKeyState(VK_LWIN as i32) as u16) & 0x8000 != 0
            || (GetAsyncKeyState(VK_RWIN as i32) as u16) & 0x8000 != 0
        {
            mods |= 0x0008;
        }
    }
    mods
}

fn begin_capture(state: &mut GuiState, hwnd: HWND, id: i32) {
    cancel_capture(state);
    state.capturing_id = Some(id);
    if let Some(btn) = get_button_by_id(state, id) {
        unsafe {
            SetWindowTextW(btn, encode_wide("Press keys...").as_ptr());
            SetFocus(hwnd);
        }
    }
}

fn cancel_capture(state: &mut GuiState) {
    if state.capturing_id.is_none() {
        return;
    }
    state.capturing_id = None;
    update_all_button_texts(state);
}

pub fn hotkey_to_string(hk: &Hotkey) -> String {
    if hk.vk == 0 {
        return "Unassigned".to_string();
    }
    let mut parts = Vec::new();
    if (hk.modifiers & 0x0002) != 0 {
        parts.push("Ctrl");
    }
    if (hk.modifiers & 0x0001) != 0 {
        parts.push("Alt");
    }
    if (hk.modifiers & 0x0004) != 0 {
        parts.push("Shift");
    }
    if (hk.modifiers & 0x0008) != 0 {
        parts.push("Win");
    }

    let vk = hk.vk;
    let vk_str: String = if (0x41..=0x5A).contains(&vk) {
        char::from_u32(vk).map(|c| c.to_string()).unwrap_or_default()
    } else if (0x30..=0x39).contains(&vk) {
        char::from_u32(vk).map(|c| c.to_string()).unwrap_or_default()
    } else if (0x70..=0x87).contains(&vk) {
        format!("F{}", vk - 0x70 + 1)
    } else {
        match vk {
            0x09 => "Tab".to_string(),
            0x1B => "Esc".to_string(),
            0x20 => "Space".to_string(),
            0x0D => "Enter".to_string(),
            0x08 => "Backspace".to_string(),
            0x2E => "Delete".to_string(),
            0x24 => "Home".to_string(),
            0x23 => "End".to_string(),
            0x21 => "PageUp".to_string(),
            0x22 => "PageDown".to_string(),
            0x25 => "Left".to_string(),
            0x27 => "Right".to_string(),
            0x26 => "Up".to_string(),
            0x28 => "Down".to_string(),
            _ => format!("VK{}", vk),
        }
    };
    parts.push(&vk_str);
    parts.join("+")
}
