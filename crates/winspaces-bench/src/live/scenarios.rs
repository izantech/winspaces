//! The eight live scenarios (`startup` is orchestrated by `live.rs` since
//! it runs before any daemon is attached). Every scenario returns a
//! [`Scenario`] and never panics: a Win32 call that fails becomes a
//! `skipped` reason, not an unwrap.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::{Config, LayoutStore};
use winspaces_core::daemon::find_daemon_window;

use crate::live::driver;
use crate::live::sampler::Sampler;
use crate::live::verdict::{
    condition, floor_exact, floor_tolerance, growth_is_zero, last_n_median,
};
use crate::report::{Phase, Sample, Scenario, Verdict};
use crate::Opts;

/// Every scenario name this crate knows, `startup` included even though
/// `live.rs` dispatches it separately — this list is what `--scenario`
/// validates against.
pub const KNOWN: &[&str] = &[
    "idle",
    "switch",
    "overview",
    "menu",
    "reload",
    "startup",
    "indicator_ab",
    "tiling",
    "soak",
];

pub const DEFAULT: &[&str] = &["idle", "switch", "overview", "menu", "reload"];

const PRIVATE_TOLERANCE_BYTES: f64 = 256.0 * 1024.0;
const HANDLE_TOLERANCE: f64 = 2.0;
const USER_TOLERANCE: f64 = 1.0;

/// Everything a scenario needs about the daemon and the run, cheap to
/// copy (an `HWND` is a raw pointer) so every scenario function takes it
/// by value instead of threading a lifetime through the module.
#[derive(Clone, Copy)]
pub(crate) struct Ctx {
    pub msgwnd: HWND,
    pub quick: bool,
    pub verbose: bool,
    pub harness_elevated: bool,
    pub daemon_elevated: bool,
}

fn quick_secs(base: f64, quick: bool) -> f64 {
    if quick {
        base / 3.0
    } else {
        base
    }
}

/// `--quick` divides counts by 3 too, but never below 2 — the plan's own
/// wording ("clamps event counts to at least 2").
fn quick_count(base: u32, quick: bool) -> u32 {
    if quick {
        (base / 3).max(2)
    } else {
        base
    }
}

fn skip(name: &str, state_before: String, reason: impl Into<String>) -> Scenario {
    Scenario {
        name: name.to_string(),
        state_before,
        phases: Vec::new(),
        metrics: BTreeMap::new(),
        verdicts: Vec::new(),
        skipped: Some(reason.into()),
        notes: Vec::new(),
    }
}

fn mean_cycles(samples: &[Sample]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(|s| s.cycles_delta as f64).sum::<f64>() / samples.len() as f64
}

/// Cycles per wall-clock second over a phase, normalised by the real
/// spacing between samples (the 1 Hz loop drifts by its 20 ms sleep, and
/// a phase's first delta reaches back into the previous phase). With one
/// sample there is no spacing to measure; fall back to the per-sample mean.
fn cycles_per_s(samples: &[Sample]) -> f64 {
    if samples.len() < 2 {
        return mean_cycles(samples);
    }
    let span = samples[samples.len() - 1].t - samples[0].t;
    if span <= 0.0 {
        return mean_cycles(samples);
    }
    samples[1..]
        .iter()
        .map(|s| s.cycles_delta as f64)
        .sum::<f64>()
        / span
}

fn sum_cycles(samples: &[Sample]) -> f64 {
    samples.iter().map(|s| s.cycles_delta as f64).sum()
}

fn floor_of(phase: &Phase, field: impl Fn(&Sample) -> f64) -> f64 {
    last_n_median(&phase.samples, 5, field)
}

/// GDI/USER/handles/private-bytes floor, read off the last 5 samples of a
/// phase (`docs/benchmarks.md` §2: "floor" is a steady value, not a point).
struct Floor {
    gdi: f64,
    user: f64,
    handles: f64,
    private_bytes: f64,
}

fn floor_at(phase: &Phase) -> Floor {
    Floor {
        gdi: floor_of(phase, |s| s.gdi as f64),
        user: floor_of(phase, |s| s.user_objects as f64),
        handles: floor_of(phase, |s| s.handles as f64),
        private_bytes: floor_of(phase, |s| s.private_bytes as f64),
    }
}

fn floor_verdicts(before: &Floor, after: &Floor) -> Vec<Verdict> {
    vec![
        floor_exact("gdi_returns_to_floor", before.gdi, after.gdi, ""),
        floor_tolerance(
            "user_returns_to_floor",
            before.user,
            after.user,
            USER_TOLERANCE,
            "",
        ),
        floor_tolerance(
            "handles_returns_to_floor",
            before.handles,
            after.handles,
            HANDLE_TOLERANCE,
            "",
        ),
        floor_tolerance(
            "private_returns_to_floor",
            before.private_bytes,
            after.private_bytes,
            PRIVATE_TOLERANCE_BYTES,
            "B",
        ),
    ]
}

