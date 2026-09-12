//! The gRPC surface, end to end over a real socket.
//!
//! # The query test is a differential, not a set of examples
//!
//! `oracle.rs` in the kernel establishes that every access path returns the same
//! rows. That says nothing about this layer: a filter that loses its operator, a
//! sort that loses its direction, a limit applied before the offset — each turns
//! a correct engine into a wrong answer, and each looks like a working server.
//!
//! So the sweep below runs every query three ways. Against a filter and a sort
//! written out again in Rust, which is an independent statement of the same
//! rule. Against the planner's own choice. And against each index in turn,
//! forced, so a conversion that happens to suit one access path is caught by
//! another. Every generated sort ends in the primary key, because `ORDER BY size
//! LIMIT 5` has many correct answers and a test that does not pin ties is
//! testing the tie-breaking.
//!
//! `the_filters_select_some_but_not_all_rows` guards the guard: a sweep whose
//! predicates all match everything agrees with itself perfectly and proves
//! nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, app_in, doc, doc_ids, docs_query, drain, serving_leader, user};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{CmpOp, Expr};
use slate_schema::Ordinal;
use slate_server::convert::{Space, column_ref, expr_to_proto, row_to_proto, value_to_proto};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_tuple::Value;
use std::sync::Arc;
use std::time::Duration;
use tonic::Code;
use tonic::transport::Channel;

const ID: Ordinal = Ordinal(0);
const KIND: Ordinal = Ordinal(1);
const SIZE: Ordinal = Ordinal(2);
const NOTE: Ordinal = Ordinal(3);

/// The fixture, in a shape the oracle can read directly.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Doc {
    id: u64,
    kind: String,
    size: i64,
    note: Option<String>,
}

fn fixture() -> Vec<Doc> {
    (0..40_u64)
        .map(|id| Doc {
            id,
            kind: format!("kind-{}", id % 4),
            // Deliberately not monotonic in `id`, and with repeats, so a sort
            // on `size` is a real sort and its ties are real ties.
            size: (id as i64 * 7) % 23 - 5,
            note: (id % 3 != 0).then(|| format!("note {id}")),
        })
        .collect()
}

async fn seeded() -> (common::Serving, Vec<Doc>) {
    let docs = fixture();
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let table = common::docs();
        let rows: Vec<_> = docs
            .iter()
            .map(|d| doc(d.id, &d.kind, d.size, d.note.as_deref()))
            .collect();
        let txn = store.begin().await.unwrap();
        txn.insert_many(&context, &table, &rows).await.unwrap();
        txn.commit().await.unwrap();
    }
    (serving_leader(backing).await, docs)
}

// --- the differential sweep -----------------------------------------------

/// A predicate, stated twice: once for the server and once for the oracle.
struct Filter {
    name: &'static str,
    wire: Expr,
    holds: fn(&Doc) -> bool,
}

fn filters() -> Vec<Filter> {
    vec![
        Filter {
            name: "everything",
            wire: Expr::True,
            holds: |_| true,
        },
        Filter {
            name: "kind = kind-1",
            wire: Expr::eq(KIND, Value::Str("kind-1".to_owned())),
            holds: |d| d.kind == "kind-1",
        },
        Filter {
            name: "size >= 5",
            wire: Expr::compare(SIZE, CmpOp::Ge, Value::I64(5)),
            holds: |d| d.size >= 5,
        },
        Filter {
            name: "size < 5 and kind = kind-2",
            wire: Expr::compare(SIZE, CmpOp::Lt, Value::I64(5))
                .and(Expr::eq(KIND, Value::Str("kind-2".to_owned()))),
            holds: |d| d.size < 5 && d.kind == "kind-2",
        },
        Filter {
            name: "note is null",
            wire: Expr::is_null(NOTE),
            holds: |d| d.note.is_none(),
        },
        Filter {
            // Unanchored, so the planner cannot turn it into scan bounds and
            // it has to survive as a residual on every access path.
            name: "kind like %-2",
            wire: Expr::like(KIND, "%-2"),
            holds: |d| d.kind.ends_with("-2"),
        },
        Filter {
            name: "id in (1, 5, 9, 40)",
            wire: Expr::In {
                column: ID,
                values: vec![Value::U64(1), Value::U64(5), Value::U64(9), Value::U64(40)],
            },
            holds: |d| matches!(d.id, 1 | 5 | 9 | 40),
        },
    ]
}

