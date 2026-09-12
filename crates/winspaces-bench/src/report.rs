//! The result model (JSON schema 1), its markdown rendering, and where a
//! result is written.
//!
//! One schema for every group so `compare` can diff any two runs. Series
//! are kept whole (every live sample), not only their summaries: a JSON on
//! disk is the record, and a question nobody thought to ask at run time
//! should still be answerable from it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::stamp::{Build, Machine};
use crate::stats::{fmt_bytes, fmt_cycles, fmt_ns, Summary};

pub const SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Report {
    pub schema: u32,
    pub generated_unix: u64,
    pub generated_iso: String,
    /// The command that produced this report (`micro`, `live`, `all`, ...).
    pub command: String,
    pub machine: Machine,
    pub build: Build,
    pub groups: Groups,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Groups {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub micro: Option<BenchGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primitives: Option<BenchGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<LiveGroup>,
    #[serde(default, rename = "static", skip_serializing_if = "Option::is_none")]
    pub static_info: Option<StaticInfo>,
}

/// An in-process group (`micro`, `primitives`).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct BenchGroup {
    pub smoke: bool,
    pub entries: Vec<BenchEntry>,
    /// `(name, reason)` for benchmarks that could not run here.
    #[serde(default)]
    pub skipped: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BenchEntry {
    pub name: String,
    pub iters: u64,
    pub samples: usize,
    /// Wall nanoseconds per iteration.
    pub ns: Summary,
    /// Thread cycles per iteration.
    pub cycles: Summary,
}

// --- live ------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LiveGroup {
    pub daemon: DaemonInfo,
    pub scenarios: Vec<Scenario>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DaemonInfo {
    pub pid: u32,
    pub exe: String,
    pub elevated: bool,
    pub harness_elevated: bool,
    /// The harness started this daemon (`--own`) rather than attaching to
    /// the user's.
    pub owned: bool,
    /// `--quick` divides every phase duration by this.
    pub duration_divisor: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Scenario {
    pub name: String,
    /// The daemon state this scenario started from, in the vocabulary of
    /// `docs/benchmarks.md` §2 (`cold`, `switching`, `overview-warm`, `fully-warm`,
    /// or `unknown` when attached to a daemon whose history is not known).
    pub state_before: String,
    pub phases: Vec<Phase>,
    /// Derived numbers, keyed by a stable name (`floor_cycles_per_s`,
    /// `per_op_cycles_isolated`, `gdi_floor_before`, ...). Flat so `compare`
    /// can diff them without knowing the scenario.
    pub metrics: BTreeMap<String, f64>,
    pub verdicts: Vec<Verdict>,
    /// Why the scenario did not run, when it did not. Never a pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Phase {
    pub name: String,
    pub seconds: f64,
    /// Events driven during the phase (space switches, opens, reloads).
    pub events: u32,
    pub samples: Vec<Sample>,
}

/// One reading of the daemon's counters.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Sample {
    /// Seconds since the phase started.
    pub t: f64,
    /// Process cycles retired since the previous sample.
    pub cycles_delta: u64,
    pub kernel_ms: f64,
    pub user_ms: f64,
    pub private_bytes: u64,
    pub working_set: u64,
    pub peak_working_set: u64,
    pub page_faults: u32,
    pub gdi: u32,
    pub user_objects: u32,
    pub handles: u32,
    pub threads: u32,
    /// `None` when `GetProcessIoCounters` is denied (cross-integrity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_bytes: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Verdict {
    pub check: String,
    pub pass: bool,
    pub detail: String,
}

// --- static ----------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct StaticInfo {
    pub exe_path: String,
    pub exe_size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_exe_size: Option<u64>,
    pub sections: Vec<Section>,
    pub imports: Vec<Import>,
    pub lock_packages: u32,
    /// `[profile.release]` knobs as written in `Cargo.toml`.
    pub profile: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    pub virtual_size: u32,
    pub raw_size: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Import {
    pub dll: String,
    pub functions: u32,
}

// --- construction and IO -----------------------------------------------------

impl Report {
    pub fn new(command: &str, machine: Machine, build: Build) -> Report {
        let generated_unix = winspaces_common::unix_now();
        Report {
            schema: SCHEMA,
            generated_unix,
            generated_iso: iso_utc(generated_unix),
            command: command.to_string(),
            machine,
            build,
            groups: Groups::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Report, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let report: Report =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if report.schema != SCHEMA {
            return Err(format!(
                "{}: schema {} (this tool reads schema {})",
                path.display(),
                report.schema,
                SCHEMA
            ));
        }
        Ok(report)
    }
}

/// The repository root: this crate lives at `<root>/crates/winspaces-bench`.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("crate sits two levels below the repository root")
}

/// `.local/bench/results/<yyyymmdd-hhmmss>-<sha7>-<command>.json`.
pub fn default_out_path(report: &Report) -> PathBuf {
    let stamp = compact_utc(report.generated_unix);
    let sha = if report.build.git_sha.is_empty() {
        "nosha".to_string()
    } else {
        report.build.git_sha.clone()
    };
    repo_root()
        .join(".local")
        .join("bench")
        .join("results")
        .join(format!("{stamp}-{sha}-{}.json", report.command))
}

// --- time formatting (no chrono: civil-from-days, Howard Hinnant) -----------

fn civil_from_unix(unix: u64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = unix as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if mo <= 2 { y + 1 } else { y };
    (y, mo, d, h, m, s)
}

pub fn iso_utc(unix: u64) -> String {
    let (y, mo, d, h, m, s) = civil_from_unix(unix);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub fn compact_utc(unix: u64) -> String {
    let (y, mo, d, h, m, s) = civil_from_unix(unix);
    format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}")
}

// --- markdown ----------------------------------------------------------------

fn dash_or<F: Fn(f64) -> String>(s: &Summary, f: F, field: fn(&Summary) -> f64) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        f(field(s))
    }
}

