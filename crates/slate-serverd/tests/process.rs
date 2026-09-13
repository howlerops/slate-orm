//! The binary, started as a process and talked to over gRPC.
//!
//! Everything asserted here is a property of the *file*: a table declared in
//! TOML is queryable, a policy written in TOML hides rows, a `CHECK` written
//! in TOML refuses a write, a partial index written in TOML is one the planner
//! will read. Each is the same question — did the declaration reach the
//! kernel, or did it merely parse — and only a running server answers it.

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
    Files, Identity, Serving, all_of, compare, connect, i64_value, is_not_null, null_value, proto,
    query, row, rows, str_value, u64_value,
};

/// The fixture every test in this file serves.
///
/// Deliberately not the shape `clients/python/testserver` uses: this one is
/// chosen so that each declared thing hides or admits a *different* row, so a
/// declaration that silently did nothing changes the answer rather than
/// leaving it the same.
///
/// - the size-5 row is hidden by the policy and by nothing else
/// - the tenant-2 row is hidden by the tenant column and by nothing else
/// - `kind = 'forbidden'` is refused by the `CHECK` and by nothing else
const CONFIG: &str = r#"
[listen]
address = "127.0.0.1:0"

[auth]
mode = "trusted-header"
require_tenant = true

[shutdown]
# Short so a test that leaves a client connected does not sit through the
# default. tonic's shutdown waits for connections to close, not merely for
# in-flight requests to finish, so an idle-but-open channel costs the whole
# grace period.
grace = "2s"

[storage]
backend = "memory"

[[tables]]
name = "docs"
id = 1
columns = [
  { name = "tenant_id", type = "u64" },
  { name = "id",        type = "u64" },
  { name = "kind",      type = "str" },
  { name = "size",      type = "i64" },
  { name = "note",      type = "str", nullable = true },
]
primary_key = ["tenant_id", "id"]
tenant_column = "tenant_id"

[[tables.indexes]]
name = "by_kind"
id = 1
columns = ["kind"]
# `note IS NOT NULL` rather than something about `size`, and the reason is
# worth recording: the policy below conjoins `size >= 10` onto *every* read of
# this table, so a partial index predicated on `size > 0` would be provably
# satisfied by every query and there would be no case where the planner
# declines it. That the security predicate is what makes an index usable is the
# repository's own point — security narrows the scan rather than costing a
# filter pass — but it makes `size` the wrong column to test the refusal with.
where = "note IS NOT NULL"

[[tables.indexes]]
name = "by_size"
id = 2
columns = [{ column = "size", direction = "desc" }]

[[tables.checks]]
name = "not_forbidden"
predicate = "kind <> 'forbidden'"

[[security.grants]]
role = "app"
tables = ["docs"]
actions = ["all"]

[[security.policies]]
name = "big_enough"
table = "docs"
actions = ["read", "insert", "update", "delete"]
using = "size >= 10"
"#;

const SEED: &str = r#"
[[seed]]
table = "docs"
rows = [
  { tenant_id = 1, id = 1, kind = "kind-a", size = 5,  note = "hidden by the policy" },
  { tenant_id = 1, id = 2, kind = "kind-b", size = 15 },
  { tenant_id = 1, id = 3, kind = "kind-b", size = 25, note = "third" },
  { tenant_id = 2, id = 1, kind = "kind-c", size = 50 },
]
"#;

/// Ordinals, as the file declares them.
const TENANT: u32 = 0;
const ID: u32 = 1;
const KIND: u32 = 2;
const NOTE: u32 = 4;

const APP: Identity = Identity::app("u64:1", "u64:1");
const OTHER_TENANT: Identity = Identity::app("u64:1", "u64:2");

fn serving(files: &Files) -> Serving {
    let config = files.write("head.toml", CONFIG);
    let seed = files.write("seed.toml", SEED);
    Serving::start(&[
        "--config",
        &config.display().to_string(),
        "--seed",
        &seed.display().to_string(),
    ])
}