/// A sort, stated twice. The primary key is appended to every one so the
/// expected answer is a single sequence rather than a set of legal ones.
struct Sort {
    name: &'static str,
    keys: Vec<pb::SortKey>,
    order: fn(&Doc, &Doc) -> std::cmp::Ordering,
}

fn sort_key(column: Ordinal, descending: bool) -> pb::SortKey {
    pb::SortKey {
        column: Some(column_ref(0, column.0)),
        direction: if descending {
            pb::SortDirection::Desc as i32
        } else {
            pb::SortDirection::Asc as i32
        },
        nulls: pb::NullsOrder::Unspecified as i32,
    }
}

fn sorts() -> Vec<Sort> {
    vec![
        Sort {
            name: "id",
            keys: vec![sort_key(ID, false)],
            order: |a, b| a.id.cmp(&b.id),
        },
        Sort {
            name: "size, id",
            keys: vec![sort_key(SIZE, false), sort_key(ID, false)],
            order: |a, b| a.size.cmp(&b.size).then(a.id.cmp(&b.id)),
        },
        Sort {
            name: "size desc, id",
            keys: vec![sort_key(SIZE, true), sort_key(ID, false)],
            order: |a, b| b.size.cmp(&a.size).then(a.id.cmp(&b.id)),
        },
        Sort {
            name: "kind, id",
            keys: vec![sort_key(KIND, false), sort_key(ID, false)],
            order: |a, b| a.kind.cmp(&b.kind).then(a.id.cmp(&b.id)),
        },
    ]
}

/// Every access path the planner could pick, plus letting it pick.
fn access_paths() -> Vec<(&'static str, Option<pb::AccessHint>)> {
    vec![
        ("the planner's choice", None),
        (
            "a forced table scan",
            Some(pb::AccessHint {
                path: Some(pb::access_hint::Path::TableScan(true)),
            }),
        ),
        (
            "forced through by_kind",
            Some(pb::AccessHint {
                path: Some(pb::access_hint::Path::Index("by_kind".to_owned())),
            }),
        ),
        (
            "forced through by_size",
            Some(pb::AccessHint {
                path: Some(pb::access_hint::Path::Index("by_size".to_owned())),
            }),
        ),
    ]
}

async fn ids_from(client: &mut RecordsClient<Channel>, query: pb::Query) -> Vec<u64> {
    let stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(query),
            freshness: None,
        }))
        .await
        .expect("the query should be served")
        .into_inner();
    let (rows, _) = drain(stream).await;
    doc_ids(&rows)
}

#[tokio::test]
async fn every_access_path_agrees_with_an_oracle_written_out_by_hand() {
    let (serving, docs) = seeded().await;
    let mut client = serving.client().await;
    let mut checked = 0;

    for filter in filters() {
        for sort in sorts() {
            for (limit, offset) in [(None, 0_u64), (Some(5_u64), 0), (None, 3), (Some(5), 3)] {
                // The oracle: filter, sort, drop, take — the obvious
                // implementation, written here rather than shared with the one
                // under test.
                let mut expected: Vec<&Doc> = docs.iter().filter(|d| (filter.holds)(d)).collect();
                expected.sort_by(|a, b| (sort.order)(a, b));
                let expected: Vec<u64> = expected
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit.map_or(usize::MAX, |l| l as usize))
                    .map(|d| d.id)
                    .collect();

                for (path, hint) in access_paths() {
                    let query = pb::Query {
                        filter: Some(expr_to_proto(&Space::table(&common::docs()), &filter.wire)),
                        sort: sort.keys.clone(),
                        limit,
                        offset,
                        hint: hint.clone(),
                        ..docs_query()
                    };
                    let actual = ids_from(&mut client, query).await;
                    assert_eq!(
                        actual, expected,
                        "`{}` sorted by `{}` limit {limit:?} offset {offset}, via {path}",
                        filter.name, sort.name
                    );
                    checked += 1;
                }
            }
        }
    }

    assert!(checked >= 400, "the sweep only ran {checked} queries");
}

