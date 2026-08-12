//! Scoring how specifically a `WorkspaceRule` identifies a window. Pure.

use windows_sys::Win32::Foundation::HWND;
use winspaces_common::WorkspaceRule;

use super::query::{get_process_image_path, get_window_aumid, get_window_class, get_window_title};
use crate::spaces::is_valid_window;

/// # Safety
/// `hwnd` is an opaque Win32 handle; the `query` helpers this calls tolerate
/// a stale or invalid one by failing gracefully.
pub unsafe fn match_rule_for_window(hwnd: HWND, rules: &[WorkspaceRule]) -> Option<WorkspaceRule> {
    if rules.is_empty() || !is_valid_window(hwnd) {
        return None;
    }

    let aumid = get_window_aumid(hwnd).to_lowercase();
    let exe_path = get_process_image_path(hwnd).to_lowercase();
    let class_name = get_window_class(hwnd).to_lowercase();
    let title = get_window_title(hwnd).to_lowercase();

    let mut best_rule: Option<WorkspaceRule> = None;
    let mut best_score = 0;

    for rule in rules {
        if let Some(score) = score_rule(&aumid, &exe_path, &class_name, &title, rule) {
            if score > best_score {
                best_score = score;
                best_rule = Some(rule.clone());
            }
        }
    }

    best_rule
}

/// Score how specifically `rule` identifies a window with the given
/// (lowercased) attributes. Returns `None` when any matcher the rule
/// specifies disagrees with the window.
pub fn score_rule(
    aumid: &str,
    exe_path: &str,
    class_name: &str,
    title: &str,
    rule: &WorkspaceRule,
) -> Option<i32> {
    let rule_aumid = rule.aumid.to_lowercase();
    let rule_exe = rule.exe_path.to_lowercase();
    let rule_class = rule.class_name.to_lowercase();
    let rule_title = rule.title_pattern.to_lowercase();

    let exe_name = std::path::Path::new(exe_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let rule_exe_name = std::path::Path::new(&rule_exe)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let mut score = 0;

    // 1. AUMID Match (Highest specificity: 100 points!). Exact equality only:
    // AUMIDs are identifiers, and Chromium browsers use "Brave" for the
    // default profile and "Brave.<profile>" for others — a substring match
    // makes the default-profile rule swallow every profile's windows.
    if !rule_aumid.is_empty() {
        if !aumid.is_empty() && aumid == rule_aumid {
            score += 100;
        } else {
            return None;
        }
    }

    // 2. Executable Match (20 points)
    if !rule_exe.is_empty() {
        let matches_exe = exe_path == rule_exe
            || exe_path.ends_with(&rule_exe)
            || (!exe_name.is_empty() && exe_name == rule_exe_name);
        if matches_exe {
            score += 20;
        } else {
            return None;
        }
    }

    // 3. Class Match (10 points)
    if !rule_class.is_empty() {
        if class_name == rule_class {
            score += 10;
        } else {
            return None;
        }
    }

    // 4. Title Pattern Match (30 points). One direction only: matching
    // "rule contains window title" lets a window with a short transient
    // title (e.g. "w") match nearly any rule.
    if !rule_title.is_empty() {
        if title.contains(&rule_title) {
            score += 30;
        } else {
            return None;
        }
    }

    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aumid_matching_is_exact_across_browser_profiles() {
        // Chromium: default profile AUMID is "Brave", other profiles are
        // "Brave.<profile>". The Personal rule must not claim Work windows.
        let personal = WorkspaceRule {
            name: "Brave Personal".into(),
            aumid: "Brave".into(),
            exe_path: r"C:\brave\brave.exe".into(),
            display_index: 0,
            space_index: 0,
            ..Default::default()
        };
        let work = WorkspaceRule {
            name: "Brave Work".into(),
            aumid: "Brave.Profile1".into(),
            exe_path: r"C:\brave\brave.exe".into(),
            display_index: 0,
            space_index: 3,
            ..Default::default()
        };

        let exe = r"c:\brave\brave.exe";
        // Work-profile window: only the Work rule may match.
        assert_eq!(score_rule("brave.profile1", exe, "", "", &personal), None);
        assert!(score_rule("brave.profile1", exe, "", "", &work).is_some());
        // Personal-profile window: only the Personal rule may match.
        assert!(score_rule("brave", exe, "", "", &personal).is_some());
        assert_eq!(score_rule("brave", exe, "", "", &work), None);
    }

    #[test]
    fn rule_with_aumid_rejects_window_without_one() {
        let rule = WorkspaceRule {
            aumid: "Brave".into(),
            ..Default::default()
        };
        assert_eq!(score_rule("", "", "", "", &rule), None);
    }

    #[test]
    fn title_pattern_matches_one_direction_only() {
        let rule = WorkspaceRule {
            exe_path: r"C:\apps\chrome.exe".into(),
            title_pattern: "gmail".into(),
            ..Default::default()
        };
        let exe = r"c:\apps\chrome.exe";
        assert!(score_rule("", exe, "", "gmail - google chrome", &rule).is_some());
        // Window title being a substring of the pattern must NOT match.
        assert_eq!(score_rule("", exe, "", "g", &rule), None);
    }
}
