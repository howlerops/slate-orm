//! What build produced a number.
//!
//! # Why this exists
//!
//! `POINT_READ_COST` was recorded as 3.0, re-measured at 1.0 by #269, and sat
//! for nine tasks with a comment saying *"what changed since the recorded runs
//! is not known"* beside three guesses. #278 answered it: the recorded run had
//! SlateDB's block cache **compiled out** — `slatedb/foyer` absent, so
//! `default_db_cache()` returned a cache that answers `None` to every lookup
//! and discards every insert. Same code, same fixture, same machine, same row
//! count. A cargo feature moved a planner constant by 3x, and every number it
//! changed looked exactly like a number taken with the cache in.
//!
//! The figures were never wrong. The *records* were incomplete, and the same
//! was true of every other measurement here: a table of rows-per-second with
//! no profile beside it cannot be read against a later one, and the reader
//! cannot tell.
//!
//! So a program that measures anything prints this line, and a table that
//! records what it printed carries the line with it.
//!
//! # What it deliberately does not carry
//!
//! **The commit.** Every measurement in this repository is recorded in a
//! `ledger/` entry, which is committed with the change it describes, so the
//! code version is already recoverable from the entry. Reading `git` in a
//! build script would also make every build depend on a `.git` that release
//! tarballs do not have.
//!
//! **The machine.** CPU model and core count belong to the run, not the
//! build, and the harnesses that care already print them. This answers "what
//! was compiled", which is the question nothing could answer.

use slate_kernel::build::Build;
use std::fmt;

/// Every feature this crate declares, and whether it is on in this build.
///
/// Written out rather than discovered, because `cfg!` cannot enumerate: it
/// answers only for a name the author already thought of, which is exactly
/// the failure this module exists to stop. `scripts/check_build_stamp.py`
/// compares these names against `[features]` in `Cargo.toml` and fails if
/// they have drifted, so the list is hand-written but not hand-*maintained* —
/// a feature added without a line here is a red build, not a quiet omission.
pub const FEATURES: &[(&str, bool)] = &[
    ("aws", cfg!(feature = "aws")),
    ("cache", cfg!(feature = "cache")),
    ("dhat-heap", cfg!(feature = "dhat-heap")),
];

/// Features whose absence has been *measured* to change what a number means,
/// with what it did. A stamp naming one of these says so loudly.
///
/// This is not "important features". It is the ones where turning the feature
/// off silently produces a plausible, wrong measurement — the thing that cost
/// nine tasks. `cache` is here because #278 measured 3.05 object-store GETs
/// per point read without it against 1.04 with it, on one fixture at 200,000
/// rows, and nothing in the output said which build it was.
pub const LOAD_BEARING: &[(&str, &str)] = &[(
    "cache",
    "SlateDB's block cache is compiled out; point reads cost ~3x the \
         object-store requests they do in a default build (#278)",
)];

// `dhat-heap` is deliberately NOT here, and the reason is worth keeping.
// It was, for about an hour, and then `scan_tuning` was run: the warning
// fired above a table of scan timings, naming a program the run has nothing
// to do with. A warning that appears where it does not apply is not a
// cautious warning, it is noise that teaches a reader to skip the line —
// and the line it would teach them to skip is the `cache` one, which is the
// whole point. A feature belongs here when its absence changes what *any*
// measurement means; `dhat-heap` changes what one program's means, so
// `row_footprint` says so itself, next to its own zeroes.

/// The build that produced whatever this program is about to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    /// This crate's version.
    pub version: &'static str,
    /// The profile, optimisation level and target, which belong to the cargo
    /// invocation and so are the kernel's to report.
    pub build: Build,
    /// The `slatedb` version actually resolved, read from `Cargo.lock` at
    /// build time — not the range this crate asks for.
    pub slatedb: &'static str,

    /// Every stamped dependency and its resolved version, as
    /// `slatedb 0.16.0, foyer 0.22.3, object_store 0.14.1, tokio 1.53.1`.
    ///
    /// Four rather than one since #285. #269 offered three guesses for the
    /// 3.0 figure it could not explain — a SlateDB release, a block size, the
    /// readahead — and the code behind all three lives in `slatedb` and
    /// `foyer`. `foyer` is the cache whose *absence* was the whole of #278;
    /// a different version of it is as worth recording as none of it.
    pub deps: &'static str,
}

