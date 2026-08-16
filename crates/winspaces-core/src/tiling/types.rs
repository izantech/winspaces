//! Tiling engine types: layout kind, gaps, direction, drag outcomes, and per-space state.

use std::collections::{HashMap, HashSet};
use windows_sys::Win32::Foundation::HWND;
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
    /// Ambiguous drag, deep split drag, or drag dropped outside tiles -> snap back.
    SnapBack,
}

/// State of a single space's tiling arrangement.
/// Maintained in parallel with `MonitorState.spaces`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TileSpace {
    pub layout: LayoutKind,
    /// Stable slot order maintained by the tiler (unlike `spaces[s]` which is z-order derived).
    pub order: Vec<HWND>,
    /// Set of windows currently floating on this space.
    pub floating: HashSet<HWND>,
    /// Split ratios for dwindle splits (index `i` is split ratio for window `i`, default 0.5).
    pub ratios: Vec<f32>,
    /// Inner and outer gaps for this space.
    pub gaps: Gaps,
    /// Last applied target frames: "this placement is ours".
    pub expected: HashMap<HWND, WindowRect>,
    /// Resistance strikes counter (2 strikes -> auto-float).
    pub strikes: HashMap<HWND, u8>,
    /// Whether this space needs retiling on next flush.
    pub dirty: bool,
}

impl Default for TileSpace {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl TileSpace {
    pub fn new() -> Self {
        Self {
            layout: LayoutKind::Dwindle,
            order: Vec::new(),
            floating: HashSet::new(),
            ratios: Vec::new(),
            gaps: Gaps::NONE,
            expected: HashMap::new(),
            strikes: HashMap::new(),
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
