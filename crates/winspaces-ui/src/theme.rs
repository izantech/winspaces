//! Theme resolution for the settings window: the UI-local theme preference
//! (`HKCU\Software\WinSpaces\GuiTheme` — deliberately outside the daemon's
//! `settings.json` contract), the system light/dark state, the DWM accent
//! color, and the resolved color palette. Every color the settings UI paints
//! comes from the `Palette` returned here — no other module hardcodes ARGB.

use windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows_sys::Win32::UI::Accessibility::HIGHCONTRASTW;
use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST};

use winspaces_win32::registry::{read_hkcu_string, read_hkcu_u32, write_hkcu_string};

/// Re-exported so existing `theme::rgb` consumers keep compiling now that the
/// implementation lives in winspaces-win32 alongside menu.rs's and
/// overview's copies.
pub use winspaces_win32::gdi::color::rgb;
use winspaces_win32::gdi::color::Tint;

const THEME_KEY: &str = "Software\\WinSpaces";
const THEME_VALUE: &str = "GuiTheme";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemePref {
    System,
    Light,
    Dark,
}

impl ThemePref {
    pub fn as_str(self) -> &'static str {
        match self {
            ThemePref::System => "system",
            ThemePref::Light => "light",
            ThemePref::Dark => "dark",
        }
    }

    // Infallible by design (unrecognized/missing values fall back to System),
    // so this intentionally does not implement `std::str::FromStr`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "light" => ThemePref::Light,
            "dark" => ThemePref::Dark,
            _ => ThemePref::System,
        }
    }

    pub fn label(self) -> &'static str {
        use winspaces_common::i18n::t;
        use winspaces_common::Msg;
        t(match self {
            ThemePref::System => Msg::ThemeSystem,
            ThemePref::Light => Msg::ThemeLight,
            ThemePref::Dark => Msg::ThemeDark,
        })
    }

    pub const ALL: [ThemePref; 3] = [ThemePref::System, ThemePref::Light, ThemePref::Dark];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}

/// Resolved paint palette. COLORREF values unless noted.
#[derive(Clone)]
pub struct Palette {
    pub light: bool,
    pub high_contrast: bool,
    /// Window base tint, painted at `base_alpha` over the Mica backdrop
    /// (opaque when Mica is unavailable).
    pub base_r: u32,
    pub base_g: u32,
    pub base_b: u32,
    pub base_alpha: u32,

    pub text: u32,
    pub text_dim: u32,

    pub card: u32,
    pub card_hover: u32,
    pub card_pressed: u32,
    pub card_stroke: u32,

    pub ctl: u32,
    pub ctl_hover: u32,
    pub ctl_pressed: u32,
    pub ctl_stroke: u32,

    pub accent: u32,
    pub accent_text: u32,

    pub success: u32,
    pub success_bg: u32,
    pub critical: u32,
    pub critical_bg: u32,

    pub focus: u32,
}

impl Palette {
    /// The base window tint (`base_r/g/b` at `base_alpha`) as a `Tint`, for
    /// `winspaces_win32::gdi::surface::paint_surface` call sites — that
    /// primitive lives below settings and must not know about `Palette`.
    pub fn tint(&self) -> Tint {
        Tint {
            r: self.base_r,
            g: self.base_g,
            b: self.base_b,
            alpha: self.base_alpha,
        }
    }
}

pub fn load_pref() -> ThemePref {
    read_hkcu_string(THEME_KEY, THEME_VALUE)
        .map(|s| ThemePref::from_str(&s))
        .unwrap_or(ThemePref::System)
}

pub fn save_pref(pref: ThemePref) {
    let _ = write_hkcu_string(THEME_KEY, THEME_VALUE, pref.as_str());
}

/// System `AppsUseLightTheme` flag (defaults to light when unreadable,
/// matching how Windows treats a missing value).
pub fn system_uses_light() -> bool {
    read_hkcu_u32(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    )
    .map(|flag| flag != 0)
    .unwrap_or(true)
}

pub fn high_contrast_active() -> bool {
    unsafe {
        let mut hc: HIGHCONTRASTW = std::mem::zeroed();
        hc.cbSize = std::mem::size_of::<HIGHCONTRASTW>() as u32;
        if SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, &mut hc as *mut _ as _, 0) == 0 {
            return false;
        }
        const HCF_HIGHCONTRASTON: u32 = 1;
        hc.dwFlags & HCF_HIGHCONTRASTON != 0
    }
}

