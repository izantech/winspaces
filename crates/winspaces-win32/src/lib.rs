//! Thin Win32 wrapper layer: GDI drawing primitives, window-class registration,
//! DWM/DPI helpers, and the hand-rolled FFI (shell cloak, event hooks) shared by
//! WinSpaces' UI surfaces.
#![deny(unsafe_op_in_unsafe_fn)]

pub mod display;
pub mod dpi;
pub mod dwm;
pub mod gdi;
pub mod glyphs;
pub mod hooks;
pub mod module;
pub mod shell_cloak;
pub mod text;
pub mod window_class;
