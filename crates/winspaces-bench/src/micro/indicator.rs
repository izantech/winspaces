//! `indicator/*`: the transient space-toast per-pixel alpha fill and its
//! easing curve.

use windows_sys::Win32::Foundation::RECT;
use winspaces_ui::space_indicator::geometry::{indicator_rect, round_rect_coverage, smoothstep};
use winspaces_win32::dpi::px;

use crate::timing::Runner;

pub fn bench(r: &mut Runner) {
    bench_round_rect_coverage(r);
    bench_smoothstep(r);
}

/// Replicates the edge-band loop `paint_panel` runs around
/// `round_rect_coverage` (`crates/winspaces-ui/src/space_indicator.rs`),
/// summing every pixel's outer and inner coverage into one accumulator.
fn bench_round_rect_coverage(r: &mut Runner) {
    let work = RECT {
        left: 0,
        top: 0,
        right: 2560,
        bottom: 1440,
    };
    let scale = 2.0f32;
    let panel = indicator_rect(work, scale, 160);
    let width = panel.right - panel.left;
    let height = panel.bottom - panel.top;

    let radius = px(scale, winspaces_ui::space_indicator::geometry::RADIUS) as f32;
    let border_w = scale.max(1.0);
    let band = (radius.ceil().max(border_w.ceil()) as i32 + 1).min(width.min(height));

    r.bench("indicator/round_rect_coverage/toast", || {
        let mut sum = 0.0f32;
        for y in 0..height {
            let edge_row = y < band || y >= height - band;
            for x in 0..width {
                if edge_row || x < band || x >= width - band {
                    let outer = round_rect_coverage(x, y, width, height, radius, 0.0);
                    let inner =
                        round_rect_coverage(x, y, width, height, radius - border_w, border_w);
                    sum += outer + inner;
                }
            }
        }
        std::hint::black_box(sum);
    });
}

fn bench_smoothstep(r: &mut Runner) {
    r.bench("indicator/smoothstep", || {
        std::hint::black_box(smoothstep(std::hint::black_box(0.42)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_smoke_run_covers_every_named_benchmark() {
        let mut r = Runner::new(true, None, false);
        bench(&mut r);
        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        for expected in [
            "indicator/round_rect_coverage/toast",
            "indicator/smoothstep",
        ] {
            assert!(names.contains(&expected), "missing benchmark {expected}");
        }
    }
}
