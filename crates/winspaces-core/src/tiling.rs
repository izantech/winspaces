//! Hyprland-like dynamic dwindle tiling window manager engine.

pub mod algorithms;
pub mod apply;
pub mod engine;
pub mod membership;
pub mod neighbors;
pub mod notify;
pub mod resize;
pub mod types;

pub use algorithms::compute;
pub use apply::apply_layout;
pub use membership::reconcile_order;
pub use neighbors::directional_neighbor;
pub use notify::{schedule_retile, set_retile_scheduler};
pub use resize::classify_drag;
pub use types::{
    Direction, DragOutcome, Gaps, LayoutKind, TileSpace, TilingDrag, DEFAULT_RATIO, MAX_RATIO,
    MIN_RATIO,
};