fn insert_floor_metrics(
    metrics: &mut BTreeMap<String, f64>,
    prefix: &str,
    before: &Floor,
    after: &Floor,
) {
    metrics.insert(format!("gdi_floor_before{prefix}"), before.gdi);
    metrics.insert(format!("gdi_floor_after{prefix}"), after.gdi);
    metrics.insert(format!("user_floor_before{prefix}"), before.user);
    metrics.insert(format!("user_floor_after{prefix}"), after.user);
    metrics.insert(format!("handles_floor_before{prefix}"), before.handles);
    metrics.insert(format!("handles_floor_after{prefix}"), after.handles);
    metrics.insert(
        format!("private_floor_before{prefix}_bytes"),
        before.private_bytes,
    );
    metrics.insert(
        format!("private_floor_after{prefix}_bytes"),
        after.private_bytes,
    );
}

/// Samples at 1 Hz for `seconds`, driving no events. The workhorse for
/// every `floor`/`settle` phase.
fn sample_phase(
    sampler: &mut Sampler,
    scenario: &str,
    name: &str,
    seconds: f64,
    verbose: bool,
) -> Phase {
    eprintln!("  live  {scenario}/{name}  {seconds:.0} s");
    sample_phase_with_events(sampler, scenario, name, seconds, None, verbose, |_| {})
}

