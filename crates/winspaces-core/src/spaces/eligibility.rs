//! Whether a window is one WinSpaces manages. `is_eligible` is pure and
//! unit-tested; `gather_window_facts`/`is_valid_window` do the Win32 probing
//! that feeds it.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId, IsWindow,
    GA_ROOTOWNER, GWL_EXSTYLE, GWL_STYLE, WS_CAPTION, WS_CHILD, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_THICKFRAME, WS_VISIBLE,
};

use super::state::{
    get_window_state, WINSPACES_STATE_CLOAKED, WINSPACES_STATE_HIDDEN_MASK,
    WINSPACES_STATE_SHELL_CLOAKED,
};

/// Facts about a window that the eligibility decision needs, gathered from
/// Win32 by `is_valid_window` so the decision itself (`is_eligible`) stays
/// pure and unit-testable.
pub(crate) struct WindowFacts {
    style: u32,
    ex_style: u32,
    class_name: String,
    has_title: bool,
    can_resize: bool,
    /// DWM-cloaked without our CLOAKED state bit: cloaked by Windows or
    /// another app (suspended UWP, native virtual desktops, ...).
    externally_cloaked: bool,
    /// Our state bits say we hid it (cloak or forced minimize), so a clear
    /// WS_VISIBLE / set cloak must not disqualify it.
    hidden_by_us: bool,
    /// Root-owner facts; None when unowned or when WS_EX_APPWINDOW asks to
    /// be judged standalone.
    owner: Option<Box<WindowFacts>>,
}

/// Shell/system window classes that are never managed. Classes are stable
/// identifiers across locales and Windows builds, unlike window titles.
fn is_shell_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Windows.UI.Core.CoreWindow"
            | "EdgeUiInputTopWndClass"
            | "XamlExplorerHost"
            | "TopLevelWindowForOverflowXamlIsland"
            | "Windows.UI.Composition.DesktopWindowTarget"
            | "TaskListThumbnailWnd"
            | "NativeHWNDHost"
            | "PopupHost"
            | "Xaml_WindowedPopupClass"
            | "IME"
            | "MSCTFIME UI"
            | "tooltips_class32"
            | "SysShadow"
            | "ComboLBox"
            | "#32768"
    )
}

/// Structural noise checks shared by a window and its root owner, modeled on
/// the shell's Alt-Tab rules (visibility, extended styles, cloak state)
/// instead of title blacklists. `require_title` is off for the owner: only
/// the window itself must be titled.
fn passes_structural_checks(facts: &WindowFacts, require_title: bool) -> bool {
    // A child control is not a window anyone can manage. Nothing used to be
    // able to hand one over — `EnumWindows` and the shell hooks only ever
    // yield top-level handles — but `EVENT_OBJECT_SHOW` fires with
    // OBJID_WINDOW/CHILDID_SELF for every control an app shows, and a
    // captioned one (a `Button`, a `Static`, an `Edit` with text) passes
    // every other check here: `GW_OWNER` is null for children so the owner
    // probe walks up to the real top-level parent and clears, the class is
    // not in the shell list, it is visible, and it has a title. Tracking one
    // would tile a control inside its parent and then cloak it away for good
    // on the next space switch, since the app never re-shows it.
    if (facts.style & WS_CHILD) != 0 {
        return false;
    }
    if (facts.ex_style & WS_EX_TOOLWINDOW) != 0 {
        return false;
    }
    // Never-activated windows (overlays, OSDs) don't appear on the taskbar
    // unless WS_EX_APPWINDOW forces them in.
    if (facts.ex_style & WS_EX_NOACTIVATE) != 0 && (facts.ex_style & WS_EX_APPWINDOW) == 0 {
        return false;
    }
    if is_shell_class(&facts.class_name) {
        return false;
    }
    if require_title && !facts.has_title {
        return false;
    }
    if facts.externally_cloaked {
        return false;
    }
    if (facts.style & WS_VISIBLE) == 0 && !facts.hidden_by_us {
        return false;
    }
    true
}

/// A window is manageable when it passes the structural checks itself and,
/// if owned, its root owner does too. Owned dialogs of real apps stay
/// individually managed (they must cloak with their app on space switches);
/// popups of hidden or tool-window owners are noise.
fn is_eligible(facts: &WindowFacts) -> bool {
    if !passes_structural_checks(facts, true) {
        return false;
    }
    match &facts.owner {
        Some(owner) => passes_structural_checks(owner, false),
        None => true,
    }
}

/// Whether the window draws a real caption bar (`WS_CAPTION`, both bits).
///
/// A frameless top-level popup — a media viewer, an overlay, a splash, a
/// browser in F11 — owns its geometry: it sizes itself to its content or its
/// monitor and is shown and hidden by its app, so placing it by a saved rect
/// only tears it off whatever it was covering. It is still a window to
/// *track* (it must hide with its space), just not one a workspace rule or a
/// layout snapshot should ever describe.
pub(crate) fn has_app_frame(facts: &WindowFacts) -> bool {
    (facts.style & WS_CAPTION) == WS_CAPTION
}

