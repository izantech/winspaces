//! `compare`: diff two reports, flag what moved beyond the documented noise
//! (`reports/benchmark-suite-plan.md` section 5).
//!
//! Continuous metrics (median ns, cycles, bytes, ms, exe/section size) are
//! flagged only when they moved in the *worse* direction beyond their
//! threshold; an improvement is shown as `better` and never flagged, same as
//! `docs/benchmarks.md`'s own convention. Counters that are meant to be flat
//! floors (GDI/USER/handle/thread counts, `Cargo.lock` package count) are
//! flagged on any change past their tolerance, in either direction, because
//! an unexplained floor movement is itself the anomaly.

use std::collections::{BTreeMap, BTreeSet};

use crate::report::{
    fmt_metric, BenchEntry, BenchGroup, LiveGroup, Report, Scenario, Section, StaticInfo,
};
use crate::stats::{fmt_bytes, fmt_cycles, fmt_ns, pct_change};

const MEDIAN_NS_THRESHOLD_PCT: f64 = 15.0;
const LIVE_CYCLES_THRESHOLD_PCT: f64 = 20.0;
const LIVE_BYTES_THRESHOLD_PCT: f64 = 5.0;
const LIVE_MS_THRESHOLD_PCT: f64 = 20.0;
const USER_TOLERANCE: f64 = 1.0;
const EXE_SIZE_THRESHOLD_PCT: f64 = 2.0;
const SECTION_SIZE_THRESHOLD_PCT: f64 = 5.0;

/// Signed percent delta and whether it crossed `threshold_pct` in the worse
/// (increasing) direction. `base == 0` with a positive `new` counts as a
/// regression (there is no base to divide by, but something appeared).
fn worse_above(base: f64, new: f64, threshold_pct: f64) -> (bool, String) {
    match pct_change(base, new) {
        Some(delta) => (delta > threshold_pct, format!("{delta:+.1}%")),
        None if base == 0.0 && new > 0.0 => (true, format!("+{new:.0} (base 0)")),
        None => (false, "-".to_string()),
    }
}

/// Flags any change past `tolerance`, in either direction — for counters
/// that are expected to be exactly flat.
fn drifted(base: f64, new: f64, tolerance: f64) -> (bool, String) {
    let diff = new - base;
    (diff.abs() > tolerance, format!("{diff:+.0}"))
}

fn header(base: &Report, new: &Report) -> String {
    let mut out = String::new();
    out.push_str("# Benchmark comparison\n\n");
    out.push_str("| | base | new |\n| :--- | :--- | :--- |\n");
    out.push_str(&format!(
        "| Date | {} | {} |\n",
        base.generated_iso, new.generated_iso
    ));
    out.push_str(&format!(
        "| Commit | {}{} | {}{} |\n",
        if base.build.git_sha.is_empty() {
            "?"
        } else {
            &base.build.git_sha
        },
        if base.build.git_dirty { " (dirty)" } else { "" },
        if new.build.git_sha.is_empty() {
            "?"
        } else {
            &new.build.git_sha
        },
        if new.build.git_dirty { " (dirty)" } else { "" },
    ));
    out.push_str(&format!(
        "| Profile | {} | {} |\n",
        base.build.profile, new.build.profile
    ));
    out.push_str(&format!(
        "| Machine | {} | {} |\n\n",
        if base.machine.cpu.is_empty() {
            "?"
        } else {
            &base.machine.cpu
        },
        if new.machine.cpu.is_empty() {
            "?"
        } else {
            &new.machine.cpu
        },
    ));
    if base.machine.cpu != new.machine.cpu {
        out.push_str(
            "> **Warning:** base and new ran on different machines; a diff between them is not a regression claim.\n\n",
        );
    }
    if base.build.profile != new.build.profile {
        out.push_str(
            "> **Warning:** base and new were built with different profiles; sizes and timings are not comparable.\n\n",
        );
    }
    out
}

