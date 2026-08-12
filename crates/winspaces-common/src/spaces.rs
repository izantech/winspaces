/// Hard ceiling on spaces per monitor. Keeps the hotkey ID partition and the
/// digit shortcuts (Alt+1..9, Mission Control 1..9) compile-time constant
/// while the actual per-monitor count varies at runtime.
pub const MAX_DESKTOPS: usize = 9;
/// Space count a monitor starts with before the user grows or shrinks it
/// (also the serde default for snapshots captured before counts existed).
pub const DEFAULT_DESKTOPS: usize = 4;
