//! Tiling engine types: layout kind, gaps, direction, drag outcomes, and per-space state.

use std::collections::{HashMap, HashSet};
use windows_sys::Win32::Foundation::{HWND, RECT};
use winspaces_common::WindowRect;

/// Minimum and maximum ratio bounds for split divisions.
pub const MIN_RATIO: f32 = 0.1;
pub const MAX_RATIO: f32 = 0.9;
pub const DEFAULT_RATIO: f32 = 0.5;

/// Supported tiling layout kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutKind {
    /// Hyprland-like dwindle/BSP layout: each window splits the remaining area along its longer dimension.
    #[default]
    Dwindle,
    /// Master-stack layout: one large master window on the left, stack on the right. (Reserved for future).
    #[allow(dead_code)]
    MasterStack,
}

/// Inner (between tiles) and outer (screen edge) gap sizes in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Gaps {
    pub inner: u32,
    pub outer: u32,
}

impl Gaps {
    pub const NONE: Self = Self { inner: 0, outer: 0 };

    pub fn new(inner: u32, outer: u32) -> Self {
        Self { inner, outer }
    }

    /// Scale gaps by monitor DPI (baseline 96 DPI).
    pub fn scaled_for_dpi(&self, dpi: u32) -> Self {
        if dpi == 96 || dpi == 0 {
            *self
        } else {
            Self {
                inner: (self.inner * dpi + 48) / 96,
                outer: (self.outer * dpi + 48) / 96,
            }
        }
    }
}

/// Orientation override for the primary dwindle split (split 0).
///
/// Naming convention, used consistently across the tiler: `Horizontal` means
/// the *windows* sit side by side (the separator line is vertical);
/// `Vertical` means they stack (the separator is horizontal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitDirection {
    /// Aspect-ratio driven (`w >= h` -> side-by-side), the historical behavior.
    #[default]
    Auto,
    /// Force side-by-side (left | right).
    Horizontal,
    /// Force stacked (top / bottom).
    Vertical,
}

/// Everything the bin needs to toast a completed split toggle, by value.
///
/// The work rect is carried rather than looked up because the toast runs
/// inside the caller's `AppState` borrow and must not re-enter it — same
/// rationale as `SwitchNotice`.
#[derive(Clone, Copy)]
pub struct SplitToggleNotice {
    /// The direction the primary split just became (never `Auto`).
    pub direction: SplitDirection,
    /// Work rect of the monitor whose space toggled.
    pub work: RECT,
}

/// Navigation / focus / swap direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Result of classifying a window drag gesture on a tiled space.
#[derive(Debug, Clone, PartialEq)]
pub enum DragOutcome {
    /// Border drag on the first split edge adjusts the split ratio.
    AdjustDwindleRatio { ratio_index: usize, new_ratio: f32 },
    /// Dragging a tile over another tile swaps their slot order positions.
    Reorder { from_index: usize, to_index: usize },
    /// Shift was held through a positional drag: toggle the primary split.
    ToggleSplit,
    /// Ambiguous drag, deep split drag, or drag dropped outside tiles -> snap back.
    SnapBack,
}

/// Active drag tracking for mouse gestures on tiled spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilingDrag {
    pub hwnd: HWND,
    pub start_rect: WindowRect,
    pub mon_idx: usize,
    pub space_idx: usize,
    /// The window was maximized when the drag began. Windows restores it on
    /// the first move, so the size delta at drop is not a border resize.
    pub from_maximized: bool,
}

/// State of a single space's tiling arrangement.
/// Maintained in parallel with `MonitorState.spaces`.
#[derive(Debug, Clone)]
pub struct TileSpace {
    pub layout: LayoutKind,
    /// Orientation override for split 0; deeper splits stay aspect-driven.
    pub split_direction: SplitDirection,
    /// Stable slot order maintained by the tiler (unlike `spaces[s]` which is z-order derived).
    pub order: Vec<HWND>,
    /// Split ratios for dwindle splits (index `i` is split ratio for window `i`, default 0.5).
    pub ratios: Vec<f32>,
    /// Last applied target frames: "this placement is ours".
    pub expected: HashMap<HWND, WindowRect>,
    /// Resistance strikes counter (2 strikes -> auto-float).
    pub strikes: HashMap<HWND, u8>,
    /// Flatten retry attempts counter for maximized windows (3 attempts -> auto-float).
    pub flatten_strikes: HashMap<HWND, u8>,
    /// Windows honoured as maximized over the layout on the last flush. They
    /// keep their slot in `order` (and its rect in `expected`) but are never
    /// pushed; restoring one returns it to that slot.
    pub maximized: HashSet<HWND>,
    /// Windows sitting on their slot but larger than it: clamped by their own
    /// minimum size, not resisting. Kept so the overflow is logged once.
    pub overflowing: HashSet<HWND>,
    /// Whether this space needs retiling on next flush.
    pub dirty: bool,
}

impl Default for TileSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl TileSpace {
    pub fn new() -> Self {
        Self {
            layout: LayoutKind::Dwindle,
            split_direction: SplitDirection::Auto,
            order: Vec::new(),
            ratios: Vec::new(),
            expected: HashMap::new(),
            strikes: HashMap::new(),
            flatten_strikes: HashMap::new(),
            maximized: HashSet::new(),
            overflowing: HashSet::new(),
            dirty: false,
        }
    }

    pub fn ratio(&self, index: usize) -> f32 {
        self.ratios
            .get(index)
            .copied()
            .unwrap_or(DEFAULT_RATIO)
            .clamp(MIN_RATIO, MAX_RATIO)
    }

    pub fn set_ratio(&mut self, index: usize, ratio: f32) {
        if self.ratios.len() <= index {
            self.ratios.resize(index + 1, DEFAULT_RATIO);
        }
        self.ratios[index] = ratio.clamp(MIN_RATIO, MAX_RATIO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaps_dpi_scaling() {
        let gaps = Gaps::new(8, 16);
        // 96 DPI (1.0x) -> exact
        assert_eq!(gaps.scaled_for_dpi(96), Gaps::new(8, 16));
        // 144 DPI (1.5x) -> 8 * 1.5 = 12, 16 * 1.5 = 24
        assert_eq!(gaps.scaled_for_dpi(144), Gaps::new(12, 24));
        // 120 DPI (1.25x) -> (8 * 120 + 48) / 96 = 10, (16 * 120 + 48) / 96 = 20
        assert_eq!(gaps.scaled_for_dpi(120), Gaps::new(10, 20));
        // 0 DPI fallback
        assert_eq!(gaps.scaled_for_dpi(0), Gaps::new(8, 16));
    }
}