pub fn render_bench_group(out: &mut String, title: &str, group: &BenchGroup) {
    out.push_str(&format!("## {title}\n\n"));
    if group.smoke {
        out.push_str("Smoke run: one iteration, one sample per benchmark; timings are not representative.\n\n");
    }
    if group.entries.is_empty() {
        out.push_str("No benchmarks ran.\n\n");
    } else {
        out.push_str("| Benchmark | median | min | p95 | cycles (median) | iters |\n");
        out.push_str("| :--- | ---: | ---: | ---: | ---: | ---: |\n");
        let mut entries: Vec<&BenchEntry> = group.entries.iter().collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for e in entries {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} |\n",
                e.name,
                dash_or(&e.ns, fmt_ns, |s| s.median),
                dash_or(&e.ns, fmt_ns, |s| s.min),
                dash_or(&e.ns, fmt_ns, |s| s.p95),
                dash_or(&e.cycles, fmt_cycles, |s| s.median),
                e.iters
            ));
        }
        out.push('\n');
    }
    if !group.skipped.is_empty() {
        out.push_str("Skipped:\n\n");
        let mut skipped: Vec<&(String, String)> = group.skipped.iter().collect();
        skipped.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, reason) in skipped {
            out.push_str(&format!("- `{name}`: {reason}\n"));
        }
        out.push('\n');
    }
}

fn phase_summary(phase: &Phase) -> String {
    if phase.samples.is_empty() {
        return format!(
            "| {} | {:.0} s | {} | - | - | - | - | - |\n",
            phase.name, phase.seconds, phase.events
        );
    }
    let cycles: Vec<f64> = phase
        .samples
        .iter()
        .skip(1)
        .map(|s| s.cycles_delta as f64)
        .collect();
    let c = Summary::of(&cycles);
    let last = phase.samples.last().unwrap();
    format!(
        "| {} | {:.0} s | {} | {} | {} | {} / {} / {} | {} | {} |\n",
        phase.name,
        phase.seconds,
        phase.events,
        if c.is_empty() {
            "-".to_string()
        } else {
            fmt_cycles(c.median)
        },
        if c.is_empty() {
            "-".to_string()
        } else {
            fmt_cycles(c.max)
        },
        last.gdi,
        last.user_objects,
        last.handles,
        fmt_bytes(last.private_bytes as f64),
        last.threads
    )
}

