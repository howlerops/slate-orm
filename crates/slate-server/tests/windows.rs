//! Windows over the wire, against a real server on a real socket.
//!
//! `tests/wire.rs` proves the conversion round-trips, which is a statement
//! about two functions and says nothing about whether the daemon *runs* the
//! window it decoded or puts the answer where the protocol says. That is what
//! this file is for, and the two halves it checks are the two that can be
//! wrong independently:
//!
//! - the numbers, against the same `GROUP BY` oracle the kernel suite uses —
//!   a whole-partition aggregate must equal the grouped aggregate for that
//!   row's key, computed here through a *different RPC*, so a window that ran
//!   over the wrong set disagrees;
//! - the *placement*, because a value in the wrong list is a value the client
//!   reads as a different thing. `Row` has three lists now and the split is
//!   the whole reason a client does no ordinal arithmetic.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{app, doc, docs_query, drain, serving_leader};
use slate_kernel::memory::MemoryStore;
use slate_server::convert::column_ref;
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use std::sync::Arc;
use tonic::Code;
use tonic::transport::Channel;

const ID: u32 = 0;
const KIND: u32 = 1;
const SIZE: u32 = 2;

const ROWS: u64 = 40;

/// Four kinds, sizes that repeat, so a partition has more than one row and an
/// ordered window has real peer groups.
async fn seeded() -> common::Serving {
    let backing = Arc::new(MemoryStore::new());
    {
        let store = common::store(Arc::clone(&backing));
        let context = slate_kernel::SecurityContext::superuser();
        let table = common::docs();
        let rows: Vec<_> = (0..ROWS)
            .map(|id| {
                doc(
                    id,
                    &format!("kind-{}", id % 4),
                    (id % 5) as i64,
                    Some("note"),
                )
            })
            .collect();
        let txn = store.begin().await.unwrap();
        txn.insert_many(&context, &table, &rows).await.unwrap();
        txn.commit().await.unwrap();
    }
    serving_leader(backing).await
}

fn at(column: u32) -> pb::ColumnRef {
    column_ref(0, column as usize)
}

fn sort_asc(column: u32) -> pb::SortKey {
    pb::SortKey {
        column: Some(at(column)),
        direction: pb::SortDirection::Asc as i32,
        nulls: pb::NullsOrder::First as i32,
    }
}

fn window(function: pb::WindowFunction) -> pb::Window {
    pb::Window {
        function: function as i32,
        aggregate: None,
        column: None,
        offset: 0,
        partition_by: vec![at(KIND)],
        order: vec![sort_asc(ID)],
    }
}

async fn rows_of(client: &mut RecordsClient<Channel>, query: pb::Query) -> Vec<pb::Row> {
    let stream = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(query),
            freshness: None,
        }))
        .await
        .expect("the query should be served")
        .into_inner();
    drain(stream).await.0
}

fn u64_of(value: &pb::Value) -> u64 {
    match value.kind.as_ref() {
        Some(pb::value::Kind::Uint64Value(n)) => *n,
        other => panic!("expected a u64, got {other:?}"),
    }
}

fn i64_of(value: &pb::Value) -> i64 {
    match value.kind.as_ref() {
        Some(pb::value::Kind::Int64Value(n)) => *n,
        other => panic!("expected an i64, got {other:?}"),
    }
}