/// A sweep whose predicates all match everything agrees with itself perfectly
/// and proves nothing about filtering.
#[test]
fn the_filters_select_some_but_not_all_rows() {
    let docs = fixture();
    for filter in filters() {
        if filter.name == "everything" {
            continue;
        }
        let matched = docs.iter().filter(|d| (filter.holds)(d)).count();
        assert!(
            matched > 0 && matched < docs.len(),
            "`{}` selects {matched} of {} rows, so it tests nothing",
            filter.name,
            docs.len()
        );
    }
}

/// And a sweep whose sorts all produce the same order tests nothing about
/// sorting.
#[test]
fn the_sorts_produce_different_orders() {
    let docs = fixture();
    let orders: Vec<Vec<u64>> = sorts()
        .iter()
        .map(|sort| {
            let mut sorted: Vec<&Doc> = docs.iter().collect();
            sorted.sort_by(|a, b| (sort.order)(a, b));
            sorted.into_iter().map(|d| d.id).collect()
        })
        .collect();
    for (i, a) in orders.iter().enumerate() {
        for b in orders.iter().skip(i + 1) {
            assert_ne!(a, b, "two sorts in the sweep produce the same order");
        }
    }
}

// --- projections ----------------------------------------------------------

#[tokio::test]
async fn a_projection_returns_the_named_columns_and_the_key() {
    // Columns outside the projection come back null, which is the kernel's
    // documented behaviour and worth pinning here because it is the shape of
    // the covering-index bug the planner oracle found: a row whose contents
    // depend on which plan fetched it.
    let (serving, _) = seeded().await;
    let mut client = serving.client().await;

    let stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                filter: Some(expr_to_proto(
                    &Space::table(&common::docs()),
                    &Expr::eq(ID, Value::U64(7)),
                )),
                projection: Some(pb::Projection {
                    all_columns: false,
                    columns: vec![column_ref(0, KIND.0)],
                }),
                ..docs_query()
            }),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();
    let (rows, _) = drain(stream).await;

    assert_eq!(rows.len(), 1);
    let values = &rows[0].values;
    assert_eq!(
        values[ID.0].kind,
        Some(pb::value::Kind::Uint64Value(7)),
        "the primary key is decoded from the key and always comes back"
    );
    assert_eq!(
        values[KIND.0].kind,
        Some(pb::value::Kind::StringValue("kind-3".to_owned()))
    );
    assert!(
        matches!(values[NOTE.0].kind, Some(pb::value::Kind::NullValue(_))),
        "a column outside the projection should be null, was {:?}",
        values[NOTE.0]
    );
}

// --- explain --------------------------------------------------------------

#[tokio::test]
async fn explain_reports_the_path_and_whether_it_reads_rows() {
    let (serving, _) = seeded().await;
    let mut client = serving.client().await;

    let covering = client
        .explain(app(pb::ExplainRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                filter: Some(expr_to_proto(
                    &Space::table(&common::docs()),
                    &Expr::eq(KIND, Value::Str("kind-1".to_owned())),
                )),
                // `by_kind` holds the kind and the primary key, so this needs
                // no row read at all.
                projection: Some(pb::Projection {
                    all_columns: false,
                    columns: vec![column_ref(0, ID.0), column_ref(0, KIND.0)],
                }),
                hint: Some(pb::AccessHint {
                    path: Some(pb::access_hint::Path::Index("by_kind".to_owned())),
                }),
                ..docs_query()
            }),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();

    assert!(
        covering.index_only,
        "a covering projection should not read rows: {}",
        covering.display
    );
    assert!(covering.access.contains("by_kind"), "{}", covering.access);
    assert!(
        covering.estimated_cost > 0.0,
        "an explanation with no cost tells an operator nothing"
    );
    assert!(
        covering.residual.contains("True") || !covering.residual.is_empty(),
        "the residual should be reported: {:?}",
        covering.residual
    );
}