/// The `id` of every row a query returned.
fn ids(returned: &[proto::Row]) -> Vec<u64> {
    returned
        .iter()
        .filter_map(
            |row| match row.values.get(ID as usize).and_then(|v| v.kind.as_ref()) {
                Some(proto::value::Kind::Uint64Value(id)) => Some(*id),
                _ => None,
            },
        )
        .collect()
}

#[tokio::test]
async fn a_table_declared_in_the_file_is_queryable() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let returned = rows(&mut client, &APP, query("docs"))
        .await
        .expect("a query against the declared table");
    // Rows 2 and 3: row 1 is below the policy's threshold and the tenant-2 row
    // is another tenant's.
    assert_eq!(ids(&returned), vec![2, 3]);
    // And the row really is the declared shape, five columns wide.
    assert_eq!(returned.first().map(|r| r.values.len()), Some(5));

    drop(client);
    let finished = serving.terminate();
    assert_eq!(finished.code, Some(0), "stderr:\n{}", finished.stderr);
}

#[tokio::test]
async fn a_policy_written_in_the_file_hides_a_row() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    // Asking for the hidden row by its key reads as absent rather than
    // forbidden, which is the kernel's rule and is what makes the error code
    // not an existence oracle.
    let response = client
        .get(APP.on(proto::GetRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            primary_key: Some(row(vec![u64_value(1), u64_value(1)])),
            freshness: None,
            ..Default::default()
        }))
        .await
        .expect("a get of a hidden row is not an error")
        .into_inner();
    assert!(!response.found, "the policy did not hide the row");

    // A visible row is found through the same call, so "not found" is about
    // the policy rather than about the request being wrong.
    let response = client
        .get(APP.on(proto::GetRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            primary_key: Some(row(vec![u64_value(1), u64_value(2)])),
            freshness: None,
            ..Default::default()
        }))
        .await
        .expect("a get of a visible row")
        .into_inner();
    assert!(response.found);
}

#[tokio::test]
async fn the_tenant_column_written_in_the_file_isolates_tenants() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let mine = rows(&mut client, &APP, query("docs"))
        .await
        .expect("tenant 1");
    let theirs = rows(&mut client, &OTHER_TENANT, query("docs"))
        .await
        .expect("tenant 2");

    assert_eq!(ids(&mine), vec![2, 3]);
    assert_eq!(ids(&theirs), vec![1]);

    // Not merely different counts: the tenant column of every row is the
    // caller's own.
    for returned in &theirs {
        assert_eq!(
            returned
                .values
                .get(TENANT as usize)
                .and_then(|v| v.kind.clone()),
            Some(proto::value::Kind::Uint64Value(2))
        );
    }
}

#[tokio::test]
async fn a_write_round_trips_through_a_transaction() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

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
            rows: vec![row(vec![
                u64_value(1),
                u64_value(9),
                str_value("kind-d"),
                i64_value(42),
                null_value(),
            ])],
            ..Default::default()
        }))
        .await
        .expect("insert");

    client
        .commit(APP.on(proto::CommitRequest {
            transaction: transaction.clone(),
        }))
        .await
        .expect("commit");

    let returned = rows(&mut client, &APP, query("docs")).await.expect("query");
    assert_eq!(ids(&returned), vec![2, 3, 9]);
}

#[tokio::test]
async fn a_check_written_in_the_file_refuses_a_write() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let transaction = client
        .begin(APP.on(proto::BeginRequest {}))
        .await
        .expect("begin")
        .into_inner()
        .transaction;

    // Size 42 satisfies the policy, so the only thing that can refuse this row
    // is the `CHECK` on `kind`.
    let refused = client
        .insert(APP.on(proto::InsertRequest {
            transaction: transaction.clone(),
            table: "docs".to_owned(),
            rows: vec![row(vec![
                u64_value(1),
                u64_value(10),
                str_value("forbidden"),
                i64_value(42),
                null_value(),
            ])],
            ..Default::default()
        }))
        .await;

    let status = refused.expect_err("the CHECK must refuse this row");
    assert!(
        status.message().contains("not_forbidden"),
        "the refusal should name the check: {status}"
    );
}