/// Samples at 1 Hz for `seconds` while firing events through `fire`,
/// spaced `interval` apart starting at phase time 0, up to `max_events` of
/// them (`schedule = Some((interval, max_events))`; `None` drives no
/// events at all). Any events still unfired when the phase's wall clock
/// runs out fire immediately, so a caller relying on an exact, even event
/// count (the `switch` scenario) never comes up short from scheduling
/// jitter.
fn sample_phase_with_events(
    sampler: &mut Sampler,
    scenario: &str,
    name: &str,
    seconds: f64,
    schedule: Option<(Duration, u32)>,
    verbose: bool,
    mut fire: impl FnMut(u32),
) -> Phase {
    if let Some((_, max_events)) = schedule {
        eprintln!("  live  {scenario}/{name}  {seconds:.0} s  {max_events} events");
    }
    let start = Instant::now();
    let mut samples = Vec::new();
    let mut next_sample_at = 1.0f64;
    let mut events_fired = 0u32;
    let mut next_event_at = Duration::ZERO;
    loop {
        let elapsed = start.elapsed();
        let elapsed_s = elapsed.as_secs_f64();
        if elapsed_s >= seconds {
            break;
        }
        if let Some((iv, max_events)) = schedule {
            if events_fired < max_events && elapsed >= next_event_at {
                fire(events_fired);
                events_fired += 1;
                next_event_at += iv;
            }
        }
        if elapsed_s >= next_sample_at {
            let s = sampler.sample(elapsed_s);
            if verbose {
                eprintln!(
                    "    t={:.1}s cycles={} gdi={} user={} handles={} private={}B",
                    s.t, s.cycles_delta, s.gdi, s.user_objects, s.handles, s.private_bytes
                );
            }
            samples.push(s);
            next_sample_at += 1.0;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Fire whatever the schedule owed but wall time ran out on: an exact
    // event count matters more than exact spacing for the callers here.
    if let Some((_, max_events)) = schedule {
        while events_fired < max_events {
            fire(events_fired);
            events_fired += 1;
        }
    }
    Phase {
        name: name.to_string(),
        seconds,
        events: events_fired,
        samples,
    }
}

/// Fast dwell sampling (20 Hz) with no events, for Overview's
/// open/closed measurement windows.
fn sample_dwell(
    sampler: &mut Sampler,
    scenario: &str,
    name: &str,
    seconds: f64,
    verbose: bool,
) -> Phase {
    eprintln!("  live  {scenario}/{name}  {seconds:.1} s (20 Hz)");
    let start = Instant::now();
    let mut samples = Vec::new();
    loop {
        let elapsed_s = start.elapsed().as_secs_f64();
        if elapsed_s >= seconds {
            break;
        }
        let s = sampler.sample(elapsed_s);
        if verbose {
            eprintln!("    t={:.2}s gdi={} user={}", s.t, s.gdi, s.user_objects);
        }
        samples.push(s);
        std::thread::sleep(Duration::from_millis(50));
    }
    Phase {
        name: name.to_string(),
        seconds,
        events: 0,
        samples,
    }
}

pub(crate) fn run_one(
    name: &str,
    ctx: Ctx,
    sampler: &mut Sampler,
    opts: &Opts,
    state_before: String,
) -> Scenario {
    match name {
        "idle" => run_idle(ctx, sampler, opts, state_before),
        "switch" => run_switch(ctx, sampler, opts, state_before),
        "overview" => run_overview(ctx, sampler, opts, state_before),
        "menu" => run_menu(ctx, sampler, opts, state_before),
        "reload" => run_reload(ctx, sampler, opts, state_before),
        "indicator_ab" => run_indicator_ab(ctx, sampler, opts, state_before),
        "tiling" => run_tiling(ctx, sampler, opts, state_before),
        "soak" => run_soak(ctx, sampler, opts, state_before),
        other => skip(
            other,
            state_before,
            "not implemented (should have been rejected earlier)",
        ),
    }
}

// --- idle --------------------------------------------------------------

fn run_idle(ctx: Ctx, sampler: &mut Sampler, opts: &Opts, state_before: String) -> Scenario {
    let floor = sample_phase(
        sampler,
        "idle",
        "floor",
        quick_secs(30.0, ctx.quick),
        ctx.verbose,
    );
    let cycles: Vec<f64> = floor
        .samples
        .iter()
        .map(|s| s.cycles_delta as f64)
        .collect();
    let mut metrics = BTreeMap::new();
    metrics.insert(
        "floor_cycles_per_s".to_string(),
        cycles_per_s(&floor.samples),
    );
    let mut sorted = cycles.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    metrics.insert("floor_quiet_cycles_per_s".to_string(), median);
    let last = floor.samples.last();
    metrics.insert(
        "floor_gdi".to_string(),
        last.map(|s| s.gdi as f64).unwrap_or(0.0),
    );
    metrics.insert(
        "floor_user".to_string(),
        last.map(|s| s.user_objects as f64).unwrap_or(0.0),
    );
    metrics.insert(
        "floor_handles".to_string(),
        last.map(|s| s.handles as f64).unwrap_or(0.0),
    );
    metrics.insert(
        "floor_private_bytes".to_string(),
        last.map(|s| s.private_bytes as f64).unwrap_or(0.0),
    );
    metrics.insert(
        "threads".to_string(),
        last.map(|s| s.threads as f64).unwrap_or(0.0),
    );
    let _ = opts;
    Scenario {
        name: "idle".to_string(),
        state_before,
        phases: vec![floor],
        metrics,
        verdicts: vec![condition("sampler_ok", true, "counters read without error")],
        skipped: None,
        notes: Vec::new(),
    }
}

// --- switch --------------------------------------------------------------

/// The monitor + space `switch` toggles, from `layouts.json`'s most recent
/// topology, matched to the harness's own first enumerated monitor device
/// (`crate::stamp::monitors()[0]`). Monitor index is always 0: the plan
/// fixes "monitor 0" as the target, and this only resolves *which* two
/// spaces that monitor has.
fn resolve_switch_target() -> Result<(usize, usize), String> {
    let store = LayoutStore::load_from_file(&LayoutStore::get_path());
    let topo = store
        .topologies
        .iter()
        .max_by_key(|t| t.captured_unix)
        .ok_or_else(|| "layouts.json has no known topology".to_string())?;
    let monitors = crate::stamp::monitors();
    let device = monitors
        .first()
        .map(|m| m.device.clone())
        .ok_or_else(|| "no monitors enumerated".to_string())?;
    let mon = topo
        .monitors
        .iter()
        .find(|m| m.device == device)
        .ok_or_else(|| format!("no topology monitor matches device {device}"))?;
    if mon.space_count < 2 {
        return Err(format!(
            "monitor {device} has only {} space(s); need at least 2 to switch",
            mon.space_count
        ));
    }
    Ok((mon.current_space, mon.space_count))
}

struct SwitchRun {
    phases: Vec<Phase>,
    floor: Floor,
    settle: Floor,
    per_op_isolated: f64,
    per_op_sustained: f64,
}

/// Runs the A/B/C/D switch protocol once (`docs/benchmarks.md` §4.1),
/// toggling monitor 0 between `cur_space` and its neighbor, ending back on
/// `cur_space`. `name_prefix` lets `indicator_ab` run this twice under
/// `off/` and `on/` phase names.
fn run_switch_protocol(
    ctx: Ctx,
    sampler: &mut Sampler,
    cur_space: usize,
    space_count: usize,
    name_prefix: &str,
) -> SwitchRun {
    let alt_space = (cur_space + 1) % space_count;
    let scenario_label = "switch";

    let floor_phase = sample_phase(
        sampler,
        scenario_label,
        &format!("{name_prefix}floor"),
        quick_secs(30.0, ctx.quick),
        ctx.verbose,
    );
    let floor_mean = cycles_per_s(&floor_phase.samples);

    let mut at_alt = false;
    let mut fire = |_i: u32| {
        at_alt = !at_alt;
        let target = if at_alt { alt_space } else { cur_space };
        driver::switch(ctx.msgwnd, 0, target);
    };

    let isolated_events = quick_count(6, ctx.quick);
    let isolated_seconds = isolated_events as f64 * 10.0;
    let isolated_phase = sample_phase_with_events(
        sampler,
        scenario_label,
        &format!("{name_prefix}isolated"),
        isolated_seconds,
        Some((Duration::from_secs(10), isolated_events)),
        ctx.verbose,
        &mut fire,
    );

    let sustained_seconds = quick_secs(30.0, ctx.quick);
    let raw_sustained_events = (sustained_seconds * 1000.0 / 400.0) as u32;
    // Even, so isolated (already even) + sustained lands back on cur_space.
    let sustained_events = (raw_sustained_events - (raw_sustained_events % 2)).max(2);
    let sustained_phase = sample_phase_with_events(
        sampler,
        scenario_label,
        &format!("{name_prefix}sustained"),
        sustained_seconds,
        Some((Duration::from_millis(400), sustained_events)),
        ctx.verbose,
        &mut fire,
    );

    let settle_phase = sample_phase(
        sampler,
        scenario_label,
        &format!("{name_prefix}settle"),
        quick_secs(30.0, ctx.quick),
        ctx.verbose,
    );

    let per_op_isolated = (sum_cycles(&isolated_phase.samples)
        - floor_mean * isolated_phase.seconds)
        / isolated_phase.events as f64;
    let per_op_sustained = (sum_cycles(&sustained_phase.samples)
        - floor_mean * sustained_phase.seconds)
        / sustained_phase.events as f64;

    let floor = floor_at(&floor_phase);
    let settle = floor_at(&settle_phase);

    SwitchRun {
        phases: vec![floor_phase, isolated_phase, sustained_phase, settle_phase],
        floor,
        settle,
        per_op_isolated,
        per_op_sustained,
    }
}

fn run_switch(ctx: Ctx, sampler: &mut Sampler, opts: &Opts, state_before: String) -> Scenario {
    let _ = opts;
    let (cur_space, space_count) = match resolve_switch_target() {
        Ok(v) => v,
        Err(e) => return skip("switch", state_before, e),
    };

    let run = run_switch_protocol(ctx, sampler, cur_space, space_count, "");

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "floor_cycles_per_s".to_string(),
        cycles_per_s(&run.phases[0].samples),
    );
    metrics.insert("per_op_cycles_isolated".to_string(), run.per_op_isolated);
    metrics.insert("per_op_cycles_sustained".to_string(), run.per_op_sustained);
    insert_floor_metrics(&mut metrics, "", &run.floor, &run.settle);

    Scenario {
        name: "switch".to_string(),
        state_before,
        phases: run.phases,
        metrics,
        verdicts: floor_verdicts(&run.floor, &run.settle),
        skipped: None,
        notes: vec![format!(
            "toggled monitor 0 between space {} and space {} ({} spaces total), ending back on space {}; the starting space was read from layouts.json, which the daemon rewrites up to ~10 s after a change",
            cur_space + 1,
            (cur_space + 1) % space_count + 1,
            space_count,
            cur_space + 1
        )],
    }
}

// --- overview -----------------------------------------------------

fn run_overview(ctx: Ctx, sampler: &mut Sampler, opts: &Opts, state_before: String) -> Scenario {
    let _ = opts;
    let pre = sample_phase(
        sampler,
        "overview",
        "pre",
        quick_secs(5.0, ctx.quick),
        ctx.verbose,
    );
    let pre_floor = floor_at(&pre);

    let dwell = quick_secs(2.0, ctx.quick);
    let mut phases = vec![pre];
    let mut closed_floors: Vec<Floor> = Vec::new();
    let mut open_gdi_peak = 0.0f64;
    let mut open_user_peak = 0.0f64;
    let mut open_cycles_sum = 0.0f64;
    let mut all_closed_in_time = true;
    let mut close_detail = String::new();

    for cycle in 1..=4u32 {
        driver::toggle_overview(ctx.msgwnd);
        if !driver::wait_for_overview_visible(true, Duration::from_secs(2)) {
            return skip(
                "overview",
                state_before,
                format!("Overview did not become visible on cycle {cycle}"),
            );
        }
        let open_phase = sample_dwell(
            sampler,
            "overview",
            &format!("open_{cycle}"),
            dwell,
            ctx.verbose,
        );
        for s in &open_phase.samples {
            open_gdi_peak = open_gdi_peak.max(s.gdi as f64);
            open_user_peak = open_user_peak.max(s.user_objects as f64);
        }
        open_cycles_sum += sum_cycles(&open_phase.samples);
        phases.push(open_phase);

        driver::toggle_overview(ctx.msgwnd);
        let closed_in_time = driver::wait_for_overview_visible(false, Duration::from_secs(2));
        if !closed_in_time {
            all_closed_in_time = false;
            close_detail = format!("cycle {cycle} did not close within 2s");
        }
        let closed_phase = sample_dwell(
            sampler,
            "overview",
            &format!("closed_{cycle}"),
            dwell,
            ctx.verbose,
        );
        closed_floors.push(floor_at(&closed_phase));
        phases.push(closed_phase);
    }

    let after_1 = &closed_floors[0];
    let after_4 = &closed_floors[3];

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "retained_after_first_gdi".to_string(),
        after_1.gdi - pre_floor.gdi,
    );
    metrics.insert(
        "retained_after_first_user".to_string(),
        after_1.user - pre_floor.user,
    );
    metrics.insert(
        "retained_after_first_private_bytes".to_string(),
        after_1.private_bytes - pre_floor.private_bytes,
    );
    let growth_gdi = after_4.gdi - after_1.gdi;
    let growth_user = after_4.user - after_1.user;
    let growth_handles = after_4.handles - after_1.handles;
    metrics.insert("growth_rounds_2_to_4_gdi".to_string(), growth_gdi);
    metrics.insert("growth_rounds_2_to_4_user".to_string(), growth_user);
    metrics.insert("growth_rounds_2_to_4_handles".to_string(), growth_handles);
    metrics.insert("open_peak_gdi".to_string(), open_gdi_peak);
    metrics.insert("open_peak_user".to_string(), open_user_peak);
    metrics.insert("open_cycles_per_open".to_string(), open_cycles_sum / 4.0);

    let verdicts = vec![
        growth_is_zero("cache_not_leak_gdi", growth_gdi, ""),
        growth_is_zero("cache_not_leak_user", growth_user, ""),
        growth_is_zero("cache_not_leak_handles", growth_handles, ""),
        condition(
            "closes_within_2s",
            all_closed_in_time,
            if all_closed_in_time {
                "every close was observed within 2s".to_string()
            } else {
                close_detail
            },
        ),
    ];

    Scenario {
        name: "overview".to_string(),
        state_before,
        phases,
        metrics,
        verdicts,
        skipped: None,
        notes: vec![
            "Overview's window is retained hidden after the first close by design (a cache, not a leak); see docs/benchmarks.md §2.".to_string(),
        ],
    }
}

