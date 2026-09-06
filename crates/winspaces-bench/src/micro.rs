//! Group `micro`: pure logic through the crates' public API only.
//!
//! One submodule per area; each exposes `pub fn bench(r: &mut Runner)` and
//! builds its inputs before the timed closure. Names are `area/what/variant`
//! and stable: `compare` matches on them across runs.

mod config;
mod hotkey;
mod hotkeys;
mod i18n;
mod indicator;
mod layout;
mod tiling;
mod topology;
mod workspaces;

use crate::timing::Runner;

pub fn run(r: &mut Runner) {
    tiling::bench(r);
    workspaces::bench(r);
    layout::bench(r);
    config::bench(r);
    i18n::bench(r);
    hotkey::bench(r);
    hotkeys::bench(r);
    topology::bench(r);
    indicator::bench(r);
}
