/// COLORREF (0x00BBGGRR) helper.
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

/// Scale a color component by `alpha` (0-255) for a premultiplied-alpha
/// pixel. `component` and `alpha` are both 0-255 ranged `u32`s, matching how
/// this codebase already carries color channels (see `Tint`, `Palette`).
pub const fn premultiply(component: u32, alpha: u32) -> u32 {
    component * alpha / 255
}

/// A background tint for `gdi::surface::paint_surface`: an RGB color plus
/// the alpha it should be painted at over the DWM backdrop. Generalized off
/// the settings window's `Palette` and the menu's `MenuTheme` so this crate
/// does not need to know about either.
pub struct Tint {
    pub r: u32,
    pub g: u32,
    pub b: u32,
    pub alpha: u32,
}
