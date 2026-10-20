//! Owner-drawn UI surfaces: the tray flyout menu, Overview, the native
//! settings window, the transient space indicator, and the Shelf strip.

pub mod menu;
pub mod overview;
pub mod settings;
pub mod shelf;
pub mod space_indicator;
/// Palette resolution shared by every surface in this crate; nothing above
/// it picks colours, so it stays crate-private.
pub(crate) mod theme;
pub mod tiling_preview;
pub mod tray;
pub(crate) mod window_icon;