pub fn render_live_group(out: &mut String, live: &LiveGroup) {
    out.push_str("## Live daemon\n\n");
    let d = &live.daemon;
    out.push_str(&format!(
        "Daemon pid {} ({}), {}; harness {}; {}{}\n\n",
        d.pid,
        d.exe,
        if d.elevated {
            "elevated"
        } else {
            "non-elevated"
        },
        if d.harness_elevated {
            "elevated"
        } else {
            "non-elevated"
        },
        if d.owned {
            "started by the harness"
        } else {
            "attached to the running daemon"
        },
        if d.duration_divisor > 1.0 {
            format!("; durations divided by {:.0} (quick)", d.duration_divisor)
        } else {
            String::new()
        }
    ));
    for sc in &live.scenarios {
        out.push_str(&format!("### `{}`", sc.name));
        if !sc.state_before.is_empty() {
            out.push_str(&format!(" (from {})", sc.state_before));
        }
        out.push_str("\n\n");
        if let Some(reason) = &sc.skipped {
            out.push_str(&format!("Skipped: {reason}\n\n"));
            continue;
        }
        if !sc.phases.is_empty() {
            out.push_str("| Phase | Length | Events | cycles/sample median | max | GDI / USER / handles at end | Private at end | Threads |\n");
            out.push_str("| :--- | ---: | ---: | ---: | ---: | :--- | ---: | ---: |\n");
            for p in &sc.phases {
                out.push_str(&phase_summary(p));
            }
            out.push('\n');
        }
        if !sc.metrics.is_empty() {
            out.push_str("| Metric | Value |\n| :--- | ---: |\n");
            for (k, v) in &sc.metrics {
                out.push_str(&format!("| `{k}` | {} |\n", fmt_metric(k, *v)));
            }
            out.push('\n');
        }
        for v in &sc.verdicts {
            out.push_str(&format!(
                "- {} `{}`: {}\n",
                if v.pass { "PASS" } else { "FAIL" },
                v.check,
                v.detail
            ));
        }
        if !sc.verdicts.is_empty() {
            out.push('\n');
        }
        for n in &sc.notes {
            out.push_str(&format!("> {n}\n"));
        }
        if !sc.notes.is_empty() {
            out.push('\n');
        }
    }
}

