//! `win32/stable_monitor_ids`: `QueryDisplayConfig`, paid on every topology
//! change reconcile.

use winspaces_core::topology::stable_monitor_ids;

use crate::timing::Runner;

pub fn bench(r: &mut Runner) {
    r.bench("win32/stable_monitor_ids", || {
        std::hint::black_box(stable_monitor_ids());
    });
}
