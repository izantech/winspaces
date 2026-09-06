//! `tiling/*`: the dwindle layout algorithm, slot reconciliation, directional
//! navigation and drag classification — the pure logic behind every retile.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::WindowRect;
use winspaces_core::tiling::algorithms::compute_dwindle;
use winspaces_core::tiling::{classify_drag, directional_neighbor, reconcile_order};
use winspaces_core::tiling::{Direction, Gaps, SplitDirection};

use crate::timing::Runner;

/// 2560x1440 minus a 48 px taskbar, the reference work area the plan prices
/// tiling against.
fn work_area() -> WindowRect {
    WindowRect {
        left: 0,
        top: 0,
        right: 2560,
        bottom: 1440 - 48,
    }
}

fn fake_hwnd(v: usize) -> HWND {
    v as *mut c_void
}

fn dwindle_nine(work: &WindowRect) -> Vec<(HWND, WindowRect)> {
    let ratios = [0.5f32; 9];
    compute_dwindle(work, 9, &ratios, &Gaps::NONE, SplitDirection::Auto)
        .into_iter()
        .enumerate()
        .map(|(i, rect)| (fake_hwnd(i + 1), rect))
        .collect()
}

pub fn bench(r: &mut Runner) {
    bench_compute_dwindle(r);
    bench_reconcile_order(r);
    bench_directional_neighbor(r);
    bench_classify_drag(r);
}

fn bench_compute_dwindle(r: &mut Runner) {
    let work = work_area();
    let ratios = [0.5f32; 9];

    for n in [1usize, 2, 4, 9] {
        let name = format!("tiling/compute_dwindle/n={n}");
        r.bench(&name, || {
            let tiles = compute_dwindle(&work, n, &ratios, &Gaps::NONE, SplitDirection::Auto);
            std::hint::black_box(tiles);
        });
    }

    let gaps = Gaps::new(8, 16);
    r.bench("tiling/compute_dwindle/n=9,gaps=8x16", || {
        let tiles = compute_dwindle(&work, 9, &ratios, &gaps, SplitDirection::Auto);
        std::hint::black_box(tiles);
    });

    r.bench("tiling/compute_dwindle/n=9,split=vertical", || {
        let tiles = compute_dwindle(&work, 9, &ratios, &Gaps::NONE, SplitDirection::Vertical);
        std::hint::black_box(tiles);
    });
}

fn bench_reconcile_order(r: &mut Runner) {
    let current: Vec<HWND> = (1..=9).map(fake_hwnd).collect();
    // Candidates drop hwnd 5 (index 4) and add a brand-new hwnd 100.
    let mut candidates: Vec<HWND> = current
        .iter()
        .copied()
        .filter(|&h| h != fake_hwnd(5))
        .collect();
    candidates.push(fake_hwnd(100));
    let focused = Some(current[3]); // the 4th window in slot order

    r.bench("tiling/reconcile_order/9+1-1", || {
        let order = reconcile_order(&current, &candidates, focused);
        std::hint::black_box(order);
    });
}

fn bench_directional_neighbor(r: &mut Runner) {
    let tiles = dwindle_nine(&work_area());
    let target = tiles[0].0;

    for (label, dir) in [
        ("left", Direction::Left),
        ("right", Direction::Right),
        ("up", Direction::Up),
        ("down", Direction::Down),
    ] {
        let name = format!("tiling/directional_neighbor/{label}");
        r.bench(&name, || {
            let hit = directional_neighbor(
                std::hint::black_box(target),
                std::hint::black_box(dir),
                std::hint::black_box(&tiles),
            );
            std::hint::black_box(hit);
        });
    }
}

fn bench_classify_drag(r: &mut Runner) {
    let work = work_area();
    let tiles = dwindle_nine(&work);

    // Split 0's two tiles: `resize.rs` reads the ratio off exactly this pair.
    let (h0, t0) = tiles[0].clone();
    let ratio_new = WindowRect {
        left: t0.left,
        top: t0.top,
        right: t0.right + 200,
        bottom: t0.bottom,
    };
    r.bench("tiling/classify_drag/ratio", || {
        let outcome = classify_drag(
            std::hint::black_box(h0),
            &t0,
            std::hint::black_box(&ratio_new),
            std::hint::black_box(&tiles),
            (t0.left, t0.top),
            false,
            false,
        );
        std::hint::black_box(outcome);
    });

    // Tile 2 (index 1) dropped at the centre of tile 5 (index 4): same size,
    // position only.
    let (h1, t1) = tiles[1].clone();
    let t4 = &tiles[4].1;
    let drop_center = ((t4.left + t4.right) / 2, (t4.top + t4.bottom) / 2);
    r.bench("tiling/classify_drag/reorder", || {
        let outcome = classify_drag(
            std::hint::black_box(h1),
            &t1,
            &t1,
            std::hint::black_box(&tiles),
            std::hint::black_box(drop_center),
            false,
            false,
        );
        std::hint::black_box(outcome);
    });

    // Dropped well outside every tile: snaps back.
    let outside = (work.right + 10_000, work.bottom + 10_000);
    r.bench("tiling/classify_drag/snapback", || {
        let outcome = classify_drag(
            std::hint::black_box(h0),
            &t0,
            &t0,
            std::hint::black_box(&tiles),
            std::hint::black_box(outside),
            false,
            false,
        );
        std::hint::black_box(outcome);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dwindle_nine_pairs_every_tile_with_a_distinct_hwnd() {
        let tiles = dwindle_nine(&work_area());
        assert_eq!(tiles.len(), 9);
        let mut hwnds: Vec<HWND> = tiles.iter().map(|(h, _)| *h).collect();
        hwnds.sort();
        hwnds.dedup();
        assert_eq!(hwnds.len(), 9, "every tile must carry a distinct hwnd");
    }

    #[test]
    fn a_smoke_run_covers_every_named_benchmark() {
        let mut r = Runner::new(true, None, false);
        bench(&mut r);
        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        for expected in [
            "tiling/compute_dwindle/n=1",
            "tiling/compute_dwindle/n=2",
            "tiling/compute_dwindle/n=4",
            "tiling/compute_dwindle/n=9",
            "tiling/compute_dwindle/n=9,gaps=8x16",
            "tiling/compute_dwindle/n=9,split=vertical",
            "tiling/reconcile_order/9+1-1",
            "tiling/directional_neighbor/left",
            "tiling/directional_neighbor/right",
            "tiling/directional_neighbor/up",
            "tiling/directional_neighbor/down",
            "tiling/classify_drag/ratio",
            "tiling/classify_drag/reorder",
            "tiling/classify_drag/snapback",
        ] {
            assert!(names.contains(&expected), "missing benchmark {expected}");
        }
    }
}
