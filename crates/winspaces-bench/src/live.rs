//! Group `live`: drive the running daemon by posting messages and sample
//! its counters through phases, then judge floors and leaks. Protocols and
//! metric naming: `reports/benchmark-suite-plan.md` §4.3 and §5.

mod daemon;
mod driver;
mod sampler;
mod scenarios;
mod verdict;

use std::path::Path;

use sampler::Sampler;
use scenarios::Ctx;
use winspaces_win32::security::is_current_process_elevated;

use crate::report::{DaemonInfo, LiveGroup, Scenario};
use crate::Opts;

/// Tracks the daemon-state label a scenario started from, per
/// `docs/benchmarks.md` §2. Monotonic: once a surface has been exercised in
/// this run, the label never regresses, matching the doc's cumulative
/// "cold -> switching -> mc-warm -> fully-warm" progression.
struct StateTracker {
    rank: u8,
    owned: bool,
}

impl StateTracker {
    fn new(owned: bool) -> StateTracker {
        StateTracker { rank: 0, owned }
    }

    fn label(&self) -> String {
        match self.rank {
            0 if self.owned => "cold".to_string(),
            0 => "unknown".to_string(),
            1 => "switching".to_string(),
            2 => "mc-warm".to_string(),
            _ => "fully-warm".to_string(),
        }
    }

    fn advance(&mut self, name: &str, ran: bool) {
        if !ran {
            return;
        }
        let rank = match name {
            "switch" | "indicator_ab" => 1,
            "mission_control" => 2,
            "menu" => 3,
            _ => 0,
        };
        self.rank = self.rank.max(rank);
    }
}

fn resolve_scenario_list(opts: &Opts) -> Result<Vec<String>, String> {
    let mut names = match &opts.scenarios {
        Some(list) => list.clone(),
        None => {
            let mut defaults: Vec<String> =
                scenarios::DEFAULT.iter().map(|s| s.to_string()).collect();
            if opts.own {
                defaults.insert(0, "startup".to_string());
            }
            defaults
        }
    };
    for name in &names {
        if !scenarios::KNOWN.contains(&name.as_str()) {
            return Err(format!(
                "unknown live scenario {name:?}; known scenarios: {}",
                scenarios::KNOWN.join(", ")
            ));
        }
    }
    // `startup` only ever runs before any daemon is attached; drop it from
    // the list this function hands to the main loop and let the caller
    // dispatch it separately, once, regardless of where it appeared.
    names.retain(|n| n != "startup");
    Ok(names)
}

pub fn run(opts: &Opts, exe: &Path) -> Result<LiveGroup, String> {
    let divisor = if opts.quick { 3.0 } else { 1.0 };
    let wants_startup = match &opts.scenarios {
        Some(list) => list.iter().any(|s| s == "startup"),
        None => opts.own,
    };
    let remaining = resolve_scenario_list(opts)?;

    let owned_guard = if opts.own {
        Some(daemon::Owned::stop_existing(exe)?)
    } else {
        None
    };

    // Ensure the caller's daemon-lifecycle cleanup always runs, on every
    // return path below, including an early error.
    let result = run_inner(opts, exe, divisor, wants_startup, &remaining);

    if let Some(guard) = owned_guard {
        let restored = guard.restore();
        if let Err(e) = restored {
            return Err(match result {
                Ok(_) => e,
                Err(orig) => format!("{orig}; additionally, restoring the daemon failed: {e}"),
            });
        }
    }
    result
}