/// The build that produced this binary.
pub const fn stamp() -> Stamp {
    Stamp {
        version: env!("CARGO_PKG_VERSION"),
        build: slate_kernel::build::build(),
        slatedb: env!("SLATE_BUILD_SLATEDB"),
        deps: env!("SLATE_BUILD_DEPS"),
    }
}

impl Stamp {
    /// The features that are on, in declaration order.
    pub fn on(&self) -> Vec<&'static str> {
        FEATURES
            .iter()
            .filter(|(_, on)| *on)
            .map(|(name, _)| *name)
            .collect()
    }

    /// The features that are off.
    pub fn off(&self) -> Vec<&'static str> {
        FEATURES
            .iter()
            .filter(|(_, on)| !*on)
            .map(|(name, _)| *name)
            .collect()
    }

    /// Warnings for every load-bearing feature this build is missing.
    ///
    /// Returned rather than printed so a caller can put them where they will
    /// be read — `cost_calibration` puts them above its table, not below it.
    pub fn warnings(&self) -> Vec<String> {
        // The profile warning comes first because it invalidates more: a
        // debug build makes every wall-clock number here incomparable, while
        // a missing feature changes one family of them.
        let mut warnings = slate_kernel::build::warnings();
        warnings.extend(missing(&self.off()));
        warnings
    }
}

impl fmt::Display for Stamp {
    /// One line, prefixed `build:`, which is what
    /// `scripts/check_build_stamp.py` requires of a measuring program, and
    /// what a reader scans for in a wall of benchmark output.
    ///
    /// Nothing checks that a *recorded* table kept the line. The guard
    /// covers the programs that emit it, not the documents that quote it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "build: slate-slatedb {} | features {} | off {} | {} | {}",
            self.version,
            join(&self.on()),
            join(&self.off()),
            self.build,
            self.deps,
        )
    }
}

/// A warning for every load-bearing feature in `off`.
///
/// Split out from [`Stamp::warnings`] so a test can pass the one set this
/// binary can never be in. A test binary is one build, and that build has
/// `cache` on, so the interesting arm — the cacheless build that cost nine
/// tasks — is unreachable from inside it. The first version of the test
/// re-implemented this filter over `LOAD_BEARING` instead, and a mutation
/// that made the real filter yield nothing survived it: the test was
/// checking its own copy. That is #212's finding in a new place.
pub fn missing(off: &[&str]) -> Vec<String> {
    LOAD_BEARING
        .iter()
        .filter(|(name, _)| off.contains(name))
        .map(|(name, effect)| format!("`{name}` is OFF: {effect}"))
        .collect()
}

