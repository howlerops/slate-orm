//! The schema, reconciled before the socket opens.
//!
//! What is tested here is the *binary*'s behaviour, not the runner's — the
//! runner has its own suite in `slate-kernel`. The question this file answers
//! is whether the declaration in the TOML reaches it, which only a process that
//! stops and starts again over the same directory can say.
//!
//! The backend is `local` throughout. `memory` is a map in the process, so
//! every start would be a fresh keyspace and nothing here would mean anything:
//! the whole point is a *second* start whose schema disagrees with what the
//! first one wrote.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::needless_update,
    unreachable_pub
)]

mod harness;

use harness::{
    Files, Finished, Identity, Serving, compare, connect, proto, query, row, rows, str_value,
    u64_value,
};

const APP: Identity = Identity::app("u64:1", "u64:1");

/// The process exited zero.
#[track_caller]
fn ended_cleanly(finished: &Finished) {
    assert_eq!(finished.code, Some(0), "{}", finished.output());
}

/// The table before the index exists, `{DIR}` replaced with the store path.
const BEFORE: &str = r#"
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

/// The same table with an index on `kind`.
///
/// Two columns on purpose. The index plus the primary key then covers a filter
/// on `kind`, so the planner picks an index-only scan — which is what makes an
/// unbuilt index return nothing rather than merely cost more. A third column
/// and the same query answers correctly off a full scan, which would make this
/// file pass while testing nothing.
fn after(directory: &str, migrate_on_start: bool) -> String {
    let mut config = BEFORE.replace("{DIR}", directory);
    if !migrate_on_start {
        config.push_str("\n[schema]\nmigrate_on_start = false\n");
    }
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

/// Insert two rows through the running node.
async fn seed(serving: &Serving) {
    let mut client = connect(serving).await;
    let transaction = client
        .begin(APP.on(proto::BeginRequest {}))
        .await
        .expect("begin")
        .into_inner()
        .transaction;
    client
        .insert(APP.on(proto::InsertRequest {
            transaction: transaction.clone(),
            table: "docs".to_owned(),
            rows: vec![
                row(vec![u64_value(1), str_value("note")]),
                row(vec![u64_value(2), str_value("memo")]),
            ],
            ..Default::default()
        }))
        .await
        .expect("insert");
    client
        .commit(APP.on(proto::CommitRequest { transaction }))
        .await
        .expect("commit");
}

/// How many rows a filter on `kind` returns.
async fn matching(serving: &Serving, kind: &str) -> usize {
    let mut client = connect(serving).await;
    let found = rows(
        &mut client,
        &APP,
        proto::Query {
            filter: Some(compare(1, proto::CmpOp::Eq, str_value(kind))),
            ..query("docs")
        },
    )
    .await
    .expect("query");
    found.len()
}

#[tokio::test]
async fn a_node_builds_an_index_added_since_the_rows_were_written() {
    let files = Files::new();
    let directory = files.path().join("store");
    std::fs::create_dir_all(&directory).unwrap();
    let directory = directory.display().to_string();

    // First start: no index. Write two rows.
    let first = files.write("before.toml", &BEFORE.replace("{DIR}", &directory));
    let serving = Serving::start(&["--config", first.to_str().unwrap()]);
    seed(&serving).await;
    assert_eq!(matching(&serving, "note").await, 1);
    ended_cleanly(&serving.terminate());

    // Second start: the same data, a schema that now declares an index.
    // Without the reconciliation this returns 0 — the defect in
    // `slate_kernel::migrate`'s module docs, reached through the binary.
    let second = files.write("after.toml", &after(&directory, true));
    let serving = Serving::start(&["--config", second.to_str().unwrap()]);
    assert_eq!(
        matching(&serving, "note").await,
        1,
        "the index was declared and never built, so the query found nothing"
    );
    assert_eq!(matching(&serving, "absent").await, 0);
    let finished = serving.terminate();
    ended_cleanly(&finished);
    // And it said what it was doing. A backfill is the one startup step that
    // can take minutes, so a node that does it silently is a node an operator
    // cannot tell from a hung one.
    //
    // Both the verb and the index name are asserted, and the verb is the half
    // that was missing: naming `by_kind` alone passed a mutation that changed
    // the sentence to something an operator would not recognise, because the
    // name came from a list that was interpolated either way. It also has to
    // match what the no-op test below requires *not* to appear, or the pair
    // stops being a pair.
    let said = finished.output();
    assert!(said.contains("building"), "{said}");
    assert!(said.contains("by_kind"), "{said}");
    // And afterwards, what it actually wrote. `--plan` cannot estimate a
    // backfill — the row count is in statistics computed by a scan and held in
    // the serving process's memory, so a separate preview would have to do the
    // work it is previewing — which makes the run that pays the cost the only
    // place the number can come from. An operator sizing the *next* deploy has
    // nothing else to go on.
    assert!(said.contains("built 2 index entries"), "{said}");
}

#[tokio::test]
async fn migrate_on_start_false_refuses_rather_than_serving_an_unbuilt_index() {
    let files = Files::new();
    let directory = files.path().join("store");
    std::fs::create_dir_all(&directory).unwrap();
    let directory = directory.display().to_string();

    let first = files.write("before.toml", &BEFORE.replace("{DIR}", &directory));
    let serving = Serving::start(&["--config", first.to_str().unwrap()]);
    seed(&serving).await;
    ended_cleanly(&serving.terminate());

    // Turning the migration off is not permission to serve without one.
    let second = files.write("after.toml", &after(&directory, false));
    let finished = harness::run(&["--config", second.to_str().unwrap()]);
    let said = finished.output();
    assert!(
        finished.code != Some(0),
        "the node started with an index it had not built:\n{said}"
    );
    assert!(said.contains("by_kind"), "{said}");
    assert!(said.contains("migrate_on_start"), "{said}");
}

#[tokio::test]
async fn a_second_start_with_no_schema_change_builds_nothing() {
    let files = Files::new();
    let directory = files.path().join("store");
    std::fs::create_dir_all(&directory).unwrap();
    let directory = directory.display().to_string();

    let config = files.write("head.toml", &after(&directory, true));
    let serving = Serving::start(&["--config", config.to_str().unwrap()]);
    seed(&serving).await;
    ended_cleanly(&serving.terminate());

    // The second start has nothing to do, and must say nothing — a node that
    // announced a backfill on every restart would train its operators to
    // ignore the line that matters.
    let serving = Serving::start(&["--config", config.to_str().unwrap()]);
    assert_eq!(matching(&serving, "note").await, 1);
    let finished = serving.terminate();
    ended_cleanly(&finished);
    assert!(
        !finished.output().contains("building"),
        "a restart with nothing to do announced a backfill:\n{}",
        finished.output()
    );
}

/// `BEFORE`, plus an index on `kind` *and* a second table with an index of its
/// own and no rows in it.
///
/// Two indexes is the point, and two *tables* is what makes their counts
/// differ: two full indexes over one table write the same number of entries as
/// each other, so a breakdown over them could be wrong and look right.
fn after_two_indexes(directory: &str) -> String {
    let mut config = BEFORE.replace("{DIR}", directory);
    config.push_str(
        r#"
[[tables.indexes]]
name = "by_kind"
id = 1
columns = ["kind"]

[[tables]]
name = "spares"
id = 2
columns = [
  { name = "id",  type = "u64" },
  { name = "tag", type = "str" },
]
primary_key = ["id"]

[[tables.indexes]]
name = "by_tag"
id = 2
columns = ["tag"]

[[security.grants]]
role = "app"
tables = ["spares"]
actions = ["all"]
"#,
    );
    config
}

#[tokio::test]
async fn a_backfill_over_two_indexes_reports_each_one() {
    // The summed total answers "was there a backfill". It does not answer "and
    // which of them was the slow one", which is the question an operator has
    // when the answer to the first is "yes, for four minutes".
    //
    // Added because a mutation removing the per-index lines left every other
    // test here green: the one above builds a single index, and the breakdown
    // deliberately prints only when there is more than one.
    let files = Files::new();
    let directory = files.path().join("store");
    std::fs::create_dir_all(&directory).unwrap();
    let directory = directory.display().to_string();

    let first = files.write("before.toml", &BEFORE.replace("{DIR}", &directory));
    let serving = Serving::start(&["--config", first.to_str().unwrap()]);
    seed(&serving).await;
    ended_cleanly(&serving.terminate());

    let second = files.write("two.toml", &after_two_indexes(&directory));
    let serving = Serving::start(&["--config", second.to_str().unwrap()]);
    let finished = serving.terminate();
    ended_cleanly(&finished);

    let said = finished.output();
    assert!(said.contains("building 2 indexes"), "{said}");
    // The two counts differ, which is the whole reason to print them: `docs`
    // holds the seeded rows and `spares` holds none. A breakdown that reported
    // the total against each name would say `2` twice and pass a weaker test.
    assert!(said.contains("by_kind: 2"), "{said}");
    assert!(said.contains("by_tag: 0"), "{said}");
}
