//! Capture what this crate is being *built* as, so a measurement can say so.
//!
//! `POINT_READ_COST` was 3.0 for nine tasks and nobody could say why. The
//! answer (#278) was that the recorded run had SlateDB's block cache compiled
//! out — a cargo feature, invisible in every number it changed by 3x, because
//! no measurement in this repository recorded the build that produced it.
//!
//! The profile and target come from `slate-kernel`'s build script, because
//! they belong to the cargo invocation rather than to a crate. What is left
//! here is the one fact only this crate can answer: which `slatedb` it got.
//!
//! Versions come from `Cargo.lock` rather than the ranges in `Cargo.toml`,
//! because a range is what we asked for and the lock is what we got — and "a
//! SlateDB release" was one of the three guesses #269 offered for the 3.0
//! figure it could not explain.
//!
//! Four packages are named, not one. #269's other two guesses were "a block
//! size" and "the readahead #34 turned on", and the code that decides both
//! lives in `slatedb` and `foyer`. `foyer` is the cache implementation whose
//! *absence* was the whole of #278 — a build without it read three times as
//! many objects — so a build with a different version of it is exactly as
//! worth recording as a build without it. `object_store` issues the requests
//! that every GET count here is a count of, and `tokio` schedules the
//! concurrency that the pipelined-read costing is about.
//!
//! The list stops there on purpose: `rustls` and `aws-lc-rs` are under every
//! S3 byte too, and naming them would make the line unreadable for a
//! connection cost that no measurement in this repository isolates. Four is
//! the set whose versions have a mechanism by which they could move a
//! recorded number.

use std::path::{Path, PathBuf};

/// How far above this crate's manifest to look for the workspace lock.
///
/// Four would do for this repository (`crates/slate-slatedb` is one below the
/// root). Twelve leaves room for a deeper checkout without letting a build
/// with no lock of its own wander into an unrelated project's.
const MAX_CLIMB: usize = 12;

include!("lockfile.rs");

fn main() {
    let (versions, lock) = resolved();
    // `.get` rather than indexing. `parse_locked` returns every `STAMPED`
    // name by construction, so the fallback is unreachable today — but a
    // build script that panics cannot be debugged from a build log, and an
    // `unknown` in the line is the documented answer for "could not tell".
    let version = |name: &str| {
        versions
            .get(name)
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned())
    };
    let deps = STAMPED
        .iter()
        .map(|name| format!("{name} {}", version(name)))
        .collect::<Vec<_>>()
        .join(", ");
    println!("cargo:rustc-env=SLATE_BUILD_DEPS={deps}");
    // Kept beside the list because `Stamp::slatedb` is read on its own by the
    // test that proves the lock is really being parsed, and because SlateDB
    // is the one dependency a reader scans for.
    println!("cargo:rustc-env=SLATE_BUILD_SLATEDB={}", version("slatedb"));
    // Only the lock is worth re-reading for: the rest come from cargo itself
    // and it already reruns the script when they change.
    if let Some(path) = lock {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// Every `STAMPED` version, and the lock they were read from.
///
/// Fails soft to `unknown`. A build script that panics because a lockfile
/// moved would make this crate unbuildable to record provenance about it,
/// which is a bad trade: an honest `unknown` in the stamp is recoverable and
/// a broken build is not. `--locked` builds and vendored checkouts both keep
/// a lock where this looks.
fn resolved() -> (
    std::collections::HashMap<&'static str, String>,
    Option<PathBuf>,
) {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let mut directory = Path::new(&manifest);
    // Bounded, and a lock only counts beside a manifest. Walking freely to
    // `/` meant a vendored or registry build of this crate — which has no
    // lock of its own — could climb until it found *somebody's* Cargo.lock
    // and stamp that workspace's `slatedb` as the one this was built
    // against. A wrong version is worse than the documented `unknown`,
    // because only one of the two is obviously not an answer.
    //
    // The first attempt at this bound stopped as soon as an ancestor had no
    // `Cargo.toml`, which is wrong in the ordinary layout: `crates/` has
    // none, so every build stamped `unknown` — a fix that broke the feature
    // it was tightening. `the_resolved_slatedb_version_is_read_from_the_lock`
    // caught it, which is why that test asserts a resolved `x.y.z` rather
    // than merely "something".
    for _ in 0..MAX_CLIMB {
        let candidate = directory.join("Cargo.lock");
        if directory.join("Cargo.toml").exists()
            && let Ok(text) = std::fs::read_to_string(&candidate)
        {
            return (parse_locked(&text), Some(candidate));
        }
        match directory.parent() {
            Some(parent) => directory = parent,
            None => break,
        }
    }
    (unknown(), None)
}
