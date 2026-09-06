//! `config/*`: the `settings.json` round trip through parse, normalize and
//! serialize — paid on every reload.

use winspaces_common::{Config, WorkspaceRule};

use crate::timing::Runner;

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
        is_sticky: i < 5,
    }
}

fn config_with_rules(n: usize) -> Config {
    Config {
        workspace_rules: (0..n).map(rule).collect(),
        ..Config::default()
    }
}

pub fn bench(r: &mut Runner) {
    let cfg = config_with_rules(50);
    let json = serde_json::to_string_pretty(&cfg).expect("serialize config");

    r.bench("config/parse/rules=50", || {
        let parsed: Config = serde_json::from_str(&json).expect("parse config");
        std::hint::black_box(parsed);
    });

    // `normalize` needs a fresh copy per iteration; `config/clone` prices
    // that copy on its own so the two can be told apart.
    r.bench("config/clone/rules=50", || {
        std::hint::black_box(cfg.clone());
    });

    r.bench("config/normalize", || {
        let mut clone = cfg.clone();
        clone.normalize();
        std::hint::black_box(clone);
    });

    r.bench("config/serialize/rules=50", || {
        let out = serde_json::to_string_pretty(&cfg).expect("serialize config");
        std::hint::black_box(out);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_with_rules_builds_the_requested_count() {
        let cfg = config_with_rules(50);
        assert_eq!(cfg.workspace_rules.len(), 50);
    }

    #[test]
    fn a_smoke_run_covers_every_named_benchmark() {
        let mut r = Runner::new(true, None, false);
        bench(&mut r);
        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        for expected in [
            "config/parse/rules=50",
            "config/clone/rules=50",
            "config/normalize",
            "config/serialize/rules=50",
        ] {
            assert!(names.contains(&expected), "missing benchmark {expected}");
        }
    }
}
