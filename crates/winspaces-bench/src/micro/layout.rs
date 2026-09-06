//! `layout/*`: the layout-shadow equality check and the `layouts.json`
//! (de)serialization round trip.

use winspaces_common::{
    LayoutStore, MonitorSnapshot, RelRect, TopologySnapshot, WindowRect, WindowSnapshot,
};
use winspaces_core::layout_store::same_layout;

use crate::timing::Runner;

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> WindowRect {
    WindowRect {
        left,
        top,
        right,
        bottom,
    }
}

fn monitor(idx: usize) -> MonitorSnapshot {
    let x = idx as i32 * 1920;
    MonitorSnapshot {
        stable_id: format!("mon-{idx}"),
        device: format!("\\\\.\\DISPLAY{}", idx + 1),
        rect: rect(x, 0, x + 1920, 1080),
        work: rect(x, 0, x + 1920, 1032),
        dpi: 96,
        current_space: 0,
        space_count: 9,
    }
}

fn window_snapshot(idx: usize, monitor_count: usize) -> WindowSnapshot {
    let mon_idx = idx % monitor_count;
    let r = rect(idx as i32 * 4, 0, idx as i32 * 4 + 800, 600);
    let mon_work = monitor(mon_idx).work;
    WindowSnapshot {
        name: format!("Window {idx}"),
        aumid: String::new(),
        exe_path: format!("c:\\apps\\app-{idx}.exe"),
        class_name: "AppWindowClass".to_string(),
        title_pattern: String::new(),
        stable_monitor_id: format!("mon-{mon_idx}"),
        space_index: idx % 9,
        show_cmd: 1,
        is_snapped: false,
        is_sticky: false,
        rel: RelRect::from_abs(&r, &mon_work),
        rect: r,
        dpi: 96,
        hwnd: 0,
        pid: 1000 + idx as u32,
    }
}

fn topology(
    signature: &str,
    monitor_count: usize,
    window_count: usize,
    captured_unix: u64,
) -> TopologySnapshot {
    TopologySnapshot {
        signature: signature.to_string(),
        monitors: (0..monitor_count).map(monitor).collect(),
        windows: (0..window_count)
            .map(|i| window_snapshot(i, monitor_count))
            .collect(),
        captured_unix,
    }
}

pub fn bench(r: &mut Runner) {
    bench_same_layout(r);
    bench_store(r);
}

fn bench_same_layout(r: &mut Runner) {
    let a = topology("sig-a", 2, 60, 1);
    let b = a.clone();
    r.bench("layout/same_layout/60/equal", || {
        std::hint::black_box(same_layout(&a, &b));
    });

    let mut c = a.clone();
    let last = c.windows.len() - 1;
    c.windows[last].rect = rect(9_999, 9_999, 10_199, 10_399);
    r.bench("layout/same_layout/60/differ", || {
        std::hint::black_box(same_layout(&a, &c));
    });
}

fn bench_store(r: &mut Runner) {
    let mut store = LayoutStore::default();
    for t in 0..8u64 {
        store.upsert(topology(&format!("sig-{t}"), 2, 60, t + 1));
    }

    r.bench("layout/store/serialize/8x60", || {
        let json = serde_json::to_string_pretty(&store).expect("serialize layout store");
        std::hint::black_box(json);
    });

    let json = serde_json::to_string_pretty(&store).expect("serialize layout store");
    r.bench("layout/store/parse/8x60", || {
        let parsed: LayoutStore = serde_json::from_str(&json).expect("parse layout store");
        std::hint::black_box(parsed);
    });

    // `upsert` consumes a fresh store per iteration; `layout/store/clone`
    // prices that copy on its own so the eviction cost can be separated.
    r.bench("layout/store/clone/8x60", || {
        std::hint::black_box(store.clone());
    });

    r.bench("layout/store/upsert_at_cap", || {
        let mut clone = store.clone();
        clone.upsert(topology("sig-fresh", 2, 60, 999));
        std::hint::black_box(clone);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_builder_yields_the_requested_shape() {
        let t = topology("sig", 2, 60, 1);
        assert_eq!(t.monitors.len(), 2);
        assert_eq!(t.windows.len(), 60);
    }

    #[test]
    fn a_smoke_run_covers_every_named_benchmark() {
        let mut r = Runner::new(true, None, false);
        bench(&mut r);
        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        for expected in [
            "layout/same_layout/60/equal",
            "layout/same_layout/60/differ",
            "layout/store/serialize/8x60",
            "layout/store/parse/8x60",
            "layout/store/clone/8x60",
            "layout/store/upsert_at_cap",
        ] {
            assert!(names.contains(&expected), "missing benchmark {expected}");
        }
    }
}
