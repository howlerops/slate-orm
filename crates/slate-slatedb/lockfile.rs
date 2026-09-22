// The `Cargo.lock` parsing behind the build stamp, in a file that is
// `include!`d by both `build.rs` and `tests/lockfile.rs`.
//
// It lived in `build.rs` until #285, where two mutations survived for the
// same reason: **nothing could call it**. A build script is not a library,
// so the parser was only ever exercised against this repository's own lock —
// a lock in which all four packages resolve, each exactly once. Neither the
// duplicate-version rule nor the missing-package fallback had a test that
// could distinguish them from their opposites.
//
// `include!` rather than a crate or a `mod`: the parser must be reachable
// from a build script, which cannot depend on its own package, and making it
// a published module would put lockfile parsing in the public API of a
// storage crate for the benefit of one test.

/// The dependencies whose version is recorded, in the order they are printed.
///
/// See the module docs for why these four and not others.
const STAMPED: [&str; 4] = ["slatedb", "foyer", "object_store", "tokio"];

/// Every stamped name mapped to `unknown`, for when no lock was found.
fn unknown() -> std::collections::HashMap<&'static str, String> {
    STAMPED
        .iter()
        .map(|name| (*name, "unknown".to_owned()))
        .collect()
}

/// The `version` of each `STAMPED` `[[package]]` in a `Cargo.lock`.
///
/// A hand-rolled scan rather than a TOML dependency: this runs before the
/// crate builds, and a build-dependency on a parser to read four version
/// strings costs every consumer of this crate a compile. The format is stable
/// and the failure mode is `unknown`, not a wrong answer — `version` is only
/// read while inside a named package's block.
///
/// A name resolving to more than one version is joined with `/` rather than
/// taking the first. Twenty-six packages in this lock are duplicated today
/// (`base64`, `digest`, `rand`, …); none of these four is, but "the first one
/// I found" would be a wrong answer presented as a fact the day that changes,
/// and that is the precise failure this whole line of work exists because of.
fn parse_locked(lock: &str) -> std::collections::HashMap<&'static str, String> {
    let mut found: std::collections::HashMap<&'static str, Vec<String>> =
        std::collections::HashMap::new();
    let mut inside: Option<&'static str> = None;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            inside = None;
        } else if let Some(name) = line.strip_prefix("name = ") {
            let name = name.trim_matches('"');
            inside = STAMPED.iter().find(|stamped| **stamped == name).copied();
        } else if let Some(name) = inside
            && let Some(version) = line.strip_prefix("version = ")
        {
            let version = version.trim_matches('"').to_owned();
            let versions = found.entry(name).or_default();
            if !versions.contains(&version) {
                versions.push(version);
            }
            inside = None;
        }
    }
    STAMPED
        .iter()
        .map(|name| {
            let version = found
                .get(name)
                .filter(|versions| !versions.is_empty())
                .map_or_else(|| "unknown".to_owned(), |versions| versions.join("/"));
            (*name, version)
        })
        .collect()
}
