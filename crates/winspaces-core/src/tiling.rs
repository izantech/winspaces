//! Hyprland-like dynamic dwindle tiling window manager engine.

pub mod algorithms;
pub mod membership;
pub mod neighbors;
pub mod resize;
pub mod types;

pub use algorithms::compute;
pub use membership::reconcile_order;
pub use neighbors::directional_neighbor;
pub use resize::classify_drag;
pub use types::{
    Direction, DragOutcome, Gaps, LayoutKind, TileSpace, DEFAULT_RATIO, MAX_RATIO, MIN_RATIO,
};