fn run_inner(
    opts: &Opts,
    exe: &Path,
    divisor: f64,
    wants_startup: bool,
    remaining: &[String],
) -> Result<LiveGroup, String> {
    let harness_elevated = is_current_process_elevated();
    let mut state = StateTracker::new(opts.own);
    let mut results: Vec<Scenario> = Vec::new();

    if wants_startup {
        if opts.own {
            eprintln!("  live  startup");
            let state_before = state.label();
            let scenario = scenarios::run_startup(exe, opts.quick, opts.verbose, state_before);
            state.advance("startup", scenario.skipped.is_none());
            results.push(scenario);
        } else {
            results.push(Scenario {
                name: "startup".to_string(),
                state_before: state.label(),
                phases: Vec::new(),
                metrics: Default::default(),
                verdicts: Vec::new(),
                skipped: Some("needs --own".to_string()),
                notes: Vec::new(),
            });
        }
    } else if opts.own {
        // `--own` without `startup` in the list: still have to start the
        // daemon before anything else can run, just without timing it.
        daemon::spawn_and_wait(exe)?;
    }

    let msgwnd = driver::find_message_window()?;
    let pid = driver::pid_of(msgwnd);
    let daemon_elevated = daemon::is_pid_elevated(pid);
    let exe_path = driver::exe_path_of(pid).unwrap_or_else(|| {
        format!(
            "{} (actual path unavailable: cross-integrity QueryFullProcessImageNameW denied)",
            exe.display()
        )
    });

    let mut sampler = Sampler::new(pid)?;
    // Prime the cycle-time delta so no phase's first sample carries the
    // Sampler's own startup artifact (see sampler.rs).
    sampler.sample(0.0);

    let ctx = Ctx {
        msgwnd,
        quick: opts.quick,
        verbose: opts.verbose,
        harness_elevated,
        daemon_elevated,
    };

    for name in remaining {
        eprintln!("  live  {name}");
        let state_before = state.label();
        let scenario = scenarios::run_one(name, ctx, &mut sampler, opts, state_before);
        state.advance(name, scenario.skipped.is_none());
        results.push(scenario);
    }

    Ok(LiveGroup {
        daemon: DaemonInfo {
            pid,
            exe: exe_path,
            elevated: daemon_elevated,
            harness_elevated,
            owned: opts.own,
            duration_divisor: divisor,
        },
        scenarios: results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_scenario_list_rejects_unknown_names() {
        let opts = Opts {
            scenarios: Some(vec!["idle".to_string(), "bogus".to_string()]),
            ..Default::default()
        };
        assert!(resolve_scenario_list(&opts).is_err());
    }

    #[test]
    fn resolve_scenario_list_defaults_without_own() {
        let opts = Opts::default();
        let names = resolve_scenario_list(&opts).unwrap();
        assert_eq!(
            names,
            vec!["idle", "switch", "mission_control", "menu", "reload"]
        );
    }

    #[test]
    fn resolve_scenario_list_strips_startup_for_the_main_loop() {
        let opts = Opts {
            own: true,
            ..Default::default()
        };
        let names = resolve_scenario_list(&opts).unwrap();
        assert!(!names.contains(&"startup".to_string()));
        assert_eq!(
            names,
            vec!["idle", "switch", "mission_control", "menu", "reload"]
        );
    }

    #[test]
    fn resolve_scenario_list_respects_an_explicit_list() {
        let opts = Opts {
            scenarios: Some(vec!["switch".to_string(), "reload".to_string()]),
            ..Default::default()
        };
        let names = resolve_scenario_list(&opts).unwrap();
        assert_eq!(names, vec!["switch", "reload"]);
    }

    #[test]
    fn state_tracker_progresses_monotonically() {
        let mut st = StateTracker::new(false);
        assert_eq!(st.label(), "unknown");
        st.advance("idle", true);
        assert_eq!(st.label(), "unknown");
        st.advance("switch", true);
        assert_eq!(st.label(), "switching");
        st.advance("mission_control", true);
        assert_eq!(st.label(), "mc-warm");
        st.advance("menu", false); // skipped: does not advance
        assert_eq!(st.label(), "mc-warm");
        st.advance("menu", true);
        assert_eq!(st.label(), "fully-warm");
    }

    #[test]
    fn state_tracker_starts_cold_when_owned() {
        let st = StateTracker::new(true);
        assert_eq!(st.label(), "cold");
    }
}