/// DWM accent color as COLORREF, with a sane fallback (Windows default blue).
fn accent_color() -> u32 {
    unsafe {
        let mut argb: u32 = 0;
        let mut opaque: i32 = 0;
        if DwmGetColorizationColor(&mut argb, &mut opaque) == 0 {
            let r = (argb >> 16) & 0xFF;
            let g = (argb >> 8) & 0xFF;
            let b = argb & 0xFF;
            return r | (g << 8) | (b << 16);
        }
    }
    rgb(0x00, 0x67, 0xC0)
}

/// Accent for the tiling drag ghost preview: the system highlight color under
/// high contrast (the DWM accent may be indistinguishable from the scheme's
/// background there), the DWM accent otherwise.
pub fn preview_accent() -> u32 {
    if high_contrast_active() {
        use windows_sys::Win32::Graphics::Gdi::GetSysColor;
        const COLOR_HIGHLIGHT: i32 = 13;
        return unsafe { GetSysColor(COLOR_HIGHLIGHT) } as u32;
    }
    accent_color()
}

/// True when this preference currently resolves to light.
pub fn resolves_light(pref: ThemePref) -> bool {
    match pref {
        ThemePref::Light => true,
        ThemePref::Dark => false,
        ThemePref::System => system_uses_light(),
    }
}

pub fn build_palette(pref: ThemePref, mica: bool) -> Palette {
    let high_contrast = high_contrast_active();
    let light = resolves_light(pref);
    let accent = accent_color();

    if high_contrast {
        // Drive everything from the system scheme: solid background, system
        // text/highlight colors, visible 1px strokes everywhere.
        use windows_sys::Win32::Graphics::Gdi::GetSysColor;
        const COLOR_WINDOW: i32 = 5;
        const COLOR_WINDOWTEXT: i32 = 8;
        const COLOR_GRAYTEXT: i32 = 17;
        const COLOR_HIGHLIGHT: i32 = 13;
        const COLOR_HIGHLIGHTTEXT: i32 = 14;
        const COLOR_BTNFACE: i32 = 15;
        let win = unsafe { GetSysColor(COLOR_WINDOW) } as u32;
        let text = unsafe { GetSysColor(COLOR_WINDOWTEXT) } as u32;
        let gray = unsafe { GetSysColor(COLOR_GRAYTEXT) } as u32;
        let hilite = unsafe { GetSysColor(COLOR_HIGHLIGHT) } as u32;
        let hilite_text = unsafe { GetSysColor(COLOR_HIGHLIGHTTEXT) } as u32;
        let btn = unsafe { GetSysColor(COLOR_BTNFACE) } as u32;
        return Palette {
            light,
            high_contrast,
            base_r: win & 0xFF,
            base_g: (win >> 8) & 0xFF,
            base_b: (win >> 16) & 0xFF,
            base_alpha: 255,
            text,
            text_dim: gray,
            card: win,
            card_hover: btn,
            card_pressed: btn,
            card_stroke: text,
            ctl: btn,
            ctl_hover: hilite,
            ctl_pressed: hilite,
            ctl_stroke: text,
            accent: hilite,
            accent_text: hilite_text,
            success: text,
            success_bg: win,
            critical: text,
            critical_bg: win,
            focus: text,
        };
    }

    if light {
        Palette {
            light,
            high_contrast,
            base_r: 0xF3,
            base_g: 0xF3,
            base_b: 0xF3,
            base_alpha: if mica { 216 } else { 255 },
            text: rgb(0x1B, 0x1B, 0x1B),
            text_dim: rgb(0x60, 0x60, 0x60),
            card: rgb(0xFB, 0xFB, 0xFB),
            card_hover: rgb(0xF6, 0xF6, 0xF6),
            card_pressed: rgb(0xF0, 0xF0, 0xF0),
            card_stroke: rgb(0xE5, 0xE5, 0xE5),
            ctl: rgb(0xFB, 0xFB, 0xFB),
            ctl_hover: rgb(0xF6, 0xF6, 0xF6),
            ctl_pressed: rgb(0xF5, 0xF5, 0xF5),
            ctl_stroke: rgb(0xD1, 0xD1, 0xD1),
            accent,
            accent_text: rgb(0xFF, 0xFF, 0xFF),
            success: rgb(0x0F, 0x7B, 0x0F),
            success_bg: rgb(0xDF, 0xF6, 0xDD),
            critical: rgb(0xC4, 0x2B, 0x1C),
            critical_bg: rgb(0xFD, 0xE7, 0xE9),
            focus: rgb(0x1B, 0x1B, 0x1B),
        }
    } else {
        Palette {
            light,
            high_contrast,
            base_r: 0x20,
            base_g: 0x20,
            base_b: 0x20,
            base_alpha: if mica { 216 } else { 255 },
            text: rgb(0xF5, 0xF5, 0xF5),
            text_dim: rgb(0xA6, 0xA6, 0xA6),
            card: rgb(0x2B, 0x2B, 0x2B),
            card_hover: rgb(0x32, 0x32, 0x32),
            card_pressed: rgb(0x27, 0x27, 0x27),
            card_stroke: rgb(0x1D, 0x1D, 0x1D),
            ctl: rgb(0x34, 0x34, 0x34),
            ctl_hover: rgb(0x3A, 0x3A, 0x3A),
            ctl_pressed: rgb(0x30, 0x30, 0x30),
            ctl_stroke: rgb(0x45, 0x45, 0x45),
            accent,
            accent_text: rgb(0x00, 0x00, 0x00),
            success: rgb(0x6C, 0xCB, 0x5F),
            success_bg: rgb(0x39, 0x3D, 0x1B),
            critical: rgb(0xFF, 0x99, 0xA4),
            critical_bg: rgb(0x44, 0x27, 0x26),
            focus: rgb(0xF5, 0xF5, 0xF5),
        }
    }
}