/// Metric names carry their unit as a suffix so the renderer and `compare`
/// can format them without a registry: `_cycles`, `_bytes`, `_ms`, `_ns`.
pub fn fmt_metric(name: &str, v: f64) -> String {
    if name.ends_with("_bytes") {
        fmt_bytes(v)
    } else if name.contains("cycles") {
        fmt_cycles(v)
    } else if name.ends_with("_ms") {
        format!("{v:.1} ms")
    } else if name.ends_with("_ns") {
        fmt_ns(v)
    } else if v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

pub fn render_static(out: &mut String, s: &StaticInfo) {
    out.push_str("## Binary\n\n");
    out.push_str(&format!(
        "`{}`: {} ({} bytes)",
        s.exe_path,
        fmt_bytes(s.exe_size as f64),
        s.exe_size
    ));
    if let Some(d) = s.debug_exe_size {
        out.push_str(&format!("; debug build {}", fmt_bytes(d as f64)));
    }
    out.push_str(&format!(
        "; {} packages in `Cargo.lock`.\n\n",
        s.lock_packages
    ));
    if !s.profile.is_empty() {
        let knobs: Vec<String> = s
            .profile
            .iter()
            .map(|(k, v)| format!("`{k} = {v}`"))
            .collect();
        out.push_str(&format!("Release profile: {}.\n\n", knobs.join(", ")));
    }
    if !s.sections.is_empty() {
        out.push_str("| Section | Virtual | Raw |\n| :--- | ---: | ---: |\n");
        for sec in &s.sections {
            out.push_str(&format!(
                "| `{}` | {} | {} |\n",
                sec.name,
                fmt_bytes(sec.virtual_size as f64),
                fmt_bytes(sec.raw_size as f64)
            ));
        }
        out.push('\n');
    }
    if !s.imports.is_empty() {
        out.push_str("| Imported DLL | Functions |\n| :--- | ---: |\n");
        for imp in &s.imports {
            out.push_str(&format!("| `{}` | {} |\n", imp.dll, imp.functions));
        }
        out.push('\n');
    }
}

pub fn render_markdown(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!("# WinSpaces benchmark: `{}`\n\n", report.command));
    out.push_str(&format!(
        "{} UTC; {} @ {} MHz, {} logical CPUs; Windows build {}; {} monitor(s): {}; harness {}.\n\n",
        report.generated_iso,
        report.machine.cpu,
        report.machine.mhz,
        report.machine.logical_cpus,
        report.machine.os_build,
        report.machine.monitors.len(),
        report
            .machine
            .monitors
            .iter()
            .map(|m| format!("{}x{} @ {} dpi", m.width, m.height, m.dpi))
            .collect::<Vec<_>>()
            .join(", "),
        if report.machine.harness_elevated { "elevated" } else { "non-elevated" }
    ));
    out.push_str(&format!(
        "Build: v{} at `{}`{}; {} profile; `{}` ({}).\n\n",
        report.build.version,
        if report.build.git_sha.is_empty() {
            "?"
        } else {
            &report.build.git_sha
        },
        if report.build.git_dirty {
            " (dirty)"
        } else {
            ""
        },
        report.build.profile,
        report.build.exe_path,
        if report.build.exe_size > 0 {
            fmt_bytes(report.build.exe_size as f64)
        } else {
            "not built".to_string()
        }
    ));
    if let Some(g) = &report.groups.micro {
        render_bench_group(&mut out, "Micro (pure logic)", g);
    }
    if let Some(g) = &report.groups.primitives {
        render_bench_group(&mut out, "Primitives (Win32 per event)", g);
    }
    if let Some(s) = &report.groups.static_info {
        render_static(&mut out, s);
    }
    if let Some(l) = &report.groups.live {
        render_live_group(&mut out, l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_and_compact_stamps() {
        // 2026-09-06T12:34:56Z
        let unix = 1_788_698_096;
        assert_eq!(iso_utc(unix), "2026-09-06T12:34:56Z");
        assert_eq!(compact_utc(unix), "20260906-123456");
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn report_round_trips_through_json() {
        let mut r = Report::new("micro", Machine::default(), Build::default());
        r.groups.micro = Some(BenchGroup {
            smoke: true,
            entries: vec![BenchEntry {
                name: "a/b".into(),
                iters: 1,
                samples: 1,
                ns: Summary::of(&[5.0]),
                cycles: Summary::of(&[9.0]),
            }],
            skipped: vec![],
        });
        let json = serde_json::to_string(&r).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(back.groups.micro.unwrap().entries[0].name, "a/b");
        assert!(json.contains("\"schema\":1"));
        assert!(!json.contains("\"live\""));
    }

    #[test]
    fn metric_units_follow_the_suffix() {
        assert_eq!(fmt_metric("floor_bytes", 2048.0), "2.0 KiB");
        assert_eq!(fmt_metric("per_op_cycles", 1_500.0), "1.5 kcyc");
        assert_eq!(fmt_metric("time_to_window_ms", 12.34), "12.3 ms");
        assert_eq!(fmt_metric("gdi_floor", 19.0), "19");
    }

    #[test]
    fn markdown_renders_every_group_heading() {
        let mut r = Report::new("all", Machine::default(), Build::default());
        r.groups.micro = Some(BenchGroup::default());
        r.groups.static_info = Some(StaticInfo::default());
        r.groups.live = Some(LiveGroup {
            daemon: DaemonInfo::default(),
            scenarios: vec![Scenario {
                name: "idle".into(),
                skipped: Some("no daemon".into()),
                ..Default::default()
            }],
        });
        let md = render_markdown(&r);
        assert!(md.contains("## Micro"));
        assert!(md.contains("## Binary"));
        assert!(md.contains("## Live daemon"));
        assert!(md.contains("Skipped: no daemon"));
    }
}
