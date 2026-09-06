//! `winspaces-bench`: the benchmark suite. A console tool built by
//! `dev bench`, never shipped. What each group measures, the scenario
//! protocols and how to read a result: `docs/benchmarks.md`.
//!
//! Exit codes: 0 on success, 1 on a usage or runtime error, 2 when `compare`
//! flagged a regression (so a script can gate on it).

mod compare;
mod live;
mod micro;
mod primitives;
mod report;
mod stamp;
mod static_info;
mod stats;
mod timing;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use report::{BenchGroup, Report};
use stamp::{default_daemon_exe, Build, Machine};
use timing::Runner;

/// Everything the command line can say. Hand-parsed: a handful of flags
/// does not justify an argument-parsing dependency in this workspace.
#[derive(Debug, Default, Clone)]
pub struct Opts {
    pub command: String,
    pub positional: Vec<String>,
    /// `--filter <substr>`: run only benchmarks whose name contains it.
    pub filter: Option<String>,
    /// `--smoke`: one iteration, one sample; proves the suite runs.
    pub smoke: bool,
    /// `--quick`: divide live phase durations by 3.
    pub quick: bool,
    /// `--out <file>`: where the JSON goes (a `.md` sibling is always written).
    pub out: Option<PathBuf>,
    /// `--md <file>`: `compare`/`report` markdown destination.
    pub md: Option<PathBuf>,
    /// `--scenario a,b`: live scenarios to run (default set when absent).
    pub scenarios: Option<Vec<String>>,
    /// `--own`: start the daemon instead of attaching to the running one.
    pub own: bool,
    /// `--exe <path>`: the daemon exe for `--own`, `static` and the stamp.
    pub exe: Option<PathBuf>,
    /// `--allow-disruptive`: run scenarios that change the user's desktop
    /// beyond a space switch (tiling toggle).
    pub allow_disruptive: bool,
    /// `--allow-config-edit`: run scenarios that rewrite `settings.json`
    /// (indicator A/B). The file is restored afterwards.
    pub allow_config_edit: bool,
    /// `--minutes <n>`: length of the `soak` scenario.
    pub minutes: Option<f64>,
    /// `--verbose`: print every sample as it is taken.
    pub verbose: bool,
}

const USAGE: &str = "\
winspaces-bench <command> [options]

Commands:
  micro        pure-logic benchmarks (tiling, matching, layout, config, i18n)
  primitives   the Win32 calls the daemon pays per event (probes, GDI, IO)
  static       the binary: size, PE sections and imports, dependency count
  live         drive the running daemon and sample its counters per scenario
  all          micro + primitives + static + live (default scenarios)
  smoke        micro + primitives + static, one iteration each (used by dev check)
  compare <base.json> <new.json>   diff two results, flag regressions (exit 2)
  report <result.json>             render a result's markdown again
  help

Options:
  --filter <substr>     only benchmarks whose name contains <substr>
  --smoke               one iteration, one sample
  --quick               live: divide phase durations by 3
  --scenario <a,b,..>   live: idle, switch, mission_control, menu, reload,
                        startup (needs --own), indicator_ab (needs
                        --allow-config-edit), tiling (needs --allow-disruptive),
                        soak (--minutes)
  --own                 live: stop the running daemon, start --exe, restore after
  --exe <path>          daemon exe (default target\\release\\winspaces.exe)
  --allow-disruptive    live: permit the tiling scenario
  --allow-config-edit   live: permit scenarios that rewrite settings.json
  --minutes <n>         live: soak length
  --out <file.json>     result path (default .local\\bench\\results\\<stamp>-<sha>-<cmd>.json)
  --md <file.md>        compare/report: markdown destination
  --verbose             print every live sample
";

fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut opts = Opts {
        command: args.first().cloned().unwrap_or_else(|| "help".to_string()),
        ..Default::default()
    };
    let mut i = 1;
    let value = |i: &mut usize, flag: &str| -> Result<String, String> {
        *i += 1;
        args.get(*i)
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    };
    while i < args.len() {
        match args[i].as_str() {
            "--filter" => opts.filter = Some(value(&mut i, "--filter")?),
            "--smoke" => opts.smoke = true,
            "--quick" => opts.quick = true,
            "--out" => opts.out = Some(PathBuf::from(value(&mut i, "--out")?)),
            "--md" => opts.md = Some(PathBuf::from(value(&mut i, "--md")?)),
            "--scenario" | "--scenarios" => {
                opts.scenarios = Some(
                    value(&mut i, "--scenario")?
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect(),
                )
            }
            "--own" => opts.own = true,
            "--exe" => opts.exe = Some(PathBuf::from(value(&mut i, "--exe")?)),
            "--allow-disruptive" => opts.allow_disruptive = true,
            "--allow-config-edit" => opts.allow_config_edit = true,
            "--minutes" => {
                opts.minutes = Some(
                    value(&mut i, "--minutes")?
                        .parse()
                        .map_err(|_| "--minutes needs a number".to_string())?,
                )
            }
            "--verbose" | "-v" => opts.verbose = true,
            "-h" | "--help" => opts.command = "help".to_string(),
            other if other.starts_with('-') => return Err(format!("unknown option {other}")),
            other => opts.positional.push(other.to_string()),
        }
        i += 1;
    }
    Ok(opts)
}

