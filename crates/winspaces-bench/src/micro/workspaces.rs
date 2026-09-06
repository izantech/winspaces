//! `workspaces/*`: rule specificity scoring, the hot path every window
//! activation and restore assignment runs.

use winspaces_common::WorkspaceRule;
use winspaces_core::workspaces::score_rule;

use crate::timing::Runner;

fn rule(name: &str, aumid: &str, exe: &str, class: &str, title: &str) -> WorkspaceRule {
    WorkspaceRule {
        name: name.to_string(),
        aumid: aumid.to_string(),
        exe_path: exe.to_string(),
        class_name: class.to_string(),
        title_pattern: title.to_string(),
        ..WorkspaceRule::default()
    }
}

pub fn bench(r: &mut Runner) {
    bench_score_rule(r);
    bench_match_rules(r);
}

fn bench_score_rule(r: &mut Runner) {
    let hit_rule = rule(
        "hit",
        "app.aumid",
        "c:\\apps\\app.exe",
        "appwindowclass",
        "inbox",
    );
    r.bench("workspaces/score_rule/hit", || {
        let score = score_rule(
            "app.aumid",
            "c:\\apps\\app.exe",
            "appwindowclass",
            "inbox - app",
            &hit_rule,
        );
        std::hint::black_box(score);
    });

    // The rule's executable differs from the window's: `score_rule` rejects
    // at the executable check, the common miss walking a large rule set.
    let miss_rule = rule("miss", "", "c:\\apps\\other.exe", "appwindowclass", "");
    r.bench("workspaces/score_rule/miss", || {
        let score = score_rule("", "c:\\apps\\app.exe", "appwindowclass", "", &miss_rule);
        std::hint::black_box(score);
    });
}

fn bench_match_rules(r: &mut Runner) {
    // 50 rules, 49 near-misses (a differing exe rejects immediately) and one
    // hit on every matcher — mirrors the loop `match_rule_for_window` runs.
    let mut rules: Vec<WorkspaceRule> = (0..49)
        .map(|i| {
            rule(
                &format!("rule-{i}"),
                "",
                &format!("c:\\apps\\other-{i}.exe"),
                "",
                "",
            )
        })
        .collect();
    rules.push(rule(
        "the-hit",
        "app.aumid",
        "c:\\apps\\app.exe",
        "appwindowclass",
        "inbox",
    ));

    let aumid = "app.aumid";
    let exe = "c:\\apps\\app.exe";
    let class = "appwindowclass";
    let title = "inbox - app";

    r.bench("workspaces/match_rules/rules=50", || {
        let mut best_score = 0;
        let mut best: Option<&WorkspaceRule> = None;
        for candidate in &rules {
            if let Some(score) = score_rule(aumid, exe, class, title, candidate) {
                if score > best_score {
                    best_score = score;
                    best = Some(candidate);
                }
            }
        }
        std::hint::black_box(best);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_rule_scores_every_matcher() {
        let hit_rule = rule(
            "hit",
            "app.aumid",
            "c:\\apps\\app.exe",
            "appwindowclass",
            "inbox",
        );
        let score = score_rule(
            "app.aumid",
            "c:\\apps\\app.exe",
            "appwindowclass",
            "inbox - app",
            &hit_rule,
        );
        assert_eq!(score, Some(100 + 20 + 10 + 30));
    }

    #[test]
    fn a_smoke_run_covers_every_named_benchmark() {
        let mut r = Runner::new(true, None, false);
        bench(&mut r);
        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        for expected in [
            "workspaces/score_rule/hit",
            "workspaces/score_rule/miss",
            "workspaces/match_rules/rules=50",
        ] {
            assert!(names.contains(&expected), "missing benchmark {expected}");
        }
    }
}