// --- menu ------------------------------------------------------------------

fn run_menu(ctx: Ctx, sampler: &mut Sampler, opts: &Opts, state_before: String) -> Scenario {
    let _ = opts;
    if ctx.daemon_elevated && !ctx.harness_elevated {
        return skip(
            "menu",
            state_before,
            "UIPI: daemon is elevated and the harness is not; run dev bench live --admin",
        );
    }

    let floor_before_phase = sample_phase(
        sampler,
        "menu",
        "floor_before",
        quick_secs(10.0, ctx.quick),
        ctx.verbose,
    );
    let floor_before = floor_at(&floor_before_phase);

    let mut phases = vec![floor_before_phase];
    let mut cycle_samples: Vec<Sample> = Vec::new();
    let clock = Instant::now();

    let mut repaint_cycles_per_repaint = 0.0f64;
    let mut in_paint_peak_gdi = 0.0f64;
    let mut open_gdi_delta = 0.0f64;
    let mut open_user_delta = 0.0f64;
    let mut all_opened = true;
    let mut all_closed = true;
    let mut fail_detail = String::new();

    for cycle in 1..=4u32 {
        driver::open_menu(ctx.msgwnd);
        let hwnd = match driver::menu_window(Duration::from_secs(2)) {
            Some(h) => h,
            None => {
                all_opened = false;
                fail_detail = format!("the tray menu did not appear on cycle {cycle}");
                break;
            }
        };
        let open_sample = sampler.sample(clock.elapsed().as_secs_f64());
        if cycle == 1 {
            open_gdi_delta = open_sample.gdi as f64 - floor_before.gdi;
            open_user_delta = open_sample.user_objects as f64 - floor_before.user;
            cycle_samples.push(open_sample);

            let pre_repaint = sampler.sample(clock.elapsed().as_secs_f64());
            for _ in 0..20 {
                driver::force_repaint(hwnd);
                std::thread::sleep(Duration::from_millis(50));
            }
            let post_repaint = sampler.sample(clock.elapsed().as_secs_f64());
            repaint_cycles_per_repaint = post_repaint.cycles_delta as f64 / 20.0;
            cycle_samples.push(pre_repaint);
            cycle_samples.push(post_repaint);

            let inpaint_deadline =
                Instant::now() + Duration::from_secs_f64(quick_secs(2.0, ctx.quick));
            while Instant::now() < inpaint_deadline {
                driver::async_invalidate(hwnd);
                in_paint_peak_gdi = in_paint_peak_gdi.max(sampler.peek_gdi() as f64);
            }
        } else {
            cycle_samples.push(open_sample);
        }

        if !driver::close_menu(hwnd, Duration::from_secs(2)) {
            all_closed = false;
            fail_detail = format!("the tray menu did not close by message on cycle {cycle}");
        }
    }

    if !all_opened {
        return skip("menu", state_before, fail_detail);
    }

    phases.push(Phase {
        name: "cycles".to_string(),
        seconds: clock.elapsed().as_secs_f64(),
        events: 4,
        samples: cycle_samples,
    });

    let floor_after_phase = sample_phase(
        sampler,
        "menu",
        "floor_after",
        quick_secs(10.0, ctx.quick),
        ctx.verbose,
    );
    let floor_after = floor_at(&floor_after_phase);
    phases.push(floor_after_phase);

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "repaint_cycles_per_repaint".to_string(),
        repaint_cycles_per_repaint,
    );
    metrics.insert("in_paint_peak_gdi".to_string(), in_paint_peak_gdi);
    metrics.insert("open_gdi_delta".to_string(), open_gdi_delta);
    metrics.insert("open_user_delta".to_string(), open_user_delta);
    insert_floor_metrics(&mut metrics, "", &floor_before, &floor_after);

    let verdicts = vec![
        condition(
            "menu_opened",
            all_opened,
            "the menu window appeared on every cycle",
        ),
        condition(
            "menu_closed_by_message",
            all_closed,
            if all_closed {
                "WM_MENU_CLOSE closed the menu on every cycle".to_string()
            } else {
                fail_detail.clone()
            },
        ),
        floor_exact(
            "handles_returned_on_close_gdi",
            floor_before.gdi,
            floor_after.gdi,
            "",
        ),
        floor_tolerance(
            "handles_returned_on_close_user",
            floor_before.user,
            floor_after.user,
            USER_TOLERANCE,
            "",
        ),
        floor_exact(
            "handles_returned_on_close_handles",
            floor_before.handles,
            floor_after.handles,
            "",
        ),
    ];

    Scenario {
        name: "menu".to_string(),
        state_before,
        phases,
        metrics,
        verdicts,
        skipped: None,
        notes: Vec::new(),
    }
}

