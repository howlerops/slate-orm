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
//! The SlateDB version comes from `Cargo.lock` rather than the `0.16` range
//! in `Cargo.toml`, because the range is what we asked for and the lock is
//! what we got — and "a SlateDB release" was one of the three guesses #269
//! offered for the 3.0 figure it could not explain.

use std::path::{Path, PathBuf};

fn main() {
    let (version, lock) = slatedb_version();
    println!("cargo:rustc-env=SLATE_BUILD_SLATEDB={version}");
    // Only the lock is worth re-reading for: the rest come from cargo itself
    // and it already reruns the script when they change.
    if let Some(path) = lock {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// The resolved `slatedb` version, and the lock it was read from.
///
/// Fails soft to `unknown`. A build script that panics because a lockfile
/// moved would make this crate unbuildable to record provenance about it,
/// which is a bad trade: an honest `unknown` in the stamp is recoverable and
/// a broken build is not. `--locked` builds and vendored checkouts both keep
/// a lock where this looks.
fn slatedb_version() -> (String, Option<PathBuf>) {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let mut directory = Path::new(&manifest);
    loop {
        let candidate = directory.join("Cargo.lock");
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            return (parse_locked(&text, "slatedb"), Some(candidate));
        }
        match directory.parent() {
            Some(parent) => directory = parent,
            None => return ("unknown".to_owned(), None),
        }
    }
}

/// The `version` of one `[[package]]` in a `Cargo.lock`.
///
/// A hand-rolled scan rather than a TOML dependency: this runs before the
/// crate builds, and a build-dependency on a parser to read five characters
/// costs every consumer of this crate a compile. The format is stable and the
/// failure mode is `unknown`, not a wrong answer — `version` is only read
/// while inside the named package's block.
fn parse_locked(lock: &str, package: &str) -> String {
    let mut inside = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            inside = false;
        } else if let Some(name) = line.strip_prefix("name = ") {
            inside = name.trim_matches('"') == package;
        } else if inside && let Some(version) = line.strip_prefix("version = ") {
            return version.trim_matches('"').to_owned();
        }
    }
    "unknown".to_owned()
}