/// Flyout palette for the tray context menu (`menu.rs`) — the same theme
/// preference resolved here as for the settings window's `Palette`, so the
/// two surfaces can never disagree about light/dark, but a deliberately
/// distinct set of tints: the flyout reads slightly lighter and more opaque
/// than the settings window. Do NOT unify these values with `build_palette`'s
/// light/dark branches — they are close by design, not duplicated by
/// accident. The flyout's background alpha (`menu.rs`'s `BG_ALPHA`, 232) is
/// likewise intentionally higher than `Palette::base_alpha` (216).
pub struct MenuTheme {
    pub text: u32,
    pub dim: u32,
    pub hover: u32,
    pub separator: u32,
    pub bg_r: u32,
    pub bg_g: u32,
    pub bg_b: u32,
}

/// Palette for the transient space indicator. A third small struct rather
/// than a reuse of `MenuTheme`: the indicator is a peer surface, not a part
/// of the flyout, and the whole point of the crate-level theme module is that
/// surfaces share the *preference* (light/dark/high-contrast) without reaching
/// into each other's tints. Same reasoning as the `MenuTheme` note above.
///
/// Unlike the flyout and the settings window this one is opaque: the indicator
/// is a layered window with no DWM backdrop (see `docs/space-indicator.md`),
/// so there is nothing behind it for a translucent tint to reveal.
pub struct IndicatorTheme {
    pub bg: u32,
    pub text: u32,
    pub border: u32,
}

pub fn indicator_theme(light: bool) -> IndicatorTheme {
    if high_contrast_active() {
        use windows_sys::Win32::Graphics::Gdi::GetSysColor;
        const COLOR_WINDOW: i32 = 5;
        const COLOR_WINDOWTEXT: i32 = 8;
        let text = unsafe { GetSysColor(COLOR_WINDOWTEXT) } as u32;
        return IndicatorTheme {
            bg: unsafe { GetSysColor(COLOR_WINDOW) } as u32,
            text,
            // The border carries the panel's shape in high contrast, where
            // background and desktop may be the same color.
            border: text,
        };
    }
    if light {
        IndicatorTheme {
            bg: rgb(0xF9, 0xF9, 0xF9),
            text: rgb(0x1B, 0x1B, 0x1B),
            border: rgb(0xE0, 0xE0, 0xE0),
        }
    } else {
        IndicatorTheme {
            bg: rgb(0x2C, 0x2C, 0x2C),
            text: rgb(0xF5, 0xF5, 0xF5),
            border: rgb(0x45, 0x45, 0x45),
        }
    }
}

pub fn menu_theme(light: bool) -> MenuTheme {
    if light {
        MenuTheme {
            text: rgb(0x1B, 0x1B, 0x1B),
            dim: rgb(0x60, 0x60, 0x60),
            hover: rgb(0xEA, 0xEA, 0xEA),
            separator: rgb(0xE0, 0xE0, 0xE0),
            bg_r: 0xF9,
            bg_g: 0xF9,
            bg_b: 0xF9,
        }
    } else {
        MenuTheme {
            text: rgb(0xF5, 0xF5, 0xF5),
            dim: rgb(0xA6, 0xA6, 0xA6),
            hover: rgb(0x3D, 0x3D, 0x3D),
            separator: rgb(0x45, 0x45, 0x45),
            bg_r: 0x2C,
            bg_g: 0x2C,
            bg_b: 0x2C,
        }
    }
}