// --- reload ------------------------------------------------------------------

fn run_reload(ctx: Ctx, sampler: &mut Sampler, opts: &Opts, state_before: String) -> Scenario {
    let _ = opts;
    let floor_phase = sample_phase(
        sampler,
        "reload",
        "floor",
        quick_secs(10.0, ctx.quick),
        ctx.verbose,
    );
    let floor_mean = cycles_per_s(&floor_phase.samples);

    let events = quick_count(10, ctx.quick);
    let events_seconds = events as f64 * 2.0;
    let events_phase = sample_phase_with_events(
        sampler,
        "reload",
        "events",
        events_seconds,
        Some((Duration::from_secs(2), events)),
        ctx.verbose,
        |_i| driver::reload_config(ctx.msgwnd),
    );

    let settle_phase = sample_phase(
        sampler,
        "reload",
        "settle",
        quick_secs(10.0, ctx.quick),
        ctx.verbose,
    );

    let per_op_cycles = (sum_cycles(&events_phase.samples) - floor_mean * events_phase.seconds)
        / events_phase.events as f64;

    let before = floor_at(&floor_phase);
    let after = floor_at(&settle_phase);

    let mut metrics = BTreeMap::new();
    metrics.insert("per_op_cycles".to_string(), per_op_cycles);
    insert_floor_metrics(&mut metrics, "", &before, &after);

    Scenario {
        name: "reload".to_string(),
        state_before,
        phases: vec![floor_phase, events_phase, settle_phase],
        metrics,
        verdicts: floor_verdicts(&before, &after),
        skipped: None,
        notes: Vec::new(),
    }
}

