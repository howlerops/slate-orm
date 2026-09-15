//! Is the committed timezone table still what `tzdata` says?
//!
//! `src/zones.rs` is generated — see `scripts/generate_zones.py` — and a
//! generated file nobody regenerates drifts. The drift here would be silent
//! and specific: a hand-edited pair, or a rule revision upstream, giving a
//! wrong answer in one month of one year while every other query stays right.
//!
//! The check itself lives in the generator, which runs with `--check` and
//! compares the *pairs* rather than the bytes. The reason is written up there:
//! the input is the system's `tzdata`, not a file in this repository, so a
//! byte compare would also fail on a comment, on the version string, and on a
//! `cargo fmt` from a different toolchain.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::process::Command;

/// The committed tables agree with this system's timezone database.
///
/// A hard failure when `python3` is missing rather than a skip, because the
/// thing being skipped is the point — the Python harness used to skip its whole
/// suite when `cargo` was absent, which read in CI as a passing suite that
/// exercised nothing.
#[test]
fn the_committed_tables_are_current() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/generate_zones.py");
    let output = Command::new("python3")
        .args([root, "--check"])
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "could not run {root} with python3 ({error}). This test needs \
                 python3 and a system timezone database; it fails rather than \
                 skipping, because a skipped freshness check is a green one."
            )
        });
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The header's own claims hold: a version is recorded, and the window bounds
/// the tables rather than merely being written next to them.
///
/// Separate from the check above because it needs no subprocess and catches a
/// different edit: `COVERS` widened without regenerating would make
/// `tests/zones.rs`'s coverage assertions vacuous, and nothing else would
/// notice.
#[test]
fn the_header_agrees_with_the_tables() {
    use slate_kernel::zones;

    assert!(
        !zones::TZDATA_VERSION.is_empty(),
        "no tzdata version recorded"
    );
    let (from, to) = zones::COVERS;
    assert!(from < to, "the window runs backwards: {from} to {to}");
    // Approximate seconds, which is all a bound needs: a year of slack either
    // side is far less than the decades a truncation would show up as.
    let year = 31_556_952;
    let (start, end) = ((from - 1970) * year, (to - 1970) * year);
    for zone in zones::ZONES {
        for (instant, _) in zone.transitions {
            assert!(
                *instant >= start - year && *instant <= end + year,
                "{}'s transition at {instant} is outside {from}-{to}",
                zone.name
            );
        }
    }
}