/// A window is eligible for dynamic tiling when it is manageable (`is_eligible`),
/// unowned (not a child/dialog of another app window), and resizable (`WS_THICKFRAME`).
pub(crate) fn is_tile_eligible(facts: &WindowFacts) -> bool {
    is_eligible(facts) && facts.owner.is_none() && facts.can_resize
}

unsafe fn gather_window_facts(hwnd: HWND, follow_owner: bool) -> WindowFacts {
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;

    let mut class_buf = [0u16; 256];
    let clen = GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32);
    let class_name = if clen > 0 {
        String::from_utf16_lossy(&class_buf[..clen as usize])
    } else {
        String::new()
    };

    let mut title = [0u16; 2];
    let has_title = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) > 0;

    let state = get_window_state(hwnd);
    let mut cloaked: u32 = 0;
    let hr = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED as _,
        &mut cloaked as *mut _ as _,
        std::mem::size_of::<u32>() as u32,
    );
    let externally_cloaked = hr == 0
        && cloaked != 0
        && (state & (WINSPACES_STATE_CLOAKED | WINSPACES_STATE_SHELL_CLOAKED)) == 0;
    let hidden_by_us = (state & WINSPACES_STATE_HIDDEN_MASK) != 0;

    let owner = if follow_owner && (ex_style & WS_EX_APPWINDOW) == 0 {
        let root = GetAncestor(hwnd, GA_ROOTOWNER);
        if !root.is_null() && root != hwnd {
            Some(Box::new(gather_window_facts(root, false)))
        } else {
            None
        }
    } else {
        None
    };

    let can_resize = (style & WS_THICKFRAME) != 0;

    WindowFacts {
        style,
        ex_style,
        class_name,
        has_title,
        can_resize,
        externally_cloaked,
        hidden_by_us,
        owner,
    }
}

/// Whether the handle still refers to a live OS window.
///
/// Deliberately weaker than [`is_valid_window`]: it asks only whether the
/// window exists, not whether WinSpaces should manage it. Pruning tracked
/// windows must use *this* — a window we hid on an inactive space is
/// intentionally invisible and can momentarily fail the fuller eligibility
/// test without having been closed, and dropping it would lose the user's
/// space assignment.
///
/// `hwnd` is an opaque handle; `IsWindow` tolerates a stale one by returning
/// false, so this stays a safe fn despite the raw-pointer-typed parameter.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn is_live_window(hwnd: HWND) -> bool {
    unsafe { !hwnd.is_null() && IsWindow(hwnd) != 0 }
}

/// `hwnd` is an opaque Win32 handle: every call below (`IsWindow`,
/// `GetWindowThreadProcessId`, the eligibility probes) tolerates a stale or
/// invalid handle by failing gracefully, so this stays a safe fn despite
/// carrying a raw-pointer-typed parameter.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn is_valid_window(hwnd: HWND) -> bool {
    unsafe {
        if !is_live_window(hwnd) {
            return false;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == std::process::id() {
            return false;
        }

        is_eligible(&gather_window_facts(hwnd, true))
    }
}

/// Whether `hwnd` is a live top-level window with a caption bar — the kind a
/// workspace rule may place and a layout snapshot may record. See
/// [`has_app_frame`].
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn is_framed_window(hwnd: HWND) -> bool {
    if !is_live_window(hwnd) {
        return false;
    }
    unsafe { has_app_frame(&gather_window_facts(hwnd, false)) }
}