// --- startup -------------------------------------------------------------

/// Spawns `exe` itself and measures the cold start. Runs before any daemon
/// is attached, so it owns its own [`Sampler`] rather than sharing the
/// one `live::run` builds afterward for every other scenario.
pub(crate) fn run_startup(
    exe: &Path,
    quick: bool,
    verbose: bool,
    state_before: String,
) -> Scenario {
    let t0 = Instant::now();
    if let Err(e) = std::process::Command::new(exe).spawn() {
        return skip(
            "startup",
            state_before,
            format!("spawning {}: {e}", exe.display()),
        );
    }

    let deadline = Instant::now() + Duration::from_secs(15);
    let hwnd = loop {
        let hwnd = find_daemon_window();
        if !hwnd.is_null() {
            break hwnd;
        }
        if Instant::now() >= deadline {
            return skip(
                "startup",
                state_before,
                "message window did not appear within 15s",
            );
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let time_to_window_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let pid = driver::pid_of(hwnd);
    let mut sampler = match Sampler::new(pid) {
        Ok(s) => s,
        Err(e) => return skip("startup", state_before, e),
    };
    // First-ever reading always carries a zero cycle delta (no prior
    // reading to diff against); prime it so the real samples below don't.
    sampler.sample(0.0);

    let one_s = quick_secs(1.0, quick);
    let ten_s = quick_secs(10.0, quick);
    std::thread::sleep(Duration::from_secs_f64(one_s));
    let s1 = sampler.sample(one_s);
    std::thread::sleep(Duration::from_secs_f64(ten_s - one_s));
    let s10 = sampler.sample(ten_s);
    if verbose {
        eprintln!("  live  startup  t+{one_s:.1}s and t+{ten_s:.1}s samples taken");
    }

    let mut metrics = BTreeMap::new();
    metrics.insert("time_to_window_ms".to_string(), time_to_window_ms);
    metrics.insert("cold_private_bytes".to_string(), s10.private_bytes as f64);
    metrics.insert("cold_gdi".to_string(), s10.gdi as f64);
    metrics.insert("cold_user".to_string(), s10.user_objects as f64);
    metrics.insert("cold_handles".to_string(), s10.handles as f64);
    metrics.insert("cold_threads".to_string(), s10.threads as f64);
    metrics.insert(
        "first_10s_cycles".to_string(),
        (s1.cycles_delta + s10.cycles_delta) as f64,
    );

    Scenario {
        name: "startup".to_string(),
        state_before,
        phases: vec![
            Phase {
                name: format!("t+{one_s:.1}s"),
                seconds: one_s,
                events: 0,
                samples: vec![s1],
            },
            Phase {
                name: format!("t+{ten_s:.1}s"),
                seconds: ten_s - one_s,
                events: 0,
                samples: vec![s10],
            },
        ],
        metrics,
        verdicts: vec![condition(
            "window_within_5s",
            time_to_window_ms <= 5000.0,
            format!("{time_to_window_ms:.0} ms to the message window appearing"),
        )],
        skipped: None,
        notes: Vec::new(),
    }
}

// --- indicator_ab ------------------------------------------------------------

/// Restores `settings.json` to its exact original bytes on drop, including
/// on an early return or panic unwind — the config-editing scenario must
/// never leave the user's file changed.
struct RestoreConfigOnDrop {
    path: std::path::PathBuf,
    original: Vec<u8>,
    msgwnd: HWND,
    done: bool,
}

impl Drop for RestoreConfigOnDrop {
    fn drop(&mut self) {
        if !self.done {
            let _ = std::fs::write(&self.path, &self.original);
            driver::reload_config(self.msgwnd);
        }
    }
}

fn run_indicator_ab(
    ctx: Ctx,
    sampler: &mut Sampler,
    opts: &Opts,
    state_before: String,
) -> Scenario {
    if !opts.allow_config_edit {
        return skip("indicator_ab", state_before, "needs --allow-config-edit");
    }
    let (cur_space, space_count) = match resolve_switch_target() {
        Ok(v) => v,
        Err(e) => return skip("indicator_ab", state_before, e),
    };

    let path = Config::get_config_path();
    let original = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            return skip(
                "indicator_ab",
                state_before,
                format!("reading {}: {e}", path.display()),
            )
        }
    };
    let mut guard = RestoreConfigOnDrop {
        path: path.clone(),
        original: original.clone(),
        msgwnd: ctx.msgwnd,
        done: false,
    };

    let mut cfg = Config::load_from_file(&path);

    cfg.space_indicator = false;
    if let Err(e) = cfg.save_to_file(&path) {
        return skip(
            "indicator_ab",
            state_before,
            format!("writing {}: {e}", path.display()),
        );
    }
    driver::reload_config(ctx.msgwnd);
    std::thread::sleep(Duration::from_secs(1));
    let off = run_switch_protocol(ctx, sampler, cur_space, space_count, "off/");

    cfg.space_indicator = true;
    if let Err(e) = cfg.save_to_file(&path) {
        return skip(
            "indicator_ab",
            state_before,
            format!("writing {}: {e}", path.display()),
        );
    }
    driver::reload_config(ctx.msgwnd);
    std::thread::sleep(Duration::from_secs(1));
    let on = run_switch_protocol(ctx, sampler, cur_space, space_count, "on/");

    std::fs::write(&path, &original).ok();
    driver::reload_config(ctx.msgwnd);
    guard.done = true;
    drop(guard);

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "off_per_op_cycles_isolated".to_string(),
        off.per_op_isolated,
    );
    metrics.insert(
        "off_per_op_cycles_sustained".to_string(),
        off.per_op_sustained,
    );
    metrics.insert("on_per_op_cycles_isolated".to_string(), on.per_op_isolated);
    metrics.insert(
        "on_per_op_cycles_sustained".to_string(),
        on.per_op_sustained,
    );
    metrics.insert(
        "ab_delta_per_op_cycles_isolated".to_string(),
        on.per_op_isolated - off.per_op_isolated,
    );
    metrics.insert(
        "ab_delta_per_op_cycles_sustained".to_string(),
        on.per_op_sustained - off.per_op_sustained,
    );

    let mut phases = off.phases;
    phases.extend(on.phases);

    Scenario {
        name: "indicator_ab".to_string(),
        state_before,
        phases,
        metrics,
        verdicts: Vec::new(),
        skipped: None,
        notes: vec![
            "ran the switch protocol twice (space_indicator=false then true) on the same monitor/spaces, restoring the original settings.json bytes afterward".to_string(),
        ],
    }
}