/// `micro`/`primitives`: match by `BenchEntry.name`, threshold on median ns.
fn compare_bench_group(
    out: &mut String,
    title: &str,
    base: Option<&BenchGroup>,
    new: Option<&BenchGroup>,
) -> bool {
    let (Some(base), Some(new)) = (base, new) else {
        return false;
    };
    let mut regressed = false;
    out.push_str(&format!("## {title}\n\n"));

    let base_map: BTreeMap<&str, &BenchEntry> =
        base.entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let new_map: BTreeMap<&str, &BenchEntry> =
        new.entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let names: BTreeSet<&str> = base_map.keys().chain(new_map.keys()).copied().collect();

    let mut only_base = Vec::new();
    let mut only_new = Vec::new();
    out.push_str(
        "| Benchmark | base median | new median | delta | base cycles | new cycles | flag |\n",
    );
    out.push_str("| :--- | ---: | ---: | ---: | ---: | ---: | :--- |\n");
    for name in names {
        match (base_map.get(name), new_map.get(name)) {
            (Some(b), Some(n)) => {
                let (flagged, delta_text) =
                    worse_above(b.ns.median, n.ns.median, MEDIAN_NS_THRESHOLD_PCT);
                let improved = pct_change(b.ns.median, n.ns.median)
                    .map(|d| d < -MEDIAN_NS_THRESHOLD_PCT)
                    .unwrap_or(false);
                if flagged {
                    regressed = true;
                }
                let flag = if flagged {
                    "REGRESSION"
                } else if improved {
                    "better"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "| `{name}` | {} | {} | {} | {} | {} | {} |\n",
                    fmt_ns(b.ns.median),
                    fmt_ns(n.ns.median),
                    delta_text,
                    fmt_cycles(b.cycles.median),
                    fmt_cycles(n.cycles.median),
                    flag
                ));
            }
            (Some(_), None) => only_base.push(name),
            (None, Some(_)) => only_new.push(name),
            (None, None) => unreachable!(),
        }
    }
    out.push('\n');
    if !only_base.is_empty() {
        out.push_str("Only in base: ");
        out.push_str(
            &only_base
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str("\n\n");
    }
    if !only_new.is_empty() {
        out.push_str("Only in new: ");
        out.push_str(
            &only_new
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push_str("\n\n");
    }
    regressed
}

/// The threshold and direction a live metric key is judged by, from the
/// unit suffix its name carries (same convention `report::fmt_metric` uses).
fn live_metric_flag(key: &str, base: f64, new: f64) -> (bool, String) {
    if key.contains("cycles") {
        worse_above(base, new, LIVE_CYCLES_THRESHOLD_PCT)
    } else if key.ends_with("_bytes") {
        worse_above(base, new, LIVE_BYTES_THRESHOLD_PCT)
    } else if key.ends_with("_ms") {
        worse_above(base, new, LIVE_MS_THRESHOLD_PCT)
    } else if key.contains("user") {
        drifted(base, new, USER_TOLERANCE)
    } else if key.contains("gdi") || key.contains("handles") || key.contains("threads") {
        drifted(base, new, 0.0)
    } else {
        // An unrecognized metric kind is reported but never flagged: better
        // to show an unjudged number than to guess a threshold for it.
        (
            false,
            pct_change(base, new).map_or_else(|| format!("{new:+.2}"), |d| format!("{d:+.1}%")),
        )
    }
}

/// `live`: match scenarios by name, then metrics by key; verdict flips.
fn compare_live_group(out: &mut String, base: Option<&LiveGroup>, new: Option<&LiveGroup>) -> bool {
    let (Some(base), Some(new)) = (base, new) else {
        return false;
    };
    let mut regressed = false;
    out.push_str("## Live daemon\n\n");

    let base_map: BTreeMap<&str, &Scenario> = base
        .scenarios
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();
    let new_map: BTreeMap<&str, &Scenario> =
        new.scenarios.iter().map(|s| (s.name.as_str(), s)).collect();
    let names: BTreeSet<&str> = base_map.keys().chain(new_map.keys()).copied().collect();

    for name in names {
        let (b, n) = match (base_map.get(name), new_map.get(name)) {
            (Some(b), Some(n)) => (b, n),
            (Some(_), None) => {
                out.push_str(&format!("### `{name}`: only in base\n\n"));
                continue;
            }
            (None, Some(_)) => {
                out.push_str(&format!("### `{name}`: only in new\n\n"));
                continue;
            }
            (None, None) => unreachable!(),
        };
        out.push_str(&format!("### `{name}`\n\n"));
        if b.skipped.is_some() || n.skipped.is_some() {
            out.push_str("Skipped in ");
            out.push_str(match (b.skipped.is_some(), n.skipped.is_some()) {
                (true, true) => "base and new",
                (true, false) => "base",
                _ => "new",
            });
            out.push_str(": not compared.\n\n");
            continue;
        }

        let keys: BTreeSet<&String> = b.metrics.keys().chain(n.metrics.keys()).collect();
        if !keys.is_empty() {
            out.push_str(
                "| Metric | base | new | delta | flag |\n| :--- | ---: | ---: | ---: | :--- |\n",
            );
            for key in keys {
                match (b.metrics.get(key), n.metrics.get(key)) {
                    (Some(&bv), Some(&nv)) => {
                        let (flagged, delta) = live_metric_flag(key, bv, nv);
                        if flagged {
                            regressed = true;
                        }
                        out.push_str(&format!(
                            "| `{key}` | {} | {} | {} | {} |\n",
                            fmt_metric(key, bv),
                            fmt_metric(key, nv),
                            delta,
                            if flagged { "REGRESSION" } else { "" }
                        ));
                    }
                    (Some(&bv), None) => out.push_str(&format!(
                        "| `{key}` | {} | only in base | | |\n",
                        fmt_metric(key, bv)
                    )),
                    (None, Some(&nv)) => out.push_str(&format!(
                        "| `{key}` | only in new | {} | | |\n",
                        fmt_metric(key, nv)
                    )),
                    (None, None) => unreachable!(),
                }
            }
            out.push('\n');
        }

        let base_verdicts: BTreeMap<&str, bool> = b
            .verdicts
            .iter()
            .map(|v| (v.check.as_str(), v.pass))
            .collect();
        let mut new_failures = false;
        for v in &n.verdicts {
            if let Some(&was_pass) = base_verdicts.get(v.check.as_str()) {
                if was_pass && !v.pass {
                    regressed = true;
                    new_failures = true;
                    out.push_str(&format!("- NEW FAILURE `{}`: {}\n", v.check, v.detail));
                }
            }
        }
        if new_failures {
            out.push('\n');
        }
    }
    regressed
}

/// `static`: exe size, section raw sizes, `Cargo.lock` package count,
/// imported DLLs.
fn compare_static(out: &mut String, base: Option<&StaticInfo>, new: Option<&StaticInfo>) -> bool {
    let (Some(base), Some(new)) = (base, new) else {
        return false;
    };
    let mut regressed = false;
    out.push_str("## Binary\n\n");

    let (size_flag, size_delta) = worse_above(
        base.exe_size as f64,
        new.exe_size as f64,
        EXE_SIZE_THRESHOLD_PCT,
    );
    if size_flag {
        regressed = true;
    }
    out.push_str(&format!(
        "- exe size: {} -> {} ({}){}\n",
        fmt_bytes(base.exe_size as f64),
        fmt_bytes(new.exe_size as f64),
        size_delta,
        if size_flag { " REGRESSION" } else { "" }
    ));

    let (pkg_flag, _) = drifted(base.lock_packages as f64, new.lock_packages as f64, 0.0);
    if pkg_flag {
        regressed = true;
    }
    out.push_str(&format!(
        "- `Cargo.lock` packages: {} -> {}{}\n\n",
        base.lock_packages,
        new.lock_packages,
        if pkg_flag { " REGRESSION" } else { "" }
    ));

    let base_secs: BTreeMap<&str, &Section> =
        base.sections.iter().map(|s| (s.name.as_str(), s)).collect();
    let new_secs: BTreeMap<&str, &Section> =
        new.sections.iter().map(|s| (s.name.as_str(), s)).collect();
    let names: BTreeSet<&str> = base_secs.keys().chain(new_secs.keys()).copied().collect();
    if !names.is_empty() {
        out.push_str("| Section | base raw | new raw | delta | flag |\n| :--- | ---: | ---: | ---: | :--- |\n");
        for name in names {
            match (base_secs.get(name), new_secs.get(name)) {
                (Some(b), Some(n)) => {
                    let (flagged, delta) = worse_above(
                        b.raw_size as f64,
                        n.raw_size as f64,
                        SECTION_SIZE_THRESHOLD_PCT,
                    );
                    if flagged {
                        regressed = true;
                    }
                    out.push_str(&format!(
                        "| `{name}` | {} | {} | {} | {} |\n",
                        fmt_bytes(b.raw_size as f64),
                        fmt_bytes(n.raw_size as f64),
                        delta,
                        if flagged { "REGRESSION" } else { "" }
                    ));
                }
                (Some(b), None) => out.push_str(&format!(
                    "| `{name}` | {} | - | only in base | |\n",
                    fmt_bytes(b.raw_size as f64)
                )),
                (None, Some(n)) => out.push_str(&format!(
                    "| `{name}` | - | {} | only in new | |\n",
                    fmt_bytes(n.raw_size as f64)
                )),
                (None, None) => unreachable!(),
            }
        }
        out.push('\n');
    }

    let base_dlls: BTreeSet<&str> = base.imports.iter().map(|i| i.dll.as_str()).collect();
    let new_dlls: BTreeSet<&str> = new.imports.iter().map(|i| i.dll.as_str()).collect();
    let new_only: Vec<&&str> = new_dlls.difference(&base_dlls).collect();
    let base_only: Vec<&&str> = base_dlls.difference(&new_dlls).collect();
    if !new_only.is_empty() {
        regressed = true;
        out.push_str("New imported DLLs (a new runtime dependency):\n\n");
        for d in &new_only {
            out.push_str(&format!("- `{d}` REGRESSION\n"));
        }
        out.push('\n');
    }
    if !base_only.is_empty() {
        out.push_str("DLLs no longer imported:\n\n");
        for d in &base_only {
            out.push_str(&format!("- `{d}`\n"));
        }
        out.push('\n');
    }

    regressed
}

/// Markdown and whether anything regressed.
pub fn run(base: &Report, new: &Report) -> (String, bool) {
    let mut out = header(base, new);
    let mut regressed = false;
    regressed |= compare_bench_group(
        &mut out,
        "Micro",
        base.groups.micro.as_ref(),
        new.groups.micro.as_ref(),
    );
    regressed |= compare_bench_group(
        &mut out,
        "Primitives",
        base.groups.primitives.as_ref(),
        new.groups.primitives.as_ref(),
    );
    regressed |= compare_static(
        &mut out,
        base.groups.static_info.as_ref(),
        new.groups.static_info.as_ref(),
    );
    regressed |= compare_live_group(
        &mut out,
        base.groups.live.as_ref(),
        new.groups.live.as_ref(),
    );
    if !regressed {
        out.push_str("No regressions flagged.\n");
    }
    (out, regressed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{DaemonInfo, Import, Verdict};
    use crate::stamp::{Build, Machine};
    use crate::stats::Summary;

    fn entry(name: &str, median_ns: f64) -> BenchEntry {
        BenchEntry {
            name: name.to_string(),
            iters: 1_000,
            samples: 30,
            ns: Summary::of(&[median_ns]),
            cycles: Summary::of(&[median_ns * 3.0]),
        }
    }

    fn report_with_micro(entries: Vec<BenchEntry>) -> Report {
        let mut r = Report::new("micro", Machine::default(), Build::default());
        r.groups.micro = Some(BenchGroup {
            smoke: false,
            entries,
            skipped: vec![],
        });
        r
    }

    #[test]
    fn no_change_flags_nothing() {
        let base = report_with_micro(vec![entry("a", 1_000.0)]);
        let new = report_with_micro(vec![entry("a", 1_000.0)]);
        let (md, regressed) = run(&base, &new);
        assert!(!regressed);
        assert!(!md.contains("REGRESSION"));
    }

    #[test]
    fn a_thirty_percent_increase_is_flagged() {
        let base = report_with_micro(vec![entry("a", 1_000.0)]);
        let new = report_with_micro(vec![entry("a", 1_300.0)]);
        let (md, regressed) = run(&base, &new);
        assert!(regressed);
        assert!(md.contains("REGRESSION"));
    }

    #[test]
    fn an_improvement_is_shown_but_not_flagged() {
        let base = report_with_micro(vec![entry("a", 1_000.0)]);
        let new = report_with_micro(vec![entry("a", 700.0)]);
        let (md, regressed) = run(&base, &new);
        assert!(!regressed);
        assert!(md.contains("better"));
    }

    #[test]
    fn a_verdict_flip_is_flagged() {
        let mut base = Report::new("live", Machine::default(), Build::default());
        base.groups.live = Some(LiveGroup {
            daemon: DaemonInfo::default(),
            scenarios: vec![Scenario {
                name: "idle".into(),
                verdicts: vec![Verdict {
                    check: "floor".into(),
                    pass: true,
                    detail: "ok".into(),
                }],
                ..Default::default()
            }],
        });
        let mut new = base.clone();
        new.groups.live.as_mut().unwrap().scenarios[0].verdicts[0].pass = false;

        let (md, regressed) = run(&base, &new);
        assert!(regressed);
        assert!(md.contains("NEW FAILURE"));
    }

    #[test]
    fn a_new_import_is_flagged() {
        let mut base = Report::new("static", Machine::default(), Build::default());
        base.groups.static_info = Some(StaticInfo {
            imports: vec![Import {
                dll: "KERNEL32.dll".into(),
                functions: 3,
            }],
            ..Default::default()
        });
        let mut new = base.clone();
        new.groups
            .static_info
            .as_mut()
            .unwrap()
            .imports
            .push(Import {
                dll: "D3D11.dll".into(),
                functions: 1,
            });

        let (md, regressed) = run(&base, &new);
        assert!(regressed);
        assert!(md.contains("D3D11.dll"));
        assert!(md.contains("REGRESSION"));
    }

    #[test]
    fn user_wobble_of_one_is_tolerated() {
        let mut base_scenario = Scenario {
            name: "idle".into(),
            ..Default::default()
        };
        base_scenario.metrics.insert("user_floor".into(), 21.0);
        let mut new_scenario = base_scenario.clone();
        new_scenario.metrics.insert("user_floor".into(), 22.0);

        let mut base = Report::new("live", Machine::default(), Build::default());
        base.groups.live = Some(LiveGroup {
            daemon: DaemonInfo::default(),
            scenarios: vec![base_scenario],
        });
        let mut new = Report::new("live", Machine::default(), Build::default());
        new.groups.live = Some(LiveGroup {
            daemon: DaemonInfo::default(),
            scenarios: vec![new_scenario],
        });

        let (_, regressed) = run(&base, &new);
        assert!(!regressed);
    }

    #[test]
    fn a_gdi_change_of_one_is_flagged() {
        let mut base_scenario = Scenario {
            name: "idle".into(),
            ..Default::default()
        };
        base_scenario.metrics.insert("gdi_floor".into(), 19.0);
        let mut new_scenario = base_scenario.clone();
        new_scenario.metrics.insert("gdi_floor".into(), 20.0);

        let mut base = Report::new("live", Machine::default(), Build::default());
        base.groups.live = Some(LiveGroup {
            daemon: DaemonInfo::default(),
            scenarios: vec![base_scenario],
        });
        let mut new = Report::new("live", Machine::default(), Build::default());
        new.groups.live = Some(LiveGroup {
            daemon: DaemonInfo::default(),
            scenarios: vec![new_scenario],
        });

        let (_, regressed) = run(&base, &new);
        assert!(regressed);
    }

    #[test]
    fn different_profiles_get_a_warning() {
        let mut base = Report::new("micro", Machine::default(), Build::default());
        base.build.profile = "release".into();
        let mut new = Report::new("micro", Machine::default(), Build::default());
        new.build.profile = "debug".into();

        let (md, _) = run(&base, &new);
        assert!(md.contains("different profiles"));
    }

    #[test]
    fn different_machines_get_a_warning() {
        let mut base = Report::new("micro", Machine::default(), Build::default());
        base.machine.cpu = "CPU A".into();
        let mut new = Report::new("micro", Machine::default(), Build::default());
        new.machine.cpu = "CPU B".into();

        let (md, _) = run(&base, &new);
        assert!(md.contains("different machines"));
    }
}
