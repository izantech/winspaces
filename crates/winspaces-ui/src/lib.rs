//! Owner-drawn UI surfaces: the tray flyout menu, Mission Control, the native
//! settings window, and the transient space indicator.

pub mod menu;
pub mod mission_control;
pub mod settings;
pub mod space_indicator;
/// Palette resolution shared by every surface in this crate; nothing above
/// it picks colours, so it stays crate-private.
pub(crate) mod theme;
pub mod tiling_preview;
pub mod tray;