// --- tiling ------------------------------------------------------------------

fn run_tiling(ctx: Ctx, sampler: &mut Sampler, opts: &Opts, state_before: String) -> Scenario {
    if !opts.allow_disruptive {
        return skip("tiling", state_before, "needs --allow-disruptive");
    }

    let floor_phase = sample_phase(
        sampler,
        "tiling",
        "floor",
        quick_secs(10.0, ctx.quick),
        ctx.verbose,
    );
    let floor_mean = cycles_per_s(&floor_phase.samples);

    // Two on/off repetitions == 4 toggles == an even count, so the feature
    // ends back in whatever state it started in.
    let toggles = 4u32;
    let dwell = quick_secs(2.0, ctx.quick);
    let toggle_phase = sample_phase_with_events(
        sampler,
        "tiling",
        "toggle",
        toggles as f64 * dwell,
        Some((Duration::from_secs_f64(dwell), toggles)),
        ctx.verbose,
        |_i| driver::toggle_tiling(ctx.msgwnd),
    );

    let settle_phase = sample_phase(
        sampler,
        "tiling",
        "settle",
        quick_secs(10.0, ctx.quick),
        ctx.verbose,
    );

    let per_toggle_cycles = (sum_cycles(&toggle_phase.samples) - floor_mean * toggle_phase.seconds)
        / toggle_phase.events as f64;

    let before = floor_at(&floor_phase);
    let after = floor_at(&settle_phase);

    let mut metrics = BTreeMap::new();
    metrics.insert("per_toggle_cycles".to_string(), per_toggle_cycles);
    insert_floor_metrics(&mut metrics, "", &before, &after);

    Scenario {
        name: "tiling".to_string(),
        state_before,
        phases: vec![floor_phase, toggle_phase, settle_phase],
        metrics,
        verdicts: floor_verdicts(&before, &after),
        skipped: None,
        notes: vec![
            "toggled tiling on/off twice (4 toggles), ending back in the starting state"
                .to_string(),
        ],
    }
}

