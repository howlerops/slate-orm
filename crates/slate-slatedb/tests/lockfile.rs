//! The build stamp's `Cargo.lock` parser, over locks written for the purpose.
//!
//! #285 extracted `parse_locked` from `build.rs` so that these could exist. A
//! build script is not a library and nothing can call into one, so until now
//! the parser was exercised only against this repository's own lock — where
//! all four packages resolve, each exactly once. Two mutations survived for
//! that reason alone: taking the first of several versions instead of joining
//! them, and returning an empty string instead of `unknown` for a package
//! that is not there. Neither shape occurs in our lock, so neither could be
//! told from its opposite.

// The convention the other integration tests here use: a test asserts, and
// asserting is panicking. `indexing_slicing` joins them because the map this
// parser returns is keyed by a fixed set of names that the test itself
// writes — an index that is wrong is a test bug, and panicking on it is the
// report.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

include!("../lockfile.rs");

/// A lock with one `[[package]]` block per entry, as cargo writes them.
fn lock(entries: &[(&str, &str)]) -> String {
    entries
        .iter()
        .map(|(name, version)| format!("[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_stamped_package_is_read() {
    let found = parse_locked(&lock(&[
        ("serde", "1.0.0"),
        ("slatedb", "0.16.0"),
        ("foyer", "0.22.3"),
        ("object_store", "0.14.1"),
        ("tokio", "1.53.1"),
    ]));
    assert_eq!(found["slatedb"], "0.16.0");
    assert_eq!(found["foyer"], "0.22.3");
    assert_eq!(found["object_store"], "0.14.1");
    assert_eq!(found["tokio"], "1.53.1");
    assert_eq!(found.len(), 4, "read something that is not stamped");
}

#[test]
fn a_package_that_is_not_there_is_unknown_not_empty() {
    // The distinction matters in the output: `tokio unknown` says the lock
    // was read and did not have it, `tokio ` says nothing at all and reads
    // as a formatting bug. Only one of the two is an answer.
    let found = parse_locked(&lock(&[("slatedb", "0.16.0")]));
    assert_eq!(found["tokio"], "unknown");
    assert_eq!(found["foyer"], "unknown");
    assert_eq!(found["slatedb"], "0.16.0");
}

#[test]
fn a_duplicated_package_reports_both_versions() {
    // Twenty-six packages in the real lock resolve twice. None of these four
    // does *today*, which is exactly why this has to be a written lock: "the
    // first one I found", presented as the version a measurement was taken
    // against, is the failure the whole build stamp exists to prevent.
    let found = parse_locked(&lock(&[
        ("tokio", "1.53.1"),
        ("serde", "1.0.0"),
        ("tokio", "2.0.0"),
    ]));
    assert_eq!(found["tokio"], "1.53.1/2.0.0");
}

#[test]
fn the_same_version_twice_is_reported_once() {
    let found = parse_locked(&lock(&[("foyer", "0.22.3"), ("foyer", "0.22.3")]));
    assert_eq!(found["foyer"], "0.22.3");
}

#[test]
fn a_version_outside_a_package_block_is_not_attributed_to_it() {
    // `[[package]]` closes the previous block. A `version` appearing after a
    // stamped name but under a *different* heading — cargo writes
    // `[[patch.unused]]` and `[metadata]` sections — must not be read as that
    // package's.
    let found = parse_locked(
        "[[package]]\nname = \"foyer\"\nversion = \"0.22.3\"\n\n\
         [metadata]\nversion = \"9.9.9\"\n",
    );
    assert_eq!(found["foyer"], "0.22.3");
}

#[test]
fn an_empty_lock_gives_unknown_for_everything() {
    let found = parse_locked("");
    assert_eq!(found.len(), 4);
    assert!(found.values().all(|version| version == "unknown"));
}

// The two below pin behaviour on a lock cargo would not write. That is the
// point: this is a hand-rolled scanner over a file it does not control, and
// the whole mechanism exists so that a *wrong* version is never presented as
// the one a measurement was taken against. On malformed input it has to
// answer `unknown`, not guess.

#[test]
fn the_most_recent_name_wins_within_a_block() {
    // Two `name` lines before a `version`. The second must take over, or a
    // stamped package inherits the version of whatever follows it.
    let found =
        parse_locked("[[package]]\nname = \"foyer\"\nname = \"serde\"\nversion = \"1.0.0\"\n");
    assert_eq!(
        found["foyer"], "unknown",
        "foyer took serde's version because the name did not take over"
    );
}

#[test]
fn a_block_with_no_version_does_not_borrow_the_next_ones() {
    // `[[package]]` closes the previous block even when that block never
    // reached a `version`. Without the close, `foyer` here is reported as
    // 1.0.0 — a version from a block that is not its own.
    let found = parse_locked("[[package]]\nname = \"foyer\"\n[[package]]\nversion = \"1.0.0\"\n");
    assert_eq!(
        found["foyer"], "unknown",
        "foyer borrowed a version from the block after it"
    );
}

#[test]
fn the_no_lock_fallback_names_every_package() {
    // `unknown()` is what `resolved()` returns when the climb finds no lock.
    // It has to carry all four names, not an empty map: the caller formats
    // one `name version` pair per `STAMPED` entry, and a missing key there
    // would read as a formatting bug rather than as "no lock was found".
    let found = unknown();
    assert_eq!(found.len(), STAMPED.len());
    for name in STAMPED {
        assert_eq!(found[name], "unknown", "{name} missing from the fallback");
    }
}