#[tokio::test]
async fn a_partial_index_written_in_the_file_is_one_the_planner_will_read() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    // A hint is advice: the planner reads a partial index only for a query it
    // can *prove* lands inside the predicate, and ignores the hint otherwise.
    // So the two explains below differ only in whether the query implies
    // `size > 0`, and the difference is visible in the chosen access path.
    let hint = proto::AccessHint {
        path: Some(proto::access_hint::Path::Index("by_kind".to_owned())),
    };

    let inside = proto::Query {
        filter: Some(all_of(vec![
            compare(KIND, proto::CmpOp::Eq, str_value("kind-b")),
            is_not_null(NOTE),
        ])),
        hint: Some(hint.clone()),
        ..query("docs")
    };
    let outside = proto::Query {
        filter: Some(compare(KIND, proto::CmpOp::Eq, str_value("kind-b"))),
        hint: Some(hint),
        ..query("docs")
    };

    let explain = |mut client: proto::records_client::RecordsClient<tonic::transport::Channel>,
                   query: proto::Query| async move {
        client
            .explain(APP.on(proto::ExplainRequest {
                transaction: String::new(),
                query: Some(query),
                freshness: None,
            }))
            .await
            .expect("explain")
            .into_inner()
    };

    let admitted = explain(client.clone(), inside).await;
    let rejected = explain(client.clone(), outside).await;

    assert!(
        admitted.access.contains("by_kind"),
        "a query inside the predicate should be able to use the partial index: {}",
        admitted.display
    );
    assert!(
        !rejected.access.contains("by_kind"),
        "a query the predicate does not cover must not read the partial index: {}",
        rejected.display
    );

    // And the plan still returns the right rows, which is the property a
    // wrongly-read partial index would break.
    let returned = rows(
        &mut client,
        &APP,
        proto::Query {
            filter: Some(all_of(vec![
                compare(KIND, proto::CmpOp::Eq, str_value("kind-b")),
                is_not_null(NOTE),
            ])),
            ..query("docs")
        },
    )
    .await
    .expect("query");
    // Row 2 has no note, so the partial index does not hold it and neither
    // does the answer. Reading the index and getting row 2 back would be the
    // bug this whole test exists for.
    assert_eq!(ids(&returned), vec![3]);
}

#[tokio::test]
async fn an_unauthenticated_request_is_refused() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let status = rows(&mut client, &Identity::nobody(), query("docs"))
        .await
        .expect_err("a request with no identity must be refused");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

#[tokio::test]
async fn require_tenant_refuses_a_caller_with_no_tenant() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let tenantless = Identity {
        principal: "u64:1",
        tenant: None,
        roles: "app",
        bearer: None,
    };
    let status = rows(&mut client, &tenantless, query("docs"))
        .await
        .expect_err("`require_tenant = true` must refuse this");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert!(status.message().contains("slate-tenant"), "{status}");
}

#[tokio::test]
async fn the_node_reports_itself_the_leader() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;

    let status = client
        .leadership(APP.on(proto::LeadershipRequest {}))
        .await
        .expect("leadership")
        .into_inner();
    assert_eq!(
        status.standing,
        proto::leadership_status::Standing::Leader as i32,
        "a single node over its own lease is the leader"
    );
    // The lease is the real `ObjectStoreLease`, so it has a generation.
    assert_eq!(status.generation, Some(1));
    assert!(
        status.holder.starts_with("slate-serverd-"),
        "{}",
        status.holder
    );
}

