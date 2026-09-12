//! Repeated runs, and refusing to report a single number.
//!
//! Every measurement in this crate is a set of *runs*, not a timing. A run is
//! a batch of identical operations, timed together and divided by the count; a
//! measurement is several runs. What gets printed is the median run with the
//! range around it, because on a machine that may have three cargo builds on
//! it the range is the finding as often as the median is.
//!
//! [`Measure::separated_from`] exists so a comparison has to answer the
//! question before it is written down: if two measurements' quartile bands
//! overlap, the difference between them is not a result.

use std::time::Duration;

/// One measurement: the per-operation cost from each of several runs.
#[derive(Debug, Clone)]
pub struct Measure {
    /// What was measured.
    pub label: String,
    /// Per-operation cost from each run, in nanoseconds.
    pub runs: Vec<f64>,
    /// Operations in each run.
    pub ops: usize,
}

impl Measure {
    /// A measurement of `ops` operations per run.
    #[must_use]
    pub fn new(label: impl Into<String>, ops: usize) -> Self {
        Self {
            label: label.into(),
            runs: Vec::new(),
            ops: ops.max(1),
        }
    }

    /// Record a run that took `elapsed` for all of its operations.
    pub fn run(&mut self, elapsed: Duration) {
        #[allow(clippy::cast_precision_loss)]
        self.runs.push(elapsed.as_nanos() as f64 / self.ops as f64);
    }

    /// Record a run whose per-operation cost is already known.
    pub fn run_each(&mut self, per_op: Duration) {
        #[allow(clippy::cast_precision_loss)]
        self.runs.push(per_op.as_nanos() as f64);
    }

    /// The median run.
    #[must_use]
    pub fn median(&self) -> f64 {
        let mut sorted = self.runs.clone();
        sorted.sort_by(f64::total_cmp);
        match sorted.len() {
            0 => f64::NAN,
            n if n % 2 == 1 => sorted.get(n / 2).copied().unwrap_or(f64::NAN),
            n => {
                let a = sorted.get(n / 2 - 1).copied().unwrap_or(f64::NAN);
                let b = sorted.get(n / 2).copied().unwrap_or(f64::NAN);
                f64::midpoint(a, b)
            }
        }
    }

    /// The fastest run.
    #[must_use]
    pub fn min(&self) -> f64 {
        self.runs.iter().copied().fold(f64::INFINITY, f64::min)
    }

    /// The slowest run.
    #[must_use]
    pub fn max(&self) -> f64 {
        self.runs.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }

    /// The range, as a fraction of the median.
    #[must_use]
    pub fn spread(&self) -> f64 {
        let median = self.median();
        if median <= 0.0 {
            return f64::NAN;
        }
        (self.max() - self.min()) / median
    }

    /// The value at `fraction` through the sorted runs, by nearest rank.
    #[must_use]
    pub fn quantile(&self, fraction: f64) -> f64 {
        if self.runs.is_empty() {
            return f64::NAN;
        }
        let mut sorted = self.runs.clone();
        sorted.sort_by(f64::total_cmp);
        #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
        let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
        sorted.get(index).copied().unwrap_or(f64::NAN)
    }

    /// Whether the two measurements have been told apart.
    ///
    /// The test is on quartiles, not on the full range, and the reason is a
    /// measurement this harness got wrong first. Comparing `max` against `min`
    /// means a single run that lost its core to a compiler — a 1.07 ms outlier
    /// among ten runs at 170 µs — declares a genuine 2.9x difference to be
    /// noise. On a machine that is never quiet that rejects almost everything.
    ///
    /// So: separated when one measurement's upper quartile is below the
    /// other's lower quartile. Outliers are still printed, because they are
    /// what the tail looks like and a reader should see them; they no longer
    /// decide whether there is a finding.
    #[must_use]
    pub fn separated_from(&self, other: &Self) -> bool {
        self.quantile(0.75) < other.quantile(0.25) || other.quantile(0.75) < self.quantile(0.25)
    }

    /// `median [min–max]`, in whatever unit reads best.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{:>10} [{} – {}]",
            duration(self.median()),
            duration(self.min()),
            duration(self.max())
        )
    }

    /// One printed line: label, median, range, spread, run count.
    #[must_use]
    pub fn line(&self) -> String {
        format!(
            "{:<46} {}  ±{:>5.1}%  n={}",
            self.label,
            self.summary(),
            self.spread() * 100.0,
            self.runs.len()
        )
    }
}

/// A duration in nanoseconds, printed in the unit that reads best.
#[must_use]
pub fn duration(nanos: f64) -> String {
    if !nanos.is_finite() {
        "—".to_owned()
    } else if nanos < 1_000.0 {
        format!("{nanos:.0} ns")
    } else if nanos < 1_000_000.0 {
        format!("{:.2} µs", nanos / 1_000.0)
    } else if nanos < 1_000_000_000.0 {
        format!("{:.2} ms", nanos / 1_000_000.0)
    } else {
        format!("{:.2} s", nanos / 1_000_000_000.0)
    }
}

/// The difference between two measurements, with an honest verdict attached.
///
/// The verdict is the point. A difference whose quartile bands overlap is
/// reported as "inside noise" rather than as a number, because reporting it as
/// a number is how a harness manufactures a finding.
#[must_use]
pub fn difference(slower: &Measure, faster: &Measure) -> String {
    let delta = slower.median() - faster.median();
    if !slower.separated_from(faster) {
        return format!(
            "{} (inside run-to-run noise, not a finding)",
            duration(delta.abs())
        );
    }
    // Quote the ratio of medians, but only once the quartiles say the two are
    // distinguishable at all.
    let ratio = if faster.median() > 0.0 {
        slower.median() / faster.median()
    } else {
        f64::NAN
    };
    format!("{} ({ratio:.2}x)", duration(delta))
}
