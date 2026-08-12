//! Segoe Fluent Icons glyph codepoints shared by the menu, Mission Control
//! and the settings window (the same codepoints exist in the older Segoe
//! MDL2 Assets font). One table, so a codepoint is never hand-typed twice
//! under two different names.

// menu.rs's original GLYPH_* table.
pub const GLYPH_TASK_VIEW: u16 = 0xE7C4;
pub const GLYPH_MONITOR: u16 = 0xE7F4;
pub const GLYPH_CAMERA: u16 = 0xE722;
pub const GLYPH_RESTORE: u16 = 0xE777;
pub const GLYPH_SETTINGS: u16 = 0xE713;
pub const GLYPH_SYNC: u16 = 0xE895;
pub const GLYPH_REFRESH: u16 = 0xE72C;
pub const GLYPH_CLOSE: u16 = 0xE8BB;
pub const GLYPH_ADD: u16 = 0xE710;
pub const GLYPH_REMOVE: u16 = 0xE738;
pub const GLYPH_CHECK: u16 = 0xE73E;
pub const GLYPH_CHEVRON: u16 = 0xE76C;

// settings_ui/pages.rs's previously-bare codepoints (19 call sites, 11
// distinct values). Two of those 11 duplicated a constant above and were
// NOT given synonyms — reuse GLYPH_MONITOR (0xE7F4) and GLYPH_RESTORE
// (0xE777) instead.
pub const GLYPH_KEYBOARD: u16 = 0xE92C;
pub const GLYPH_WORKSPACES: u16 = 0xEA37;
pub const GLYPH_MOVE: u16 = 0xE898;
pub const GLYPH_TASKBAR: u16 = 0xE737;
pub const GLYPH_AUTOSTART: u16 = 0xE7E8;
pub const GLYPH_THEME: u16 = 0xE790;
pub const GLYPH_PREV: u16 = 0xE892;
pub const GLYPH_NEXT: u16 = 0xE893;
pub const GLYPH_SNAPSHOT: u16 = 0xE7C5;

// controls.rs:327's bare ChevronDown literal — a distinct codepoint and
// purpose from GLYPH_CHEVRON above (which points right, for submenus).
pub const GLYPH_CHEVRON_DOWN: u16 = 0xE70D;

// settings_ui/mod.rs's bare banner-icon literals.
pub const GLYPH_COMPLETED: u16 = 0xE930;
pub const GLYPH_ERROR: u16 = 0xEA39;