#[tokio::test]
async fn explain_says_when_it_ignored_a_hint() {
    let (serving, _) = seeded().await;
    let mut client = serving.client().await;

    let explained = client
        .explain(app(pb::ExplainRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                hint: Some(pb::AccessHint {
                    path: Some(pb::access_hint::Path::Index("by_nothing".to_owned())),
                }),
                ..docs_query()
            }),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(explained.warnings.len(), 1, "{:?}", explained.warnings);
    assert!(explained.warnings[0].contains("by_nothing"));
}

/// The planner behind a read uses the statistics the node was configured with.
///
/// `estimated_rows` is the visible end of them. Worth pinning because the read
/// path no longer keeps its own copy of the statistics — it reads the replica
/// pool's, which is the only place they are now stated — and the failure mode
/// of getting that wrong is quiet: a node planning against `TableStats`'s
/// assumed thousand rows while the operator believes it configured half a
/// million produces plans that merely look wrong. This project has already paid
/// for that shape once, with a latency fixture and a cost model that both
/// stated rows per block and drifted.
///
/// The default-configured node is the control: without it, an assertion that
/// the estimate is large would also pass on a node that ignored the
/// configuration and happened to guess high.
#[tokio::test]
async fn a_read_is_planned_with_the_statistics_the_node_was_configured_with() {
    use slate_kernel::{Statistics, TableStats};
    use slate_server::{HeadConfig, MetadataIdentity};

    async fn estimate(config: HeadConfig) -> f64 {
        let leadership = slate_server::Leadership::new(Arc::new(common::AlwaysLeader::default()));
        leadership.campaign().await;
        let serving = common::serve(slate_server::Head::new(
            config,
            Arc::new(MemoryStore::new()),
            Vec::new(),
            leadership,
            Arc::new(MetadataIdentity::trusting_the_caller_completely()),
        ))
        .await;
        serving
            .client()
            .await
            .explain(app(pb::ExplainRequest {
                transaction: String::new(),
                query: Some(docs_query()),
                freshness: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .estimated_rows
    }

    let assumed = estimate(HeadConfig::new(common::catalog(), common::security())).await;
    let configured = estimate(
        HeadConfig::new(common::catalog(), common::security()).with_statistics(
            Statistics::new().with(common::DOCS, TableStats::with_row_count(500_000)),
        ),
    )
    .await;

    assert!(
        (assumed - 1_000.0).abs() < 1.0,
        "the control node should be planning against the assumed thousand rows, not {assumed}"
    );
    assert!(
        configured > 400_000.0,
        "a full scan of a table configured at 500,000 rows was estimated at {configured}; the statistics did not reach the read path"
    );
}

// --- writes and transactions ----------------------------------------------

#[tokio::test]
async fn a_single_statement_write_commits_and_returns_its_sequence() {
    let serving = serving_leader(Arc::new(MemoryStore::new())).await;
    let mut client = serving.client().await;

    let written = client
        .insert(app(pb::InsertRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            rows: vec![row_to_proto(&doc(1, "kind-a", 3, Some("hello")))],
            upsert: false,
        }))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(written.affected, 1);
    let sequence = written
        .sequence
        .expect("a committed write must return the sequence it landed at");

    let found = client
        .get(app(pb::GetRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            primary_key: Some(pb::Row {
                values: vec![value_to_proto(&Value::U64(1))],
            }),
            freshness: Some(pb::Freshness {
                level: Some(pb::freshness::Level::AtLeast(sequence)),
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(found.found);
}

#[tokio::test]
async fn a_rolled_back_transaction_leaves_nothing() {
    let serving = serving_leader(Arc::new(MemoryStore::new())).await;
    let mut client = serving.client().await;

    let transaction = client
        .begin(app(pb::BeginRequest {}))
        .await
        .unwrap()
        .into_inner()
        .transaction;

    client
        .insert(app(pb::InsertRequest {
            transaction: transaction.clone(),
            table: "docs".to_owned(),
            rows: vec![row_to_proto(&doc(1, "kind-a", 3, None))],
            upsert: false,
        }))
        .await
        .unwrap();

    // Visible inside the transaction...
    let inside = client
        .get(app(pb::GetRequest {
            transaction: transaction.clone(),
            table: "docs".to_owned(),
            primary_key: Some(pb::Row {
                values: vec![value_to_proto(&Value::U64(1))],
            }),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(inside.found, "a transaction must see its own writes");

    client
        .rollback(app(pb::RollbackRequest {
            transaction: transaction.clone(),
        }))
        .await
        .unwrap();

    let outside = client
        .get(app(pb::GetRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            primary_key: Some(pb::Row {
                values: vec![value_to_proto(&Value::U64(1))],
            }),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!outside.found, "a rolled-back write became visible");

    // And the handle is gone.
    let reused = client
        .commit(app(pb::CommitRequest { transaction }))
        .await
        .expect_err("a rolled-back transaction cannot be committed");
    assert!(matches!(reused.code(), Code::NotFound | Code::Aborted));
}

#[tokio::test]
async fn a_transaction_belongs_to_the_principal_that_opened_it() {
    // The handle is a capability, and capabilities leak through logs, proxies
    // and traces. The refusal is the same NOT_FOUND an unknown handle gets,
    // because "that transaction exists but is not yours" is a fact worth not
    // disclosing.
    let serving = serving_leader(Arc::new(MemoryStore::new())).await;
    let mut client = serving.client().await;

    let transaction = client
        .begin(app(pb::BeginRequest {}))
        .await
        .unwrap()
        .into_inner()
        .transaction;

    let stolen = pb::InsertRequest {
        transaction: transaction.clone(),
        table: "docs".to_owned(),
        rows: vec![row_to_proto(&doc(1, "kind-a", 3, None))],
        upsert: false,
    };
    let error = client
        .insert(common::as_principal(stolen, "u64:2", None, "app"))
        .await
        .expect_err("another principal must not use this transaction");
    assert_eq!(error.code(), Code::NotFound);

    let unknown = client
        .commit(common::as_principal(
            pb::CommitRequest {
                transaction: "3f2504e0-4f89-41d3-9a0c-0305e82c3301".to_owned(),
            },
            "u64:2",
            None,
            "app",
        ))
        .await
        .expect_err("an unknown handle");
    assert_eq!(
        error.code(),
        unknown.code(),
        "someone else's handle and an unknown handle must be indistinguishable"
    );
    assert_eq!(error.message(), unknown.message());

    // The owner is unaffected.
    client
        .rollback(app(pb::RollbackRequest { transaction }))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_idle_transaction_is_rolled_back_rather_than_pinning_a_snapshot() {
    use slate_server::{HeadConfig, Limits, MetadataIdentity};

    let backing = Arc::new(MemoryStore::new());
    let leadership = slate_server::Leadership::new(Arc::new(common::AlwaysLeader::default()));
    leadership.campaign().await;

    let head = slate_server::Head::new(
        HeadConfig::new(common::catalog(), common::security()).with_limits(Limits {
            idle_timeout: Duration::from_millis(50),
            ..Limits::default()
        }),
        Arc::clone(&backing),
        Vec::new(),
        leadership,
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    );
    let serving = common::serve(head).await;
    let mut client = serving.client().await;

    let transaction = client
        .begin(app(pb::BeginRequest {}))
        .await
        .unwrap()
        .into_inner()
        .transaction;
    client
        .insert(app(pb::InsertRequest {
            transaction: transaction.clone(),
            table: "docs".to_owned(),
            rows: vec![row_to_proto(&doc(1, "kind-a", 3, None))],
            upsert: false,
        }))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;

    let error = client
        .commit(app(pb::CommitRequest { transaction }))
        .await
        .expect_err("an abandoned transaction must not still be committable");
    assert!(matches!(error.code(), Code::NotFound | Code::Aborted));
    assert!(
        backing.is_empty(),
        "the abandoned transaction's writes were committed anyway"
    );
}

#[tokio::test]
async fn a_node_will_not_open_more_transactions_than_its_limit() {
    use slate_server::{HeadConfig, Limits, MetadataIdentity};

    let leadership = slate_server::Leadership::new(Arc::new(common::AlwaysLeader::default()));
    leadership.campaign().await;
    let head = slate_server::Head::new(
        HeadConfig::new(common::catalog(), common::security()).with_limits(Limits {
            max_transactions: 3,
            ..Limits::default()
        }),
        Arc::new(MemoryStore::new()),
        Vec::new(),
        leadership,
        Arc::new(MetadataIdentity::trusting_the_caller_completely()),
    );
    let serving = common::serve(head).await;
    let mut client = serving.client().await;

    for _ in 0..3 {
        client.begin(app(pb::BeginRequest {})).await.unwrap();
    }
    let error = client
        .begin(app(pb::BeginRequest {}))
        .await
        .expect_err("the fourth must be refused");
    assert_eq!(error.code(), Code::ResourceExhausted);
}

#[tokio::test]
async fn a_duplicate_unique_value_is_already_exists_not_internal() {
    let serving = serving_leader(Arc::new(MemoryStore::new())).await;
    let mut client = serving.client().await;

    let insert = |id: u64| pb::InsertRequest {
        transaction: String::new(),
        table: "users".to_owned(),
        rows: vec![row_to_proto(&user(1, id, 7, "taken@example.com"))],
        upsert: false,
    };
    client.insert(app_in(insert(1), 7, 1)).await.unwrap();

    let error = client
        .insert(app_in(insert(2), 7, 1))
        .await
        .expect_err("the email is taken");
    assert_eq!(error.code(), Code::AlreadyExists);
}

#[tokio::test]
async fn deleting_reports_how_many_rows_were_there() {
    let (serving, _) = seeded().await;
    let mut client = serving.client().await;

    let key = |id: u64| pb::Row {
        values: vec![value_to_proto(&Value::U64(id))],
    };
    let deleted = client
        .delete(app(pb::DeleteRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            // One that exists, one that never did.
            primary_keys: vec![key(3), key(9999)],
        }))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(deleted.affected, 1);
    assert!(deleted.sequence.is_some());
}

// --- security -------------------------------------------------------------

#[tokio::test]
async fn a_caller_with_no_role_gets_nothing() {
    let (serving, _) = seeded().await;
    let mut client = serving.client().await;

    let error = client
        .query(common::as_principal(
            pb::QueryRequest {
                transaction: String::new(),
                query: Some(docs_query()),
                freshness: None,
            },
            "u64:1",
            None,
            "",
        ))
        .await
        .expect_err("no role grants read on docs");
    assert_eq!(error.code(), Code::PermissionDenied);
}

#[tokio::test]
async fn a_caller_cannot_ask_to_be_a_superuser() {
    // `SecurityContext::superuser` bypasses every check. Nothing this crate
    // ships can produce one, and this pins that a client naming likely-looking
    // roles does not get one either.
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let table = common::users();
        let txn = store.begin().await.unwrap();
        for (id, owner) in [(1_u64, 7_u64), (2, 8)] {
            txn.insert(&context, &table, &user(1, id, owner, &format!("u{id}@x")))
                .await
                .unwrap();
        }
        txn.commit().await.unwrap();
    }
    let serving = serving_leader(backing).await;
    let mut client = serving.client().await;

    let query = pb::QueryRequest {
        transaction: String::new(),
        query: Some(pb::Query {
            table: "users".to_owned(),
            ..docs_query()
        }),
        freshness: None,
    };
    let stream = client
        .query(common::as_principal(
            query,
            "u64:7",
            Some("u64:1"),
            "app,superuser,root,admin",
        ))
        .await
        .expect("the app role still grants read")
        .into_inner();
    let (rows, _) = drain(stream).await;

    assert_eq!(
        rows.len(),
        1,
        "a caller claiming administrative roles saw {} rows; the policy admits one",
        rows.len()
    );
}

#[tokio::test]
async fn a_row_policy_applies_to_a_query_over_the_wire() {
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let table = common::users();
        let txn = store.begin().await.unwrap();
        for (tenant, id, owner) in [(1_u64, 1_u64, 7_u64), (1, 2, 8), (2, 3, 7)] {
            txn.insert(
                &context,
                &table,
                &user(tenant, id, owner, &format!("u{tenant}-{id}@x")),
            )
            .await
            .unwrap();
        }
        txn.commit().await.unwrap();
    }
    let serving = serving_leader(backing).await;
    let mut client = serving.client().await;

    let stream = client
        .query(app_in(
            pb::QueryRequest {
                transaction: String::new(),
                query: Some(pb::Query {
                    table: "users".to_owned(),
                    ..docs_query()
                }),
                freshness: None,
            },
            7,
            1,
        ))
        .await
        .unwrap()
        .into_inner();
    let (rows, _) = drain(stream).await;

    // Owner 8's row is hidden by the policy; owner 7's row in tenant 2 is
    // hidden by the tenant restriction. One row is left.
    assert_eq!(rows.len(), 1, "the policy or the tenant scope leaked");
    assert_eq!(
        rows[0].values[1].kind,
        Some(pb::value::Kind::Uint64Value(1))
    );
}

#[tokio::test]
async fn a_request_with_no_identity_is_unauthenticated() {
    let (serving, _) = seeded().await;
    let mut client = serving.client().await;

    let error = client
        .query(tonic::Request::new(pb::QueryRequest {
            transaction: String::new(),
            query: Some(docs_query()),
            freshness: None,
        }))
        .await
        .expect_err("no identity");
    assert_eq!(error.code(), Code::Unauthenticated);
}

#[tokio::test]
async fn an_identity_with_no_type_tag_is_refused() {
    // A principal id of `7` could be a u64 or a string, and `Value`'s order is
    // type-first, so the two are different principals. Guessing would make a
    // policy match or not depending on whether an id happened to look numeric.
    let (serving, _) = seeded().await;
    let mut client = serving.client().await;

    let error = client
        .query(common::as_principal(
            pb::QueryRequest {
                transaction: String::new(),
                query: Some(docs_query()),
                freshness: None,
            },
            "7",
            None,
            "app",
        ))
        .await
        .expect_err("no type tag");
    assert_eq!(error.code(), Code::Unauthenticated);
    assert!(error.message().contains("type tag"), "{}", error.message());
}

// --- malformed requests ---------------------------------------------------

#[tokio::test]
async fn an_unknown_table_is_not_found() {
    let (serving, _) = seeded().await;
    let mut client = serving.client().await;

    let error = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                table: "no_such_table".to_owned(),
                ..docs_query()
            }),
            freshness: None,
        }))
        .await
        .expect_err("no such table");
    assert_eq!(error.code(), Code::NotFound);
}

#[tokio::test]
async fn a_value_with_no_kind_is_refused_at_the_boundary() {
    let (serving, _) = seeded().await;
    let mut client = serving.client().await;

    let error = client
        .get(app(pb::GetRequest {
            transaction: String::new(),
            table: "docs".to_owned(),
            primary_key: Some(pb::Row {
                values: vec![pb::Value { kind: None }],
            }),
            freshness: None,
        }))
        .await
        .expect_err("an unset value is not a null");
    assert_eq!(error.code(), Code::InvalidArgument);
}