/// A window value comes back in `Row.windowed`, not folded into the columns.
///
/// The placement half. A server that appended the value to `values` would be
/// returning a row one column wider than the table, and a client reading
/// column 3 would get a rank — which is precisely the arithmetic the three
/// lists exist to remove, and precisely the failure that looks like working
/// software until somebody adds a column.
#[tokio::test]
async fn a_window_value_arrives_in_its_own_list() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let rows = rows_of(
        &mut client,
        pb::Query {
            window: vec![window(pb::WindowFunction::RowNumber)],
            ..docs_query()
        },
    )
    .await;

    assert_eq!(rows.len() as u64, ROWS);
    for row in &rows {
        assert_eq!(row.values.len(), 4, "the table's own columns: {row:?}");
        assert!(row.computed.is_empty(), "{row:?}");
        assert_eq!(row.windowed.len(), 1, "{row:?}");
    }

    // And the numbers are per partition: four kinds, ten rows each, so every
    // partition is numbered 1..=10 rather than the result being numbered
    // 1..=40 and split up.
    let mut numbers: Vec<u64> = rows.iter().map(|r| u64_of(&r.windowed[0])).collect();
    numbers.sort_unstable();
    let expected: Vec<u64> = (0..4).flat_map(|_| 1..=10).collect();
    let mut expected = expected;
    expected.sort_unstable();
    assert_eq!(numbers, expected);
}

/// The oracle: a whole-partition aggregate equals the grouped aggregate.
///
/// Computed through a *different RPC* — `Aggregate`, which folds rows away —
/// so the two answers come from two operators that share only the accumulator
/// arithmetic. The kernel suite makes the same comparison in process; this one
/// makes it across the wire, which is where a conversion can drop a partition
/// column and leave the numbers plausible.
#[tokio::test]
async fn a_partition_aggregate_matches_the_same_group_by_over_the_wire() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    let rows = rows_of(
        &mut client,
        pb::Query {
            window: vec![pb::Window {
                function: pb::WindowFunction::Aggregate as i32,
                aggregate: Some(pb::Aggregate {
                    function: pb::AggregateFunction::Sum as i32,
                    column: Some(at(SIZE)),
                }),
                column: None,
                offset: 0,
                partition_by: vec![at(KIND)],
                // No order: the whole partition, on every row.
                order: Vec::new(),
            }],
            ..docs_query()
        },
    )
    .await;

    let mut aggregate = client
        .aggregate(app(pb::AggregateRequest {
            transaction: String::new(),
            aggregate: Some(pb::AggregateQuery {
                input: Some(docs_query()),
                group_by: vec![at(KIND)],
                aggregates: vec![pb::Aggregate {
                    function: pb::AggregateFunction::Sum as i32,
                    column: Some(at(SIZE)),
                }],
                ..Default::default()
            }),
            freshness: None,
        }))
        .await
        .expect("the aggregate should be served")
        .into_inner();

    let mut groups = Vec::new();
    while let Some(message) = aggregate.message().await.expect("an aggregate message") {
        groups.extend(message.groups);
    }
    assert_eq!(groups.len(), 4, "{groups:?}");

    for group in &groups {
        let kind = &group.key[0];
        let expected = &group.values[0];
        let matching: Vec<&pb::Row> = rows
            .iter()
            .filter(|row| &row.values[KIND as usize] == kind)
            .collect();
        assert!(!matching.is_empty(), "no rows for {kind:?}");
        for row in matching {
            assert_eq!(
                i64_of(&row.windowed[0]),
                i64_of(expected),
                "window and GROUP BY disagree for {kind:?}"
            );
        }
    }
}

/// A sort can name a window, which is the one reference that can.
#[tokio::test]
async fn the_query_can_order_by_a_window_value() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let rows = rows_of(
        &mut client,
        pb::Query {
            window: vec![window(pb::WindowFunction::RowNumber)],
            sort: vec![
                pb::SortKey {
                    column: Some(pb::ColumnRef {
                        input: 0,
                        of: Some(pb::column_ref::Of::Windowed(0)),
                    }),
                    direction: pb::SortDirection::Desc as i32,
                    nulls: pb::NullsOrder::Last as i32,
                },
                sort_asc(ID),
            ],
            ..docs_query()
        },
    )
    .await;

    let numbers: Vec<u64> = rows.iter().map(|r| u64_of(&r.windowed[0])).collect();
    assert!(
        numbers.windows(2).all(|pair| pair[0] >= pair[1]),
        "not descending by the window's value: {numbers:?}"
    );
    // Not vacuous: the numbers actually differ, so "descending" is a claim.
    assert!(numbers.first() > numbers.last(), "{numbers:?}");
}

