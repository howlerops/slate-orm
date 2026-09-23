//! What build produced a number.
//!
//! # Why this exists
//!
//! `POINT_READ_COST` was recorded as 3.0, re-measured at 1.0 by #269, and sat
//! for nine tasks under a comment saying *"what changed since the recorded
//! runs is not known"* beside three guesses. #278 answered it: the recorded
//! run had SlateDB's block cache **compiled out** — a cargo feature. Same
//! code, same fixture, same machine, same row count, and a planner constant
//! 3x out.
//!
//! The figures were never wrong; the *records* were incomplete. That was true
//! of every measurement here, not just that one. A survey of `docs/` and
//! `site/` for #280 found **82 recorded tables and output blocks, of which
//! two record the cargo features they were taken under** — and those two are
//! the write-up of this very defect. Six record release-versus-debug. None
//! records a commit.
//!
//! Debug versus release is not a footnote in this repository:
//! `scripts/run_examples.sh` builds and runs **debug** binaries, while every
//! example's own doc comment and every command in `docs/performance.md` say
//! `cargo run --release`. Both produce a table of plausible numbers, and
//! until now nothing in either transcript said which one you were reading.
//!
//! So: a program that measures anything prints this line first, and a table
//! that records what it printed carries the line with it.
//!
//! # What it deliberately does not carry
//!
//! **The commit.** Every measurement here is written up in a `ledger/` entry
//! committed alongside the change, so the code version is recoverable from
//! the entry. Reading `git` in a build script would also make the build
//! depend on a `.git` that a release tarball does not have.
//!
//! **The machine.** CPU and load belong to the run, not the build, and the
//! harnesses that care already print them (`docs/performance.md` quotes
//! machine load on about a dozen tables). This answers "what was compiled",
//! which is the question nothing could answer.

use std::fmt;

/// The build that produced this binary.
///
/// Every field is a compile-time constant read from cargo by `build.rs`, so
/// this costs nothing at runtime and cannot drift from the binary it
/// describes — which a hand-written header can and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Build {
    /// `debug` or `release`, as cargo names it.
    pub profile: &'static str,
    /// The optimisation level. `release` with `opt-level = 0` is a thing that
    /// happens, and `profile` alone would not show it.
    pub opt_level: &'static str,
    /// Whether debug info was requested.
    pub debug: &'static str,
    /// The target triple — `wasm32-unknown-unknown` for the browser build,
    /// whose numbers `site/README.md` records beside native ones.
    pub target: &'static str,
}

/// The build that produced this binary.
pub const fn build() -> Build {
    Build {
        profile: env!("SLATE_BUILD_PROFILE"),
        opt_level: env!("SLATE_BUILD_OPT_LEVEL"),
        debug: env!("SLATE_BUILD_DEBUG"),
        target: env!("SLATE_BUILD_TARGET"),
    }
}

impl Build {
    /// Whether this build is one whose timings mean anything.
    ///
    /// A debug build is not a slower release build, it is a different
    /// program: no inlining, overflow checks on every arithmetic op, and
    /// `debug_assertions` running code that `release` does not compile. The
    /// ratios this repository asks readers to trust do not survive it.
    pub fn timings_are_meaningful(&self) -> bool {
        self.profile == "release"
    }
}

impl fmt::Display for Build {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (opt-level {}, debug {}) | {}",
            self.profile, self.opt_level, self.debug, self.target
        )
    }
}

/// Print the build line, and shout if the build cannot support a timing.
///
/// Used directly by the crates that have no cargo features of their own;
/// `slate_slatedb::stamp::announce` wraps this and adds the feature set.
/// `scripts/check_build_stamp.py` fails if a program that measures something
/// calls neither.
pub fn announce() {
    let mut out = std::io::stdout();
    announce_to(&mut out);
}

/// The build line, written wherever the caller needs it.
///
/// A benchmark wants it on stdout, with the table it qualifies, because that
/// is what gets pasted into a document. A daemon wants it on stderr, because
/// its stdout is a machine-readable channel — `slate-serverd` prints
/// `LISTENING <addr>` there and three harnesses parse it.
///
/// Write failures are dropped. This is provenance on the way to a terminal or
/// a log; a benchmark that aborted because a pipe closed while it was saying
/// what build it is would be a worse outcome than a missing line.
pub fn announce_to(out: &mut impl std::io::Write) {
    let _ = writeln!(out, "build: {}", build());
    for warning in warnings() {
        let _ = writeln!(out, "!! {warning}");
    }
}

/// Everything about this build that makes its numbers not mean what a reader
/// will assume they mean.
pub fn warnings() -> Vec<String> {
    let mut warnings = Vec::new();
    if !build().timings_are_meaningful() {
        warnings.push(format!(
            "this is a {} build: its wall-clock numbers are not comparable \
             with anything recorded in docs/, which is measured at --release. \
             Request counts still are.",
            build().profile
        ));
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_script_reaches_the_crate() {
        // `unknown` is the build script's fail-soft answer: right there,
        // useless here. Without this the stamp could print a line of nothing
        // and every caller would still look correct.
        assert_ne!(build().profile, "unknown", "PROFILE did not arrive");
        assert_ne!(build().target, "unknown", "TARGET did not arrive");
        assert!(
            build().profile == "debug" || build().profile == "release",
            "cargo's PROFILE is debug or release, got {:?}",
            build().profile
        );
    }

    #[test]
    fn a_debug_build_is_announced_as_one() {
        // `cargo test` is a debug build unless asked otherwise, so this arm
        // is the one that actually runs here — and it is the arm that matters,
        // because `scripts/run_examples.sh` runs debug binaries while the
        // docs quote release.
        let warnings = warnings();
        if build().profile == "release" {
            assert!(warnings.is_empty());
        } else {
            assert_eq!(warnings.len(), 1);
            let first = warnings.first().map(String::as_str).unwrap_or_default();
            assert!(first.contains("not comparable"));
            assert!(!build().timings_are_meaningful());
        }
    }

    #[test]
    fn the_line_says_profile_and_target() {
        let line = build().to_string();
        assert!(line.contains(build().profile));
        assert!(line.contains(build().target));
        assert!(line.contains("opt-level"));
    }
}