/// Whether `hwnd` is a candidate for dynamic tiling.
///
/// Must be a valid, manageable top-level window with a sizing border (`WS_THICKFRAME`)
/// and no owner window.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn is_tileable_window(hwnd: HWND) -> bool {
    unsafe {
        if !is_live_window(hwnd) {
            return false;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == std::process::id() {
            return false;
        }

        is_tile_eligible(&gather_window_facts(hwnd, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain visible, titled, unowned app window.
    fn app_window() -> WindowFacts {
        WindowFacts {
            style: WS_VISIBLE | WS_THICKFRAME,
            ex_style: 0,
            class_name: "Chrome_WidgetWin_1".to_string(),
            has_title: true,
            can_resize: true,
            externally_cloaked: false,
            hidden_by_us: false,
            owner: None,
        }
    }

    #[test]
    fn plain_app_window_is_eligible() {
        assert!(is_eligible(&app_window()));
    }

    /// Telegram's "Media viewer" as observed live: `WS_POPUP | WS_SYSMENU |
    /// WS_CLIPCHILDREN | WS_CLIPSIBLINGS`, titled, visible, unowned. It is a
    /// manageable window (it must hide with its space) but has no caption,
    /// so no rule may place it and no snapshot may record it.
    #[test]
    fn frameless_popup_is_manageable_but_not_framed() {
        let mut f = app_window();
        f.style = 0x8608_0000 | WS_VISIBLE;
        f.can_resize = false;
        f.class_name = "Qt51519QWindowIcon".to_string();
        assert!(is_eligible(&f));
        assert!(!has_app_frame(&f));

        // The main window of the same app, same class: captioned.
        let mut main = app_window();
        main.style = 0x96CF_0000;
        main.class_name = "Qt51519QWindowIcon".to_string();
        assert!(has_app_frame(&main));

        // WS_CAPTION is two bits; a border alone (WS_BORDER) is not a frame.
        let mut bordered = app_window();
        bordered.style = WS_VISIBLE | 0x0080_0000;
        assert!(!has_app_frame(&bordered));
    }

    /// A captioned child control as `EVENT_OBJECT_SHOW` delivers it: visible,
    /// titled, ordinary class, and — because `GW_OWNER` is null on a child —
    /// carrying its top-level *parent* as the owner facts, which pass. Only
    /// the WS_CHILD bit separates it from a real app window.
    #[test]
    fn child_control_is_excluded() {
        let mut f = app_window();
        f.style |= WS_CHILD;
        f.class_name = "Button".to_string();
        assert!(!is_eligible(&f));

        f.owner = Some(Box::new(app_window()));
        assert!(!is_eligible(&f));

        // Tiling eligibility is a strict subset, so it must reject it too
        // even though a child can carry WS_THICKFRAME.
        assert!(!is_tile_eligible(&f));
    }

    #[test]
    fn tool_window_is_excluded_even_with_appwindow() {
        let mut f = app_window();
        f.ex_style = WS_EX_TOOLWINDOW;
        assert!(!is_eligible(&f));
        f.ex_style = WS_EX_TOOLWINDOW | WS_EX_APPWINDOW;
        assert!(!is_eligible(&f));
    }

    #[test]
    fn noactivate_is_excluded_unless_appwindow() {
        let mut f = app_window();
        f.ex_style = WS_EX_NOACTIVATE;
        assert!(!is_eligible(&f));
        f.ex_style = WS_EX_NOACTIVATE | WS_EX_APPWINDOW;
        assert!(is_eligible(&f));
    }

    #[test]
    fn shell_classes_are_excluded() {
        for class in [
            "Progman",
            "Shell_TrayWnd",
            "Windows.UI.Core.CoreWindow",
            "IME",
        ] {
            let mut f = app_window();
            f.class_name = class.to_string();
            assert!(!is_eligible(&f), "{class} must be excluded");
        }
    }

    #[test]
    fn untitled_window_is_excluded_but_untitled_owner_is_fine() {
        let mut f = app_window();
        f.has_title = false;
        assert!(!is_eligible(&f));

        let mut owned = app_window();
        let mut owner = app_window();
        owner.has_title = false;
        owned.owner = Some(Box::new(owner));
        assert!(is_eligible(&owned));
    }

    #[test]
    fn dialog_with_eligible_owner_is_included() {
        let mut f = app_window();
        f.owner = Some(Box::new(app_window()));
        assert!(is_eligible(&f));
    }

    #[test]
    fn popup_with_invisible_or_tool_window_owner_is_excluded() {
        let mut hidden_owner = app_window();
        hidden_owner.style = 0;
        let mut f = app_window();
        f.owner = Some(Box::new(hidden_owner));
        assert!(!is_eligible(&f));

        let mut tool_owner = app_window();
        tool_owner.ex_style = WS_EX_TOOLWINDOW;
        f.owner = Some(Box::new(tool_owner));
        assert!(!is_eligible(&f));
    }

    #[test]
    fn dialog_of_owner_we_cloaked_stays_eligible() {
        // Space switched away while a dialog was up: the owner is cloaked by
        // us but the dialog must remain manageable so it cloaks too.
        let mut cloaked_owner = app_window();
        cloaked_owner.hidden_by_us = true;
        cloaked_owner.style = 0;
        let mut f = app_window();
        f.owner = Some(Box::new(cloaked_owner));
        assert!(is_eligible(&f));
    }

    #[test]
    fn own_cloak_state_bit_keeps_window_eligible() {
        let mut f = app_window();
        f.hidden_by_us = true;
        f.style = 0;
        assert!(is_eligible(&f));
    }

    #[test]
    fn externally_cloaked_window_is_excluded() {
        let mut f = app_window();
        f.externally_cloaked = true;
        assert!(!is_eligible(&f));
    }

    #[test]
    fn invisible_window_without_our_state_is_excluded() {
        let mut f = app_window();
        f.style = 0;
        assert!(!is_eligible(&f));
    }

    #[test]
    fn plain_app_window_is_tile_eligible() {
        assert!(is_tile_eligible(&app_window()));
    }

    #[test]
    fn non_resizable_window_is_eligible_to_manage_but_not_tile() {
        let mut f = app_window();
        f.style = WS_VISIBLE; // no WS_THICKFRAME
        f.can_resize = false;
        assert!(is_eligible(&f));
        assert!(!is_tile_eligible(&f));
    }

    #[test]
    fn owned_dialog_is_eligible_to_manage_but_not_tile() {
        let mut f = app_window();
        f.owner = Some(Box::new(app_window()));
        assert!(is_eligible(&f));
        assert!(!is_tile_eligible(&f));
    }
}