/// A filter cannot name one, and the refusal says why rather than resolving
/// the reference to some other ordinal.
#[tokio::test]
async fn a_filter_naming_a_window_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let error = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                window: vec![window(pb::WindowFunction::RowNumber)],
                filter: Some(pb::Expr {
                    node: Some(pb::expr::Node::Compare(pb::Compare {
                        column: Some(pb::ColumnRef {
                            input: 0,
                            of: Some(pb::column_ref::Of::Windowed(0)),
                        }),
                        op: pb::CmpOp::Eq as i32,
                        value: Some(pb::Value {
                            kind: Some(pb::value::Kind::Uint64Value(1)),
                        }),
                    })),
                }),
                ..docs_query()
            }),
            freshness: None,
        }))
        .await
        .expect_err("a filter over a window should be refused");
    assert_eq!(error.code(), Code::InvalidArgument);
    assert!(
        error.message().contains("after the filter"),
        "{}",
        error.message()
    );
}

/// A function carrying a field it does not use is refused, not ignored.
#[tokio::test]
async fn a_mismatched_window_field_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;

    for (name, window) in [
        (
            "a rank with an aggregate",
            pb::Window {
                aggregate: Some(pb::Aggregate {
                    function: pb::AggregateFunction::Count as i32,
                    column: None,
                }),
                ..window(pb::WindowFunction::Rank)
            },
        ),
        (
            "a rank with an offset",
            pb::Window {
                offset: 2,
                ..window(pb::WindowFunction::Rank)
            },
        ),
        (
            "an aggregate window with no aggregate",
            pb::Window {
                order: Vec::new(),
                ..window(pb::WindowFunction::Aggregate)
            },
        ),
        (
            "a window with no function",
            pb::Window {
                ..window(pb::WindowFunction::Unspecified)
            },
        ),
        (
            "LAG at offset zero",
            pb::Window {
                column: Some(at(SIZE)),
                offset: 0,
                ..window(pb::WindowFunction::Lag)
            },
        ),
    ] {
        let error = client
            .query(app(pb::QueryRequest {
                transaction: String::new(),
                query: Some(pb::Query {
                    window: vec![window],
                    ..docs_query()
                }),
                freshness: None,
            }))
            .await
            .err()
            .unwrap_or_else(|| panic!("{name} was accepted"));
        assert_eq!(error.code(), Code::InvalidArgument, "{name}");
    }
}

/// A window over a keyset page is refused, because it would restart per page.
#[tokio::test]
async fn a_window_on_a_paged_read_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let error = client
        .query(app(pb::QueryRequest {
            transaction: String::new(),
            query: Some(pb::Query {
                window: vec![window(pb::WindowFunction::RowNumber)],
                paged: true,
                limit: Some(5),
                ..docs_query()
            }),
            freshness: None,
        }))
        .await
        .expect_err("a window over a page should be refused");
    assert_eq!(error.code(), Code::InvalidArgument);
}

/// A window on a *join input* is refused, and the message says there is no
/// "put it on the join instead".
#[tokio::test]
async fn a_window_on_a_join_input_is_refused() {
    let serving = seeded().await;
    let mut client = serving.client().await;
    let error = client
        .join(app(pb::JoinRequest {
            transaction: String::new(),
            join: Some(pb::JoinQuery {
                inputs: vec![
                    pb::JoinInput {
                        query: Some(pb::Query {
                            window: vec![window(pb::WindowFunction::RowNumber)],
                            ..docs_query()
                        }),
                        ..Default::default()
                    },
                    pb::JoinInput {
                        query: Some(docs_query()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            freshness: None,
        }))
        .await
        .expect_err("a window on a join input should be refused");
    assert_eq!(error.code(), Code::InvalidArgument);
    assert!(
        error
            .message()
            .contains("neither a join nor a grouped read"),
        "{}",
        error.message()
    );
}