/// `a, b` — or `none`, which reads better in the line above than an empty gap
/// and, unlike one, cannot be mistaken for a truncated record.
fn join(names: &[&str]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

/// Print the stamp, and any load-bearing feature that is missing.
///
/// Every program in this repository that measures something calls this before
/// its first number; `scripts/check_build_stamp.py` fails if one does not.
pub fn announce() {
    let mut out = std::io::stdout();
    announce_to(&mut out);
}

/// The stamp, written wherever the caller needs it. See
/// [`slate_kernel::build::announce_to`] for why a daemon wants stderr.
pub fn announce_to(out: &mut impl std::io::Write) {
    let _ = writeln!(out, "{}", stamp());
    for warning in stamp().warnings() {
        let _ = writeln!(out, "!! {warning}");
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::expect_used,
    reason = "a test asserts, and asserting panics"
)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_populated_by_the_build_script() {
        let stamp = stamp();
        // `unknown` is the build script's fail-soft answer. It is correct
        // behaviour there and useless here, so if the environment stopped
        // arriving this says so rather than printing a line of nothing.
        assert_ne!(stamp.build.profile, "unknown", "PROFILE did not arrive");
        assert_ne!(stamp.build.target, "unknown", "TARGET did not arrive");
    }

    #[test]
    fn the_resolved_slatedb_version_is_read_from_the_lock() {
        // The point of reading the lock rather than the `0.16` range in
        // Cargo.toml is that it resolves to a patch. A range would not.
        let slatedb = stamp().slatedb;
        assert_ne!(slatedb, "unknown", "Cargo.lock was not found or not parsed");
        assert_eq!(
            slatedb.split('.').count(),
            3,
            "want a resolved x.y.z, got {slatedb:?}"
        );
    }

    #[test]
    fn every_stamped_dependency_resolves_to_a_version() {
        // Not just "the string is non-empty". Each of the four has to name a
        // real `x.y.z` from the lock, because the failure this guards against
        // is a name silently resolving to `unknown` — which is what a typo in
        // `STAMPED`, or a dependency renamed upstream, produces. An `unknown`
        // is honest in the output and useless in a record.
        let deps = stamp().deps;
        for name in ["slatedb", "foyer", "object_store", "tokio"] {
            let found = deps
                .split(", ")
                .find_map(|pair| pair.strip_prefix(&format!("{name} ")))
                .unwrap_or_else(|| panic!("{name} is not in the stamp: {deps:?}"));
            assert_ne!(found, "unknown", "{name} did not resolve in {deps:?}");
            assert_eq!(
                found.split('.').count(),
                3,
                "want a resolved x.y.z for {name}, got {found:?}"
            );
        }
    }

    #[test]
    fn a_duplicated_dependency_is_reported_not_guessed_at() {
        // Twenty-six packages in this lock resolve to two versions. None of
        // the four does today, so this asserts the *rule* rather than the
        // tree: the parser joins them, and nothing here silently takes the
        // first. A version picked arbitrarily and presented as fact is the
        // exact failure the build stamp exists to prevent.
        let deps = stamp().deps;
        for pair in deps.split(", ") {
            let version = pair.split_once(' ').expect("`name version`").1;
            assert!(
                !version.is_empty(),
                "an empty version in {deps:?} means a package block was read wrong"
            );
        }
        assert_eq!(deps.split(", ").count(), 4, "want four deps, got {deps:?}");
    }

    #[test]
    fn the_test_build_has_the_cache_and_says_so() {
        // Guards the default itself. `default = ["aws", "cache"]` is what
        // makes POINT_READ_COST = 1.0 the right constant for the build that
        // ships; if the default ever loses `cache` again, the constant is
        // wrong and this fails rather than the planner quietly costing point
        // reads at a third of what they take.
        assert!(
            stamp().on().contains(&"cache"),
            "a default build has the block cache; without it POINT_READ_COST \
             is ~3, not 1 (#278). Features on: {:?}",
            stamp().on()
        );
        // Not `warnings().is_empty()`: a debug `cargo test` warns about the
        // profile, correctly, and asserting no warnings at all would make
        // this test mean "and also we are release", which it is not.
        assert!(
            !stamp()
                .warnings()
                .iter()
                .any(|w| w.contains("`cache` is OFF")),
            "warnings: {:?}",
            stamp().warnings()
        );
    }

    #[test]
    fn a_missing_load_bearing_feature_is_warned_about() {
        // Calls the real filter with the one set this build cannot be in,
        // rather than re-deriving it: the test binary has `cache` on, so the
        // cacheless arm is unreachable from inside it, and a test that
        // rebuilt the filter would pass against a broken one. It did, until
        // a mutation said so.
        let warnings = missing(&["cache"]);
        assert_eq!(warnings.len(), 1, "every load-bearing feature warns");
        let first = warnings.first().map(String::as_str).unwrap_or_default();
        assert!(first.contains("block cache is compiled out"));
        assert!(first.contains("3x"));
    }

    #[test]
    fn a_build_missing_nothing_load_bearing_warns_about_nothing() {
        assert!(missing(&[]).is_empty());
        // A feature that is off but not load-bearing is recorded in the line
        // and not warned about. `dhat-heap` is the real instance, and the
        // reason it is not in LOAD_BEARING is written where the list is.
        assert!(missing(&["dhat-heap"]).is_empty());
    }

    #[test]
    fn the_line_names_every_feature_exactly_once() {
        let line = stamp().to_string();
        assert!(line.starts_with("build: slate-slatedb "));
        for (name, _) in FEATURES {
            assert_eq!(
                line.matches(name).count(),
                1,
                "{name} should appear once in {line:?}"
            );
        }
    }
}
