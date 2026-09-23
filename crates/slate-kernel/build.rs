//! Capture the build's own identity, so a measurement can record it.
//!
//! A crate cannot see its own profile: `cfg!(debug_assertions)` is a proxy
//! that a customised profile breaks, and nothing in ordinary code can read the
//! optimisation level or the target triple at all. A build script can, so it
//! is read here once and handed to `build.rs`'s module as environment.
//!
//! This lives in the kernel rather than in each crate that measures something
//! because a profile is a property of the *cargo invocation*, not of a crate:
//! every crate in one build shares it, and four copies of these six lines
//! would be four things to keep in step. `slate-slatedb` composes this with
//! the feature set that is genuinely its own.
//!
//! Why this is not `vergen` or `built`: those want a `.git` and a dependency,
//! and what was actually missing — recorded in `ledger/`, #278 — was four
//! strings cargo already sets. A build-dependency on a crate to read four
//! environment variables would cost every consumer a compile.

/// Cargo's own description of this build. `PROFILE` is `debug` or `release`;
/// `OPT_LEVEL` and `DEBUG` are exact and say what `PROFILE` alone cannot once
/// a profile is customised — this workspace customises `release`.
const FROM_CARGO: [&str; 4] = ["PROFILE", "OPT_LEVEL", "DEBUG", "TARGET"];

fn main() {
    for key in FROM_CARGO {
        // Fails soft rather than panicking: a build script that breaks the
        // build to record provenance about it is a bad trade. `unknown` in a
        // stamp is honest and recoverable; an unbuildable crate is not.
        let value = std::env::var(key).unwrap_or_else(|_| "unknown".to_owned());
        println!("cargo:rustc-env=SLATE_BUILD_{key}={value}");
    }
    // Cargo reruns a build script when its own inputs change; naming no files
    // would make it rerun on every source change instead, for four strings
    // that only move when the profile does.
    println!("cargo:rerun-if-changed=build.rs");
}
