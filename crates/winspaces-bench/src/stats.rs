//! Sample summaries and the unit formatting every report shares.

use serde::{Deserialize, Serialize};

/// Five-number summary of a sample set. `n == 0` means "no data": every
/// field is zero and a renderer prints a dash.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Summary {
    pub n: usize,
    pub min: f64,
    pub median: f64,
    pub mean: f64,
    pub p95: f64,
    pub max: f64,
}

impl Summary {
    pub fn of(samples: &[f64]) -> Summary {
        if samples.is_empty() {
            return Summary::default();
        }
        let mut sorted: Vec<f64> = samples.iter().copied().filter(|v| v.is_finite()).collect();
        if sorted.is_empty() {
            return Summary::default();
        }
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = sorted.len();
        Summary {
            n,
            min: sorted[0],
            median: percentile(&sorted, 50.0),
            mean: sorted.iter().sum::<f64>() / n as f64,
            p95: percentile(&sorted, 95.0),
            max: sorted[n - 1],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}

/// Linear-interpolated percentile over an ascending slice. `p` in 0..=100.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = (p.clamp(0.0, 100.0) / 100.0) * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        return sorted[lo];
    }
    let frac = rank - lo as f64;
    sorted[lo] + (sorted[hi] - sorted[lo]) * frac
}

/// `812 ns`, `1.23 us`, `4.5 ms`. ASCII only: the console code page is not
/// guaranteed to render a micro sign.
pub fn fmt_ns(ns: f64) -> String {
    if ns < 1_000.0 {
        format!("{ns:.0} ns")
    } else if ns < 1_000_000.0 {
        format!("{:.2} us", ns / 1_000.0)
    } else if ns < 1_000_000_000.0 {
        format!("{:.2} ms", ns / 1_000_000.0)
    } else {
        format!("{:.2} s", ns / 1_000_000_000.0)
    }
}

/// `950 cyc`, `1.9 kcyc`, `3.2 Mcyc`.
pub fn fmt_cycles(c: f64) -> String {
    if c < 1_000.0 {
        format!("{c:.0} cyc")
    } else if c < 1_000_000.0 {
        format!("{:.1} kcyc", c / 1_000.0)
    } else if c < 1_000_000_000.0 {
        format!("{:.2} Mcyc", c / 1_000_000.0)
    } else {
        format!("{:.2} Gcyc", c / 1_000_000_000.0)
    }
}

/// `640 B`, `2.4 KiB`, `3.05 MiB`.
pub fn fmt_bytes(b: f64) -> String {
    const KIB: f64 = 1024.0;
    if b < KIB {
        format!("{b:.0} B")
    } else if b < KIB * KIB {
        format!("{:.1} KiB", b / KIB)
    } else if b < KIB * KIB * KIB {
        format!("{:.2} MiB", b / (KIB * KIB))
    } else {
        format!("{:.2} GiB", b / (KIB * KIB * KIB))
    }
}

/// Signed percentage change from `base` to `new`; `None` when `base` is zero.
pub fn pct_change(base: f64, new: f64) -> Option<f64> {
    if base == 0.0 || !base.is_finite() || !new.is_finite() {
        None
    } else {
        Some((new - base) / base * 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_of_known_samples() {
        let s = Summary::of(&[5.0, 1.0, 3.0, 2.0, 4.0]);
        assert_eq!(s.n, 5);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.max, 5.0);
        assert_eq!(s.median, 3.0);
        assert_eq!(s.mean, 3.0);
        assert!((s.p95 - 4.8).abs() < 1e-9);
    }

    #[test]
    fn empty_summary_is_all_zero() {
        let s = Summary::of(&[]);
        assert!(s.is_empty());
        assert_eq!(s.median, 0.0);
    }

    #[test]
    fn percentile_interpolates() {
        let v = [10.0, 20.0, 30.0, 40.0];
        assert_eq!(percentile(&v, 0.0), 10.0);
        assert_eq!(percentile(&v, 100.0), 40.0);
        assert_eq!(percentile(&v, 50.0), 25.0);
    }

    #[test]
    fn units_are_ascii() {
        assert_eq!(fmt_ns(812.0), "812 ns");
        assert_eq!(fmt_ns(1_230.0), "1.23 us");
        assert_eq!(fmt_cycles(1_900.0), "1.9 kcyc");
        assert_eq!(fmt_bytes(3_200_000.0), "3.05 MiB");
        assert!(fmt_ns(2_500_000.0).is_ascii());
    }

    #[test]
    fn pct_change_handles_zero_base() {
        assert_eq!(pct_change(0.0, 5.0), None);
        assert_eq!(pct_change(100.0, 150.0), Some(50.0));
    }
}