#[tokio::test]
async fn a_sigterm_drains_and_exits_zero() {
    let files = Files::new();
    let serving = serving(&files);
    let mut client = connect(&serving).await;
    // One real request first, so the shutdown is of a server that has served.
    rows(&mut client, &APP, query("docs")).await.expect("query");
    // And then the connection goes, so the drain is a drain rather than a wait
    // for the grace period to run out.
    drop(client);

    let finished = serving.terminate();
    assert_eq!(
        finished.code,
        Some(0),
        "a clean shutdown exits zero; stderr:\n{}",
        finished.stderr
    );
    assert!(
        finished.stdout.contains("STOPPING SIGTERM"),
        "the shutdown should say which signal it saw:\n{}",
        finished.stdout
    );
}

#[tokio::test]
async fn a_token_authenticates_and_a_missing_one_does_not() {
    const SECRET: &str = "9f2a1c8e4b6d0f37a5e9c1b8d4f60a2e";
    let files = Files::new();
    files.write("token", SECRET);
    let config = format!(
        r#"{CONFIG}
[[auth.tokens]]
name = "harness"
secret_file = "{}"
principal = "u64:1"
tenant = "u64:1"
roles = ["app"]
"#,
        files.path().join("token").display()
    )
    .replace("mode = \"trusted-header\"", "mode = \"token\"")
    .replace("require_tenant = true\n", "");

    let path = files.write("token.toml", &config);
    let seed = files.write("seed.toml", SEED);
    let serving = Serving::start(&[
        "--config",
        &path.display().to_string(),
        "--seed",
        &seed.display().to_string(),
    ]);
    let mut client = connect(&serving).await;

    let with = Identity::token(SECRET);
    let returned = rows(&mut client, &with, query("docs"))
        .await
        .expect("the configured token is accepted");
    assert_eq!(ids(&returned), vec![2, 3]);

    let without = Identity::token("00000000000000000000000000000000");
    let status = rows(&mut client, &without, query("docs"))
        .await
        .expect_err("an unknown token must be refused");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert!(
        !status.message().contains("harness"),
        "the refusal must not name a configured token: {status}"
    );

    // And a `trusted-header` identity does not work in token mode: the two
    // modes are not additive.
    let status = rows(&mut client, &APP, query("docs"))
        .await
        .expect_err("headers are not an identity in token mode");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

#[tokio::test]
async fn analyze_on_start_measures_the_seeded_rows() {
    let files = Files::new();
    let config = format!("{CONFIG}\n[planner]\nanalyze_on_start = true\n");
    let path = files.write("analyzed.toml", &config);
    let seed = files.write("seed.toml", SEED);
    let serving = Serving::start(&[
        "--config",
        &path.display().to_string(),
        "--seed",
        &seed.display().to_string(),
    ]);
    let mut client = connect(&serving).await;

    // Four rows were seeded; the planner's estimate for an unfiltered scan
    // should reflect the measurement rather than `TableStats::assumed`, which
    // is a thousand.
    let explained = client
        .explain(APP.on(proto::ExplainRequest {
            transaction: String::new(),
            query: Some(query("docs")),
            freshness: None,
        }))
        .await
        .expect("explain")
        .into_inner();
    assert!(
        explained.estimated_rows < 100.0,
        "the estimate should come from `analyze`, not from the default of a thousand: {}",
        explained.display
    );
}

#[tokio::test]
async fn the_local_backend_keeps_rows_across_a_restart() {
    let files = Files::new();
    let directory = files.path().join("data");
    let config = CONFIG.replace(
        "backend = \"memory\"",
        &format!(
            "backend = \"local\"\ndirectory = \"{}\"",
            directory.display()
        ),
    );
    let path = files.write("local.toml", &config);
    let seed = files.write("seed.toml", SEED);

    let first = Serving::start(&[
        "--config",
        &path.display().to_string(),
        "--seed",
        &seed.display().to_string(),
    ]);
    {
        let mut client = connect(&first).await;
        assert_eq!(
            ids(&rows(&mut client, &APP, query("docs")).await.expect("query")),
            vec![2, 3]
        );
    }
    let finished = first.terminate();
    assert_eq!(finished.code, Some(0), "stderr:\n{}", finished.stderr);

    // The shutdown resigned rather than merely exiting. Visible in the lease
    // file, which the lease writes in plain text for exactly this reason —
    // and worth asserting because the file lock would be released by the
    // process exiting whether or not anything resigned, so a restart working
    // is not on its own evidence that the shutdown was orderly.
    let lease = std::fs::read_to_string(directory.join("leases/writer")).expect("the lease file");
    assert!(
        lease.contains("(released)"),
        "the node should have resigned on the way out:\n{lease}"
    );

    // Restarted immediately, with no wait for a term to lapse: the local
    // backend's lease is a file lock, and a process that exits releases it.
    // Restarted without `--seed`: the rows have to come from the object store.
    let second = Serving::start(&["--config", &path.display().to_string()]);
    let mut client = connect(&second).await;
    assert_eq!(
        ids(&rows(&mut client, &APP, query("docs")).await.expect("query")),
        vec![2, 3]
    );
}

#[tokio::test]
async fn a_second_node_over_one_local_database_refuses_to_start() {
    // The failure this prevents is the expensive one: two writers over one
    // SlateDB take turns fencing each other, and the second would fence a
    // healthy first on its way to *discovering* that it is second. So the
    // lease is taken before the database is opened, and losing it is a refusal
    // rather than a degraded start.
    let files = Files::new();
    let directory = files.path().join("data");
    let config = CONFIG.replace(
        "backend = \"memory\"",
        &format!(
            "backend = \"local\"\ndirectory = \"{}\"",
            directory.display()
        ),
    );
    let path = files.write("local.toml", &config);

    let first = Serving::start(&["--config", &path.display().to_string()]);

    let second = harness::run(&["--config", &path.display().to_string()]);
    assert_eq!(
        second.code,
        Some(2),
        "the second node should refuse:\n{}",
        second.output()
    );
    assert!(
        second.output().contains("writer lease"),
        "the refusal should name the lease:\n{}",
        second.output()
    );
    assert!(
        second.output().contains("would fence"),
        "the refusal should say why:\n{}",
        second.output()
    );

    // And the first node is untouched: still serving, still the leader.
    let mut client = connect(&first).await;
    assert_eq!(
        ids(&rows(&mut client, &APP, query("docs")).await.expect("query")),
        Vec::<u64>::new(),
        "nothing was seeded, and the first node is still answering"
    );
    let standing = client
        .leadership(APP.on(proto::LeadershipRequest {}))
        .await
        .expect("leadership")
        .into_inner();
    assert_eq!(
        standing.standing,
        proto::leadership_status::Standing::Leader as i32
    );
}

#[tokio::test]
async fn the_lease_is_renewed_rather_than_lapsing() {
    // A three-hundred-millisecond term, so several renewals happen inside a
    // one-second wait. This is the whole point of running the real
    // `ObjectStoreLease` against an in-memory object store instead of a fake
    // lease that always grants: a renewal path that stopped working would show
    // up here rather than only against S3.
    let files = Files::new();
    let config = format!("{CONFIG}\n[lease]\nterm = \"300ms\"\n");
    let path = files.write("short.toml", &config);
    let serving = Serving::start(&["--config", &path.display().to_string()]);
    let mut client = connect(&serving).await;

    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    let standing = client
        .leadership(APP.on(proto::LeadershipRequest {}))
        .await
        .expect("leadership")
        .into_inner();
    assert_eq!(
        standing.standing,
        proto::leadership_status::Standing::Leader as i32,
        "four terms have passed; the node is still the leader only if renewal works"
    );
    // A renewal extends a term rather than taking a new one, so the generation
    // has not moved.
    assert_eq!(standing.generation, Some(1));

    // And writes still work, which is what leadership is for.
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
            rows: vec![row(vec![
                u64_value(1),
                u64_value(77),
                str_value("kind-e"),
                i64_value(30),
                null_value(),
            ])],
            ..Default::default()
        }))
        .await
        .expect("a write after several renewals");
    client
        .commit(APP.on(proto::CommitRequest { transaction }))
        .await
        .expect("commit");
}