// --- soak --------------------------------------------------------------------

fn linreg_slope_per_second(samples: &[Sample], field: impl Fn(&Sample) -> f64) -> f64 {
    let xs: Vec<f64> = samples.iter().map(|s| s.t).collect();
    let ys: Vec<f64> = samples.iter().map(field).collect();
    let n = xs.len() as f64;
    if n < 2.0 {
        return 0.0;
    }
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = ys.iter().sum::<f64>() / n;
    let mut num = 0.0;
    let mut den = 0.0;
    for (x, y) in xs.iter().zip(&ys) {
        num += (x - mean_x) * (y - mean_y);
        den += (x - mean_x).powi(2);
    }
    if den == 0.0 {
        0.0
    } else {
        num / den
    }
}

fn run_soak(ctx: Ctx, sampler: &mut Sampler, opts: &Opts, state_before: String) -> Scenario {
    let minutes = match opts.minutes {
        Some(m) if m > 0.0 => m,
        _ => return skip("soak", state_before, "needs --minutes"),
    };
    let seconds = minutes * 60.0;
    let floor = sample_phase(sampler, "soak", "floor", seconds, ctx.verbose);

    let drift_private =
        linreg_slope_per_second(&floor.samples, |s| s.private_bytes as f64) * 3600.0;
    let drift_handles = linreg_slope_per_second(&floor.samples, |s| s.handles as f64) * 3600.0;
    let drift_gdi = linreg_slope_per_second(&floor.samples, |s| s.gdi as f64) * 3600.0;

    let mut metrics = BTreeMap::new();
    metrics.insert("drift_private_bytes_per_hour".to_string(), drift_private);
    metrics.insert("drift_handles_per_hour".to_string(), drift_handles);
    metrics.insert("drift_gdi_per_hour".to_string(), drift_gdi);
    metrics.insert(
        "floor_cycles_per_s".to_string(),
        cycles_per_s(&floor.samples),
    );

    let verdicts = vec![
        condition(
            "private_drift_under_1mib_per_hour",
            drift_private.abs() <= 1024.0 * 1024.0,
            format!("{drift_private:+.0} B/hour"),
        ),
        condition(
            "handles_flat",
            drift_handles.abs() <= HANDLE_TOLERANCE,
            format!("{drift_handles:+.2} handles/hour"),
        ),
    ];

    Scenario {
        name: "soak".to_string(),
        state_before,
        phases: vec![floor],
        metrics,
        verdicts,
        skipped: None,
        notes: vec![format!("idle soak, {minutes:.1} minute(s) at 1 Hz")],
    }
}
