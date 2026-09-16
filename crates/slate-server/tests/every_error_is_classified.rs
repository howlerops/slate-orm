//! Every `KernelError` variant has a status code and a reason token.
//!
//! `KernelError` is `#[non_exhaustive]`, so `code_for` and `reason_for` must
//! end in a wildcard — a downstream crate cannot match it exhaustively and the
//! compiler cannot make adding a variant a compile error. The wildcard is
//! `INTERNAL` / `UNCLASSIFIED`, which the module documents as deliberate: a
//! token is a promise that the meaning is stable, and nobody has decided what
//! an unclassified variant means.
//!
//! What was *not* deliberate is that nobody was ever made to decide. Eight of
//! twenty-nine variants had reached the wildcard, and three of them —
//! `SortTooLarge`, `TooManyGroups`, `TooManyDistinctValues` — are reachable
//! from any sorted or grouped query and had been since those went on the wire.
//! A caller that exceeded a server limit got `INTERNAL`, which reads as "the
//! server broke" and invites a retry that exceeds the same limit again. They
//! were found by a *different* variant landing on the wildcard, which is to
//! say by luck.
//!
//! This reads the two files as text rather than constructing twenty-nine
//! values. That is weaker — it checks a name appears in a match, not that the
//! arm is right — and it is the check that catches the failure that actually
//! happens, which is a variant added and nobody thinking about the wire. It
//! costs nothing and needs no fixture. The arms themselves are checked by the
//! suites that provoke them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

/// The body of a `fn name(...) { ... }`, from its signature to the closing
/// brace at column zero.
fn body_of<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` is not in this file any more"));
    let rest = &source[start..];
    let end = rest
        .find("\n}\n")
        .unwrap_or_else(|| panic!("`{signature}` has no closing brace at column zero"));
    &rest[..end]
}

#[test]
fn every_kernel_error_has_a_code_and_a_reason() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let errors = std::fs::read_to_string(root.join("../slate-kernel/src/error.rs"))
        .expect("the kernel's error module");
    let status = std::fs::read_to_string(root.join("src/status.rs")).expect("status.rs");

    let variants: Vec<String> = body_of(&errors, "pub enum KernelError {")
        .lines()
        // A variant is a capitalised identifier at exactly four spaces. Doc
        // comments and attributes are indented the same and start with `/` or
        // `#`, which no identifier does.
        .filter_map(|line| {
            let rest = line.strip_prefix("    ")?;
            let first = rest.chars().next()?;
            if !first.is_ascii_uppercase() {
                return None;
            }
            Some(
                rest.chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect::<String>(),
            )
        })
        .collect();

    assert!(
        variants.len() > 20,
        "the variant scrape found only {}, so it has stopped working rather than the enum \
         having shrunk: {variants:?}",
        variants.len()
    );

    for (what, signature) in [
        ("status code", "pub fn code_for"),
        ("reason token", "fn reason_for"),
    ] {
        let body = body_of(&status, signature);
        let unclassified: Vec<&String> = variants
            .iter()
            .filter(|variant| !body.contains(&format!("KernelError::{variant}")))
            .collect();
        assert!(
            unclassified.is_empty(),
            "these `KernelError` variants have no {what} and fall to the wildcard, where a \
             caller reads them as the server having broken: {unclassified:?}"
        );
    }
}