fn run_group(opts: &Opts, smoke: bool, f: fn(&mut Runner)) -> BenchGroup {
    let mut runner = Runner::new(smoke, opts.filter.clone(), opts.verbose);
    f(&mut runner);
    BenchGroup {
        smoke,
        entries: runner.entries,
        skipped: runner.skipped,
    }
}

fn daemon_exe(opts: &Opts) -> PathBuf {
    opts.exe.clone().unwrap_or_else(default_daemon_exe)
}

fn write_result(opts: &Opts, report: &Report) -> Result<(), String> {
    let out = opts
        .out
        .clone()
        .unwrap_or_else(|| report::default_out_path(report));
    report.save(&out)?;
    let md = report::render_markdown(report);
    let md_path = out.with_extension("md");
    std::fs::write(&md_path, &md).map_err(|e| format!("{}: {e}", md_path.display()))?;
    print!("{md}");
    eprintln!("\nWrote {} and {}", out.display(), md_path.display());
    Ok(())
}

fn run_measure(opts: &Opts) -> Result<ExitCode, String> {
    let exe = daemon_exe(opts);
    let mut report = Report::new(&opts.command, Machine::detect(), Build::detect(&exe));
    match opts.command.as_str() {
        "micro" => report.groups.micro = Some(run_group(opts, opts.smoke, micro::run)),
        "primitives" => {
            report.groups.primitives = Some(run_group(opts, opts.smoke, primitives::run))
        }
        "static" => report.groups.static_info = Some(static_info::collect(&exe)?),
        "live" => report.groups.live = Some(live::run(opts, &exe)?),
        "all" => {
            report.groups.micro = Some(run_group(opts, opts.smoke, micro::run));
            report.groups.primitives = Some(run_group(opts, opts.smoke, primitives::run));
            report.groups.static_info = Some(static_info::collect(&exe)?);
            report.groups.live = Some(live::run(opts, &exe)?);
        }
        "smoke" => {
            report.groups.micro = Some(run_group(opts, true, micro::run));
            report.groups.primitives = Some(run_group(opts, true, primitives::run));
            report.groups.static_info = static_info::collect(&exe).ok();
        }
        other => return Err(format!("unknown command {other}\n\n{USAGE}")),
    }
    write_result(opts, &report)?;
    Ok(ExitCode::SUCCESS)
}

fn run_compare(opts: &Opts) -> Result<ExitCode, String> {
    let [base, new] = opts.positional.as_slice() else {
        return Err("compare needs exactly two result files".to_string());
    };
    let base = Report::load(Path::new(base))?;
    let new = Report::load(Path::new(new))?;
    let (md, regressed) = compare::run(&base, &new);
    print!("{md}");
    if let Some(path) = &opts.md {
        std::fs::write(path, &md).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(if regressed {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}

fn run_report(opts: &Opts) -> Result<ExitCode, String> {
    let [path] = opts.positional.as_slice() else {
        return Err("report needs one result file".to_string());
    };
    let report = Report::load(Path::new(path))?;
    let md = report::render_markdown(&report);
    print!("{md}");
    if let Some(path) = &opts.md {
        std::fs::write(path, &md).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    // Monitor rects and DPI must be read the way the daemon reads them.
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse_args(&args) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::from(1);
        }
    };
    let result = match opts.command.as_str() {
        "help" => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "compare" => run_compare(&opts),
        "report" => run_report(&opts),
        _ => run_measure(&opts),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_flags_and_positionals() {
        let o = parse_args(&args(
            "live --quick --scenario idle,switch --own --exe C:\\x.exe --minutes 2.5 -v",
        ))
        .unwrap();
        assert_eq!(o.command, "live");
        assert!(o.quick && o.own && o.verbose);
        assert_eq!(o.scenarios.unwrap(), vec!["idle", "switch"]);
        assert_eq!(o.exe.unwrap(), PathBuf::from("C:\\x.exe"));
        assert_eq!(o.minutes, Some(2.5));

        let o = parse_args(&args("compare a.json b.json --md out.md")).unwrap();
        assert_eq!(o.positional, vec!["a.json", "b.json"]);
        assert_eq!(o.md.unwrap(), PathBuf::from("out.md"));
    }

    #[test]
    fn rejects_unknown_and_valueless_flags() {
        assert!(parse_args(&args("micro --bogus")).is_err());
        assert!(parse_args(&args("micro --filter")).is_err());
        assert!(parse_args(&args("live --minutes x")).is_err());
    }

    #[test]
    fn no_arguments_means_help() {
        assert_eq!(parse_args(&[]).unwrap().command, "help");
    }
}
