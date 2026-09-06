//! `io/*`: the atomic-rename persist path for `settings.json` and
//! `layouts.json`, run against a scratch directory under the system temp
//! folder and cleaned up afterward.

use winspaces_common::{
    Config, LayoutStore, MonitorSnapshot, RelRect, TopologySnapshot, WindowRect, WindowSnapshot,
    WorkspaceRule,
};

use crate::timing::Runner;

fn bench_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("winspaces-bench")
}

fn rule(i: usize) -> WorkspaceRule {
    WorkspaceRule {
        name: format!("Rule {i}"),
        aumid: String::new(),
        exe_path: format!("c:\\apps\\app-{i}.exe"),
        class_name: "AppWindowClass".to_string(),
        title_pattern: String::new(),
        display_index: i % 2,
        space_index: i % 9,
        show_cmd: 1,
        rect: Default::default(),
        is_snapped: false,
        is_sticky: false,
    }
}

fn config_with_rules(n: usize) -> Config {
    Config {
        workspace_rules: (0..n).map(rule).collect(),
        ..Config::default()
    }
}

fn monitor(idx: usize) -> MonitorSnapshot {
    let x = idx as i32 * 1920;
    MonitorSnapshot {
        stable_id: format!("mon-{idx}"),
        device: format!("\\\\.\\DISPLAY{}", idx + 1),
        rect: WindowRect {
            left: x,
            top: 0,
            right: x + 1920,
            bottom: 1080,
        },
        work: WindowRect {
            left: x,
            top: 0,
            right: x + 1920,
            bottom: 1032,
        },
        dpi: 96,
        current_space: 0,
        space_count: 9,
    }
}

fn window_snapshot(idx: usize, monitor_count: usize) -> WindowSnapshot {
    let mon_idx = idx % monitor_count;
    let rect = WindowRect {
        left: idx as i32 * 4,
        top: 0,
        right: idx as i32 * 4 + 800,
        bottom: 600,
    };
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
        rel: RelRect::from_abs(&rect, &mon_work),
        rect,
        dpi: 96,
        hwnd: 0,
        pid: 1000 + idx as u32,
    }
}

fn layout_store_8x60() -> LayoutStore {
    let mut store = LayoutStore::default();
    for t in 0..8u64 {
        store.upsert(TopologySnapshot {
            signature: format!("sig-{t}"),
            monitors: (0..2).map(monitor).collect(),
            windows: (0..60).map(|i| window_snapshot(i, 2)).collect(),
            captured_unix: t + 1,
        });
    }
    store
}

pub fn bench(r: &mut Runner) {
    let cfg_name = "io/config_save_atomic";
    let layouts_name = "io/layouts_save_atomic/8x60";
    if !r.selected(cfg_name) && !r.selected(layouts_name) {
        return;
    }

    let dir = bench_dir();
    let _ = std::fs::create_dir_all(&dir);

    if r.selected(cfg_name) {
        let path = dir.join("settings-bench.json");
        let cfg = config_with_rules(50);
        r.bench(cfg_name, || {
            cfg.save_to_file(&path).expect("write config");
        });
    }

    if r.selected(layouts_name) {
        let path = dir.join("layouts-bench.json");
        let store = layout_store_8x60();
        r.bench(layouts_name, || {
            store.save_to_file(&path).expect("write layout store");
        });
    }

    let _ = std::fs::remove_dir_all(&dir);
}
