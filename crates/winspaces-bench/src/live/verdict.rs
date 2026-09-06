//! Verdict helpers shared by every scenario: "did a floor return", "did a
//! cache stay flat across cycles". A verdict is never inferred implicitly —
//! every check names itself and carries the before/after numbers a reader
//! would ask for.

use crate::report::Verdict;

/// A floor returned to within `tolerance` (absolute units, same as the
/// values). `docs/benchmarks.md` §4.1: GDI exact, USER ±1, handles ±2,
/// private bytes ≤ +256 KiB.
pub fn floor_tolerance(
    check: &str,
    before: f64,
    after: f64,
    tolerance: f64,
    unit: &str,
) -> Verdict {
    let delta = after - before;
    let pass = delta <= tolerance;
    Verdict {
        check: check.to_string(),
        pass,
        detail: format!(
            "before {before:.0}{unit}, after {after:.0}{unit} (delta {delta:+.0}{unit}, tolerance +{tolerance:.0}{unit})"
        ),
    }
}

/// Exact match (GDI floors, and any handle count with no documented slack).
pub fn floor_exact(check: &str, before: f64, after: f64, unit: &str) -> Verdict {
    floor_tolerance(check, before, after, 0.0, unit)
}

/// A cache-vs-leak check: growth from cycle 2 to the last cycle must be
/// zero. Any positive drift after the first cache step is a leak.
pub fn growth_is_zero(check: &str, growth: f64, unit: &str) -> Verdict {
    Verdict {
        check: check.to_string(),
        pass: growth == 0.0,
        detail: format!("growth over cycles 2..N: {growth:+.0}{unit}"),
    }
}

/// A boolean condition the caller has already evaluated (e.g. "the window
/// appeared", "it closed within N seconds").
pub fn condition(check: &str, pass: bool, detail: impl Into<String>) -> Verdict {
    Verdict {
        check: check.to_string(),
        pass,
        detail: detail.into(),
    }
}

/// The median of the last `n` samples' `field`, or `0.0` when there are
/// fewer than `n` (callers only call this on phases known to be long
/// enough; a short phase yields a plausible-if-noisy number rather than a
/// panic).
pub fn last_n_median(
    samples: &[crate::report::Sample],
    n: usize,
    field: impl Fn(&crate::report::Sample) -> f64,
) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let take = n.min(samples.len());
    let mut values: Vec<f64> = samples[samples.len() - take..].iter().map(field).collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = values.len() / 2;
    if values.len() >= 2 && values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Sample;

    fn sample(gdi: u32) -> Sample {
        Sample {
            gdi,
            ..Default::default()
        }
    }

    #[test]
    fn floor_exact_passes_only_on_no_growth() {
        assert!(floor_exact("x", 19.0, 19.0, "").pass);
        assert!(!floor_exact("x", 19.0, 20.0, "").pass);
        // A floor that *dropped* is not a regression.
        assert!(floor_exact("x", 20.0, 19.0, "").pass);
    }

    #[test]
    fn floor_tolerance_allows_the_documented_slack() {
        assert!(floor_tolerance("user", 21.0, 22.0, 1.0, "").pass);
        assert!(!floor_tolerance("user", 21.0, 23.0, 1.0, "").pass);
    }

    #[test]
    fn last_n_median_of_a_short_series() {
        let samples: Vec<Sample> = vec![sample(1), sample(2), sample(3)];
        // last 5 of 3 -> all 3 -> median 2
        assert_eq!(last_n_median(&samples, 5, |s| s.gdi as f64), 2.0);
        // last 2 of 3 -> [2,3] -> median 2.5
        assert_eq!(last_n_median(&samples, 2, |s| s.gdi as f64), 2.5);
    }

    #[test]
    fn last_n_median_of_empty_is_zero() {
        assert_eq!(last_n_median(&[], 5, |s| s.gdi as f64), 0.0);
    }
}
