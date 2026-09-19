//! `--plan`: what a deploy would do to the schema, before it does it.
//!
//! `migrate::plan` has been separate from `migrate::apply` since the runner was
//! written, on a doc comment that says why — "so a deployment can look first".
//! Nothing shipped let a deployment look. These tests are that gap closed: the
//! four states a keyspace can be in on the eve of a deploy, and the one
//! property that makes the flag safe to point at production.
//!
//! The backend is `local` throughout, for the reason `migrations.rs` gives: a
//! `memory` keyspace lives in the process that made it, so a separate
//! invocation could never see what a previous one wrote. The one `memory` test
//! here asserts that the flag says so rather than pretending otherwise.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    unreachable_pub
)]

mod harness;

use harness::{Files, Identity, Serving, connect, proto, row, run, str_value, u64_value};

const APP: Identity = Identity::app("u64:1", "u64:1");

/// A table with no index, `{DIR}` replaced with the store directory.
const BASE: &str = r#"
[listen]
address = "127.0.0.1:0"

[auth]
mode = "trusted-header"

[shutdown]
grace = "2s"

[storage]
backend = "local"
directory = "{DIR}"

[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id",   type = "u64" },
  { name = "kind", type = "str" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["docs"]
actions = ["all"]
"#;

/// `BASE` plus an index on `kind`.
fn with_index(directory: &str) -> String {
    let mut config = BASE.replace("{DIR}", directory);
    config.push_str(
        r#"
[[tables.indexes]]
name = "by_kind"
id = 1
columns = ["kind"]
"#,
    );
    config
}

/// `BASE` plus a third column: a layout change under rows already stored.
fn with_a_third_column(directory: &str) -> String {
    BASE.replace("{DIR}", directory).replace(
        r#"  { name = "kind", type = "str" },"#,
        "  { name = \"kind\", type = \"str\" },\n  { name = \"size\", type = \"i64\" },",
    )
}

/// A directory that exists and holds no database.
fn empty_store(files: &Files) -> String {
    let directory = files.path().join("store");
    std::fs::create_dir_all(&directory).unwrap();
    directory.display().to_string()
}

/// Run `--plan` over a configuration and return (exit code, output).
fn plan(files: &Files, name: &str, configuration: &str) -> (Option<i32>, String) {
    let path = files.write(name, configuration);
    let finished = run(&["--config", &path.display().to_string(), "--plan"]);
    (finished.code, finished.output())
}

/// Bring a keyspace into existence with `BASE`'s schema, and write a row.
///
/// A row and not just a start: an index build over an empty table writes
/// nothing, so a plan that promised one would be indistinguishable from a plan
/// that promised nothing.
async fn deployed(files: &Files, directory: &str) {
    let path = files.write("deploy.toml", &BASE.replace("{DIR}", directory));
    let serving = Serving::start(&["--config", &path.display().to_string()]);
    let mut client = connect(&serving).await;
    client
        .insert(APP.on(proto::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![row(vec![u64_value(1), str_value("note")])],
            ..Default::default()
        }))
        .await
        .expect("insert");
    let finished = serving.terminate();
    assert_eq!(finished.code, Some(0), "{}", finished.output());
}

#[test]
fn a_keyspace_that_does_not_exist_yet_is_previewed_as_a_first_deploy() {
    let files = Files::new();
    let directory = empty_store(&files);
    let (code, output) = plan(&files, "first.toml", &BASE.replace("{DIR}", &directory));

    // The interesting half is the exit code. Opening a reader on a keyspace
    // with no manifest fails inside SlateDB with a message about a missing
    // "latest transactional object", which is what this printed before the
    // existence probe went in — an error, on the single most likely thing to
    // be true the first time anybody runs the flag.
    assert_eq!(code, Some(0), "{output}");
    assert!(output.contains("first deploy"), "{output}");
    assert!(output.contains("register `docs`"), "{output}");
}

#[tokio::test]
async fn a_keyspace_already_carrying_this_schema_has_nothing_to_do() {
    let files = Files::new();
    let directory = empty_store(&files);
    deployed(&files, &directory).await;

    let (code, output) = plan(&files, "same.toml", &BASE.replace("{DIR}", &directory));
    assert_eq!(code, Some(0), "{output}");
    assert!(output.contains("Up to date"), "{output}");
    assert!(!output.contains("would be applied"), "{output}");
}

#[tokio::test]
async fn an_index_added_since_the_rows_were_written_is_previewed_as_a_build() {
    let files = Files::new();
    let directory = empty_store(&files);
    deployed(&files, &directory).await;

    let (code, output) = plan(&files, "index.toml", &with_index(&directory));
    assert_eq!(code, Some(0), "{output}");
    assert!(output.contains("build index `by_kind`"), "{output}");
    assert!(output.contains("`docs`"), "{output}");
    // The cost, not just the name. A backfill is the one step slow enough to
    // change a deploy window, and saying so is most of what the flag is for.
    assert!(output.contains("minutes"), "{output}");
}

#[tokio::test]
async fn a_layout_change_under_stored_rows_is_blocked_and_exits_non_zero() {
    let files = Files::new();
    let directory = empty_store(&files);
    deployed(&files, &directory).await;

    let (code, output) = plan(&files, "blocked.toml", &with_a_third_column(&directory));
    // One, not two: two is "this configuration is wrong" and a supervisor is
    // documented to read it that way. The configuration is fine; the keyspace
    // disagrees with it.
    assert_eq!(code, Some(1), "{output}");
    assert!(output.contains("BLOCKED"), "{output}");
    assert!(output.contains("column layout changed"), "{output}");
}

#[test]
fn a_memory_backend_says_it_has_no_stored_state_rather_than_inventing_one() {
    let files = Files::new();
    let configuration = BASE.replace("{DIR}", "unused").replace(
        "backend = \"local\"\ndirectory = \"unused\"",
        "backend = \"memory\"",
    );
    let (code, output) = plan(&files, "memory.toml", &configuration);
    assert_eq!(code, Some(0), "{output}");
    assert!(output.contains("no stored state"), "{output}");
    // The failure this guards against is the plausible one: a memory keyspace
    // is always empty, so planning against it would print a complete and
    // entirely fictional first-deploy plan.
    assert!(!output.contains("would be applied"), "{output}");
}

#[tokio::test]
async fn previewing_does_not_fence_the_node_that_holds_the_lease() {
    // The property that makes the flag safe to point at production, and the
    // one a read would not prove: `handover.rs` in `slate-server` shows a
    // fenced SlateDB can still be read from. So the leader writes afterwards.
    let files = Files::new();
    let directory = empty_store(&files);
    let path = files.write("leader.toml", &BASE.replace("{DIR}", &directory));

    let leader = Serving::start(&["--config", &path.display().to_string()]);
    let mut client = connect(&leader).await;
    client
        .insert(APP.on(proto::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![row(vec![u64_value(1), str_value("before")])],
            ..Default::default()
        }))
        .await
        .expect("the leader writes before the preview");

    let (code, output) = plan(&files, "preview.toml", &with_index(&directory));
    assert_eq!(code, Some(0), "{output}");

    // The write that proves it. A fenced writer fails here.
    client
        .insert(APP.on(proto::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![row(vec![u64_value(2), str_value("after")])],
            ..Default::default()
        }))
        .await
        .expect("the leader still holds the lease after a preview");

    let finished = leader.terminate();
    assert_eq!(finished.code, Some(0), "{}", finished.output());
}

#[test]
fn plan_and_check_cannot_be_asked_for_together() {
    // They answer different questions over different inputs — `--check` reads
    // the file and opens no storage, `--plan` must open storage — and a flag
    // pair where one silently wins is how a deployment ends up believing it
    // previewed something it did not.
    let files = Files::new();
    let directory = empty_store(&files);
    let path = files.write("both.toml", &BASE.replace("{DIR}", &directory));
    let finished = run(&["--config", &path.display().to_string(), "--plan", "--check"]);
    assert_eq!(finished.code, Some(2), "{}", finished.output());
    assert!(
        finished.output().contains("cannot be used with"),
        "{}",
        finished.output()
    );
}
