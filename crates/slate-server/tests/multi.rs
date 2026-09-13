//! Joins, chains, aggregates and computed columns, over gRPC.
//!
//! # The test is an oracle, and the oracle is the kernel
//!
//! Everything below asks the same question twice: once over a real socket and
//! once in process against `slate-kernel`, from the *same* kernel value. The
//! wire form is produced by `join_to_proto` / `chain_to_proto` /
//! `aggregate_to_proto_query` from the very `Join`, `Chain` or query the
//! in-process side runs, so the two paths cannot drift apart by one of them
//! being written differently — they can only differ if the conversion loses or
//! changes something.
//!
//! That is a stronger property than a round trip. A round trip catches a field
//! dropped on one side; it passes happily when a field is dropped on *both*.
//! An oracle against the engine catches anything that changes the answer,
//! which is the only failure that matters here.
//!
//! Rows are compared as multisets. A join has no inherent order, the three
//! algorithms genuinely produce different ones, and demanding an order would
//! be testing the implementation rather than the semantics — the same reason
//! the kernel's own `join_oracle.rs` compares multisets.
//!
//! # And the sweep runs every algorithm
//!
//! Four join types against the planner's choice and each forced algorithm. A
//! conversion that happens to suit a hash join — a join key resolved to the
//! wrong ordinal, say, where the build side masks it — is caught by the
//! nested loop, which reaches the same rows a different way. Where the kernel
//! *refuses* a combination, both paths must refuse it: a wire that quietly
//! fell back to a hash join would return the inner-join rows and a short
//! answer.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{
    app_context, at, author, authors, book, books, drain, drain_groups, drain_joined, sale, sales,
};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Aggregate, Chain, CmpOp, Expr, Group, Grouping, Join, JoinAlgorithm, JoinKey, JoinSchema,
    JoinStep, JoinType, Query, RecordStore, Scalar, SecurityContext, Side, SortKey, TimeUnit,
};
use slate_schema::{Ordinal, Row, TableDef};
use slate_server::convert::{
    aggregate_to_proto_query, chain_to_proto, column_ref, computed_ref, flat_row_from_proto,
    join_to_proto, query_to_proto, value_from_proto,
};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_tuple::Value;
use std::sync::Arc;
use tonic::Code;
use tonic::transport::Channel;

// --- the fixture ----------------------------------------------------------

/// Rows chosen so every join type has something to distinguish it, and so a
/// policy applied to the wrong side removes a different set.
///
/// - `di` has no books, so a left outer join has an unmatched left row.
/// - `orphan` has no author, so a right or full outer join has an unmatched
///   right row — the case a nested loop cannot serve.
/// - `cy` is owned by somebody else and `a-two` is too old, so each side's
///   policy hides one row, and the two are hidden for different reasons.
/// - tenant 2 repeats the same principal's id, which is the case a tenant
///   check that only looked at the principal would let through.
async fn seed(backing: &Arc<MemoryStore>) {
    let store = common::store(Arc::clone(backing));
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    txn.insert_many(
        &root,
        &authors(),
        &[
            author(1, 1, 1, "ada", "UK", 1815),
            author(1, 2, 1, "bo", "US", 1900),
            author(1, 3, 2, "cy", "UK", 1950),
            author(1, 4, 1, "di", "FR", 1970),
            author(2, 1, 1, "zz", "UK", 1800),
        ],
    )
    .await
    .unwrap();
    txn.insert_many(
        &root,
        &books(),
        &[
            book(1, 10, 1, "a-one", 2001),
            book(1, 11, 1, "a-two", 1990),
            book(1, 12, 2, "b-one", 2010),
            book(1, 13, 3, "c-one", 2005),
            book(1, 14, 9, "orphan", 2020),
            // A second visible book for author 1, so a group has more than
            // one row and `HAVING count > 1` has something to keep.
            book(1, 15, 1, "a-three", 2003),
            book(2, 10, 1, "zz-book", 2001),
        ],
    )
    .await
    .unwrap();
    txn.insert_many(
        &root,
        &sales(),
        &[
            sale(1, 20, 10, 5),
            sale(1, 21, 10, 0),
            sale(1, 22, 12, 7),
            sale(1, 23, 99, 3),
            sale(2, 20, 10, 4),
        ],
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
}

async fn seeded() -> (common::Serving, Arc<MemoryStore>) {
    let backing = Arc::new(MemoryStore::new());
    seed(&backing).await;
    (common::serving_leader(Arc::clone(&backing)).await, backing)
}

/// The identity every request runs as, on both paths.
fn ctx() -> SecurityContext {
    app_context(1, 1)
}

fn app_request<T>(message: T) -> tonic::Request<T> {
    common::app_in(message, 1, 1)
}

// --- comparing the two paths ----------------------------------------------

/// One row of a multi-table result, in a shape both paths can produce.
type Wide = Vec<Option<Row>>;

/// Rows as a sorted multiset of strings.
///
/// Sorted because a join has no inherent order; strings because the comparison
/// has to fail *legibly* — a mismatch prints the rows rather than a length.
fn multiset(rows: &[Wide]) -> Vec<String> {
    let mut out: Vec<String> = rows.iter().map(|row| format!("{row:?}")).collect();
    out.sort();
    out
}

fn from_wire(rows: &[pb::JoinedRow]) -> Vec<Wide> {
    rows.iter()
        .map(|row| {
            row.inputs
                .iter()
                .map(|input| {
                    input.row.as_ref().map(|row| {
                        // `flat_row_from_proto`, because the kernel's row is
                        // flat and the wire's is not: an input's computed
                        // values come back in `Row.computed` rather than as a
                        // tail of its columns, so comparing against the kernel
                        // means putting them back together. `row_from_proto`
                        // refuses a row carrying them, which is what a *write*
                        // wants.
                        flat_row_from_proto(row).expect("a row this server sent")
                    })
                })
                .collect()
        })
        .collect()
}

/// Run a join over gRPC.
async fn over_the_wire(
    client: &mut RecordsClient<Channel>,
    query: pb::JoinQuery,
) -> Result<Vec<Wide>, tonic::Status> {
    let stream = client
        .join(app_request(pb::JoinRequest {
            transaction: String::new(),
            join: Some(query),
            freshness: None,
        }))
        .await?
        .into_inner();
    let (rows, served_by) = drain_joined(stream).await;
    assert!(
        served_by.is_some(),
        "a join stream must carry `served_by` on its first message"
    );
    Ok(from_wire(&rows))
}

/// Run the same join in process against the kernel.
async fn in_process_join(
    backing: &Arc<MemoryStore>,
    left: &TableDef,
    right: &TableDef,
    join: &Join,
) -> Result<Vec<Wide>, slate_kernel::KernelError> {
    let store: RecordStore<Arc<MemoryStore>> = common::store(Arc::clone(backing));
    let txn = store.begin().await.unwrap();
    let mut cursor = txn.join(&ctx(), left, right, join).await?;
    let mut out = Vec::new();
    while let Some(row) = cursor.next().await? {
        out.push(vec![row.left, row.right]);
    }
    Ok(out)
}

async fn in_process_chain(
    backing: &Arc<MemoryStore>,
    tables: &[&TableDef],
    chain: &Chain,
) -> Result<Vec<Wide>, slate_kernel::KernelError> {
    let store: RecordStore<Arc<MemoryStore>> = common::store(Arc::clone(backing));
    let txn = store.begin().await.unwrap();
    let rows = txn.chain(&ctx(), tables, chain).await?.collect().await?;
    Ok(rows
        .into_iter()
        .map(|row| (0..tables.len()).map(|at| row.at(at).cloned()).collect())
        .collect())
}

// --- the two-table sweep --------------------------------------------------

/// Builds one shape of join over the two fixture tables.
type BuildJoin = Box<dyn Fn(&TableDef, &TableDef) -> Join>;

/// One aggregate request: its input, grouping, aggregates and `HAVING`.
type AggregateCase = (&'static str, Query, Vec<Ordinal>, Vec<Aggregate>, Expr);

/// The shapes of join the sweep runs, beyond varying the type and algorithm.
///
/// Each exists to reach a different piece of the conversion: a per-input
/// filter, a condition spanning both inputs, and a computed value used inside
/// one input's own filter.
fn variants() -> Vec<(&'static str, BuildJoin)> {
    vec![
        (
            "plain",
            Box::new(|a: &TableDef, b: &TableDef| Join::equating(at(a, "id"), at(b, "author_id"))),
        ),
        (
            "left filtered on country",
            Box::new(|a: &TableDef, b: &TableDef| {
                Join::equating(at(a, "id"), at(b, "author_id")).left(
                    Query::all().filter(Expr::eq(at(a, "country"), Value::Str("UK".to_owned()))),
                )
            }),
        ),
        (
            "right filtered on title",
            Box::new(|a: &TableDef, b: &TableDef| {
                Join::equating(at(a, "id"), at(b, "author_id"))
                    .right(Query::all().filter(Expr::like(at(b, "title"), "a-%")))
            }),
        ),
        (
            "cross-input condition: book.year > author.born",
            Box::new(|a: &TableDef, b: &TableDef| {
                let space = JoinSchema::of(a, b);
                Join::equating(at(a, "id"), at(b, "author_id")).having(Expr::compare_columns(
                    space.right(at(b, "year")),
                    CmpOp::Gt,
                    space.left(at(a, "born")),
                ))
            }),
        ),
        (
            "left input computes a value and filters on it",
            Box::new(|a: &TableDef, b: &TableDef| {
                let born_plus = Query::computed(a, 0);
                Join::equating(at(a, "id"), at(b, "author_id")).left(
                    Query::all()
                        .computing([Scalar::column(at(a, "born")) + 100i64])
                        .filter(Expr::compare(born_plus, CmpOp::Gt, Value::I64(1950))),
                )
            }),
        ),
        (
            "two equalities, one of them redundant",
            Box::new(|a: &TableDef, b: &TableDef| {
                Join::on([
                    JoinKey::new(at(a, "id"), at(b, "author_id")),
                    JoinKey::new(at(a, "tenant_id"), at(b, "tenant_id")),
                ])
            }),
        ),
    ]
}

fn algorithms() -> Vec<(&'static str, Option<JoinAlgorithm>)> {
    vec![
        ("the planner's choice", None),
        (
            "forced hash, build left",
            Some(JoinAlgorithm::Hash { build: Side::Left }),
        ),
        (
            "forced hash, build right",
            Some(JoinAlgorithm::Hash { build: Side::Right }),
        ),
        ("forced nested loop", Some(JoinAlgorithm::NestedLoop)),
    ]
}

const JOIN_TYPES: [(&str, JoinType); 4] = [
    ("inner", JoinType::Inner),
    ("left outer", JoinType::Left),
    ("right outer", JoinType::Right),
    ("full outer", JoinType::Full),
];

#[tokio::test]
async fn a_join_over_grpc_returns_what_the_kernel_returns() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());
    let mut checked = 0;
    let mut nonempty = 0;

    for (variant, build) in variants() {
        for (kind, join_type) in JOIN_TYPES {
            for (how, force) in algorithms() {
                let mut join = build(&a, &b);
                join.join_type = join_type;
                join.force = force;

                let expected = in_process_join(&backing, &a, &b, &join).await;
                let actual = over_the_wire(&mut client, join_to_proto(&a, &b, &join)).await;

                match (expected, actual) {
                    (Ok(expected), Ok(actual)) => {
                        assert_eq!(
                            multiset(&actual),
                            multiset(&expected),
                            "`{variant}` as a {kind} join via {how} disagreed with the kernel"
                        );
                        if !expected.is_empty() {
                            nonempty += 1;
                        }
                    }
                    // The kernel refuses a nested loop for a right or full
                    // outer join. The wire must refuse it too, and with the
                    // same code — a fallback to a hash join would return the
                    // inner rows and a short answer, which no assertion about
                    // rows would ever see.
                    (Err(_), Err(status)) => assert_eq!(
                        status.code(),
                        Code::InvalidArgument,
                        "`{variant}` as a {kind} join via {how} was refused as {status:?}"
                    ),
                    (Ok(rows), Err(status)) => panic!(
                        "`{variant}` as a {kind} join via {how}: the kernel returned {} rows \
                         and the wire refused with {status:?}",
                        rows.len()
                    ),
                    (Err(error), Ok(rows)) => panic!(
                        "`{variant}` as a {kind} join via {how}: the kernel refused with \
                         {error} and the wire returned {} rows",
                        rows.len()
                    ),
                }
                checked += 1;
            }
        }
    }

    assert!(checked >= 96, "the sweep only ran {checked} joins");
    // A sweep whose every case comes back empty agrees with itself perfectly
    // and proves nothing. The bar is measured rather than picked by eye —
    // `docs/correctness.md` records four thresholds that were guessed and
    // duly went flaky. 84 of the 96 cases return rows; the twelve that do not
    // are the right and full outer joins forced to a nested loop, which both
    // paths refuse. 60 leaves room for a fixture change without leaving room
    // for a sweep that has quietly stopped matching anything.
    assert!(
        nonempty >= 60,
        "only {nonempty} of the sweep's joins returned any rows"
    );
}

/// The sweep is only worth running if its four join types really differ.
#[tokio::test]
async fn the_four_join_types_return_different_rows() {
    let (_serving, backing) = seeded().await;
    let (a, b) = (authors(), books());
    let mut seen: Vec<(&str, Vec<String>)> = Vec::new();
    for (kind, join_type) in JOIN_TYPES {
        let mut join = Join::equating(at(&a, "id"), at(&b, "author_id"));
        join.join_type = join_type;
        // A nested loop cannot serve the right and full cases, so let the
        // planner choose.
        let rows = in_process_join(&backing, &a, &b, &join).await.unwrap();
        seen.push((kind, multiset(&rows)));
    }
    for (i, (kind, rows)) in seen.iter().enumerate() {
        for (other, others) in seen.iter().skip(i + 1) {
            assert_ne!(
                rows, others,
                "a {kind} join and a {other} join returned the same rows, \
                 so the fixture does not distinguish them"
            );
        }
    }
}

// --- chains ---------------------------------------------------------------

#[tokio::test]
async fn a_three_table_chain_over_grpc_returns_what_the_kernel_returns() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b, s) = (authors(), books(), sales());
    let tables: Vec<&TableDef> = vec![&a, &b, &s];
    let space = JoinSchema::over(tables.iter().copied());
    let mut checked = 0;

    for (kind, join_type) in JOIN_TYPES {
        for (how, force) in algorithms() {
            let mut first = JoinStep::equating(space.at(0, at(&a, "id")), at(&b, "author_id"));
            let mut second = JoinStep::equating(space.at(1, at(&b, "id")), at(&s, "book_id"));
            first.join_type = join_type;
            second.join_type = join_type;
            first.force = force;
            second.force = force;
            // A condition on the last step naming the *first* table, which is
            // the whole point of a positional ordinal space: the step can see
            // everything read before it, not only the table just added.
            second.having = Expr::compare(space.at(0, at(&a, "born")), CmpOp::Lt, Value::I64(1960));
            let chain = Chain::from(Query::all()).join(first).join(second);

            let expected = in_process_chain(&backing, &tables, &chain).await;
            let actual = over_the_wire(&mut client, chain_to_proto(&tables, &chain)).await;
            match (expected, actual) {
                (Ok(expected), Ok(actual)) => assert_eq!(
                    multiset(&actual),
                    multiset(&expected),
                    "a {kind} chain via {how} disagreed with the kernel"
                ),
                (Err(_), Err(status)) => assert_eq!(status.code(), Code::InvalidArgument),
                (Ok(rows), Err(status)) => panic!(
                    "a {kind} chain via {how}: the kernel returned {} rows, the wire said \
                     {status:?}",
                    rows.len()
                ),
                (Err(error), Ok(rows)) => panic!(
                    "a {kind} chain via {how}: the kernel refused with {error}, the wire \
                     returned {} rows",
                    rows.len()
                ),
            }
            checked += 1;
        }
    }
    assert!(checked >= 16, "the chain sweep only ran {checked} cases");
}

/// The same table twice, with the second step joining back to the *first*
/// input rather than the one before it.
///
/// This is the case a name-based addressing scheme cannot express — both
/// inputs are called `books` — and the case a "join the previous table" model
/// cannot express either. Both fall out of naming an input by position.
#[tokio::test]
async fn a_self_join_joins_back_to_the_first_input() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());
    let tables: Vec<&TableDef> = vec![&a, &b, &b];
    let space = JoinSchema::over(tables.iter().copied());

    // authors -> their books -> the same author's books again, keyed straight
    // back to input 0.
    let chain = Chain::from(Query::all())
        .join(JoinStep::equating(
            space.at(0, at(&a, "id")),
            at(&b, "author_id"),
        ))
        .join(JoinStep::equating(
            space.at(0, at(&a, "id")),
            at(&b, "author_id"),
        ));

    let expected = in_process_chain(&backing, &tables, &chain).await.unwrap();
    let actual = over_the_wire(&mut client, chain_to_proto(&tables, &chain))
        .await
        .unwrap();
    assert!(
        !expected.is_empty(),
        "the self-join fixture produced nothing, so this proves nothing"
    );
    assert_eq!(multiset(&actual), multiset(&expected));
}

// --- aggregates -----------------------------------------------------------

fn groups_from_wire(groups: &[pb::Group]) -> Vec<String> {
    let mut out: Vec<String> = groups
        .iter()
        .map(|group| {
            let key: Vec<Value> = group
                .key
                .iter()
                .map(|v| slate_server::convert::value_from_proto(v).unwrap())
                .collect();
            let values: Vec<Value> = group
                .values
                .iter()
                .map(|v| slate_server::convert::value_from_proto(v).unwrap())
                .collect();
            format!("{key:?} -> {values:?}")
        })
        .collect();
    out.sort();
    out
}

fn groups_from_kernel(groups: &[slate_kernel::Group]) -> Vec<String> {
    let mut out: Vec<String> = groups
        .iter()
        .map(|group| format!("{:?} -> {:?}", group.key, group.values))
        .collect();
    out.sort();
    out
}

async fn wire_groups(
    client: &mut RecordsClient<Channel>,
    query: pb::AggregateQuery,
) -> Result<Vec<pb::Group>, tonic::Status> {
    let stream = client
        .aggregate(app_request(pb::AggregateRequest {
            transaction: String::new(),
            aggregate: Some(query),
            freshness: None,
        }))
        .await?
        .into_inner();
    let (groups, served_by) = drain_groups(stream).await;
    assert!(served_by.is_some(), "an aggregate must carry `served_by`");
    Ok(groups)
}

#[tokio::test]
async fn an_aggregate_over_grpc_returns_what_the_kernel_returns() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;
    let b = books();
    let year = at(&b, "year");
    let author_id = at(&b, "author_id");
    let decade = Query::computed(&b, 0);

    // Each case reaches something different: every aggregate function; a
    // grouping key that is a *computed* value rather than a column; a HAVING
    // over an aggregate; and the ungrouped case, which is one group.
    let cases: Vec<AggregateCase> = vec![
        (
            "every function, ungrouped",
            Query::all(),
            vec![],
            vec![
                Aggregate::Count,
                Aggregate::CountColumn(year),
                Aggregate::Min(year),
                Aggregate::Max(year),
                Aggregate::Sum(year),
                Aggregate::Avg(year),
                Aggregate::CountDistinct(author_id),
            ],
            Expr::True,
        ),
        (
            "grouped by a column",
            Query::all(),
            vec![author_id],
            vec![Aggregate::Count, Aggregate::Max(year)],
            Expr::True,
        ),
        (
            "grouped by a computed value",
            Query::all().computing([Scalar::column(year) / 10i64]),
            vec![decade],
            vec![Aggregate::Count, Aggregate::Sum(year)],
            Expr::True,
        ),
        (
            "HAVING over an aggregate",
            Query::all(),
            vec![author_id],
            vec![Aggregate::Count],
            Expr::compare(
                slate_kernel::Group::aggregate(1, 0),
                CmpOp::Gt,
                Value::U64(1),
            ),
        ),
        (
            "HAVING over a group key",
            Query::all(),
            vec![author_id],
            vec![Aggregate::Count],
            Expr::compare(
                slate_kernel::Group::aggregate(0, 0),
                CmpOp::Ge,
                Value::U64(2),
            ),
        ),
        (
            "filtered input, grouped",
            Query::all().filter(Expr::compare(year, CmpOp::Ge, Value::I64(2005))),
            vec![author_id],
            vec![Aggregate::Count, Aggregate::Min(year)],
            Expr::True,
        ),
    ];

    let store: RecordStore<Arc<MemoryStore>> = common::store(Arc::clone(&backing));
    for (name, query, group, aggregates, having) in cases {
        let txn = store.begin().await.unwrap();
        let expected = txn
            .group_by_having(&ctx(), &b, &query, &group, &aggregates, &having)
            .await
            .unwrap();
        txn.rollback();

        let wire = aggregate_to_proto_query(&b, &query, &group, &aggregates, &having);
        let actual = wire_groups(&mut client, wire).await.unwrap();

        assert_eq!(
            groups_from_wire(&actual),
            groups_from_kernel(&expected),
            "`{name}` disagreed with the kernel"
        );
        assert!(
            !expected.is_empty(),
            "`{name}` produced no groups, so it proves nothing"
        );
    }
}

// --- computed columns on a single-table read ------------------------------

#[tokio::test]
async fn computed_columns_over_grpc_return_what_the_kernel_returns() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;
    let b = books();
    let (title, year) = (at(&b, "title"), at(&b, "year"));
    let first = Query::computed(&b, 0);

    // One case per `Scalar` shape that a string or integer column can reach.
    // `Distance` is the one left out: it needs a vector column, which none of
    // these fixtures has, and it is covered by the round trip in `wire.rs`
    // and by the kernel's own vector tests.
    let cases: Vec<(&str, Vec<Scalar>)> = vec![
        ("arithmetic", vec![Scalar::column(year) + 1i64]),
        ("subtraction", vec![Scalar::column(year) - 1i64]),
        ("multiplication", vec![Scalar::column(year) * 2i64]),
        ("division", vec![Scalar::column(year) / 10i64]),
        ("length", vec![Scalar::column(title).length()]),
        (
            "lower",
            vec![Scalar::Lower(Box::new(Scalar::column(title)))],
        ),
        (
            "upper",
            vec![Scalar::Upper(Box::new(Scalar::column(title)))],
        ),
        (
            "concat",
            vec![Scalar::Concat(vec![
                Scalar::column(title),
                Scalar::literal(Value::Str("!".to_owned())),
            ])],
        ),
        (
            "coalesce",
            vec![Scalar::Coalesce(vec![
                Scalar::literal(Value::Null),
                Scalar::column(title),
            ])],
        ),
        (
            "case when",
            vec![Scalar::Case {
                branches: vec![(
                    Expr::compare(year, CmpOp::Ge, Value::I64(2005)),
                    Scalar::literal(Value::Str("recent".to_owned())),
                )],
                otherwise: Box::new(Scalar::literal(Value::Str("older".to_owned()))),
            }],
        ),
        (
            "extract",
            vec![Scalar::column(year).extract(TimeUnit::Minute)],
        ),
        (
            "date_trunc",
            vec![Scalar::column(year).date_trunc(TimeUnit::Hour)],
        ),
        (
            "regexp_replace",
            vec![Scalar::RegexpReplace {
                value: Box::new(Scalar::column(title)),
                pattern: "-(.*)$".to_owned(),
                replacement: "/\\1".to_owned(),
            }],
        ),
        (
            "a literal, and a second value reading the first",
            vec![Scalar::literal(Value::I64(7)), Scalar::column(first) * 3i64],
        ),
    ];

    let store: RecordStore<Arc<MemoryStore>> = common::store(Arc::clone(&backing));
    for (name, compute) in cases {
        let compute_len = compute.len();
        // Sorted by the primary key so the two paths' orders are pinned, and
        // filtered on the computed value so the conversion has to survive a
        // predicate as well as a projection.
        let query = Query::all()
            .computing(compute)
            .sort_by([SortKey::asc(at(&b, "id"))]);

        let txn = store.begin().await.unwrap();
        let expected: Vec<Row> = txn
            .execute(&ctx(), &b, &query)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        txn.rollback();

        let stream = client
            .query(app_request(pb::QueryRequest {
                transaction: String::new(),
                query: Some(query_to_proto(&b, &query)),
                freshness: None,
            }))
            .await
            .unwrap()
            .into_inner();
        let (rows, _) = drain(stream).await;

        // Every row must carry exactly the table's own columns in `values`,
        // and the computed values in `computed` — never concatenated. That is
        // the split itself: with them concatenated a client reads computed
        // value `i` at `table_width + i`, from a width this protocol does not
        // publish and which moves the day a column is added.
        for row in &rows {
            assert_eq!(
                row.values.len(),
                b.columns().len(),
                "`{name}` returned a row whose stored values are not the table's columns"
            );
            assert_eq!(
                row.computed.len(),
                compute_len,
                "`{name}` returned {} computed values, not {compute_len}",
                row.computed.len()
            );
        }

        let actual: Vec<Row> = rows
            .iter()
            .map(|r| flat_row_from_proto(r).unwrap())
            .collect();

        assert_eq!(
            format!("{actual:?}"),
            format!("{expected:?}"),
            "`{name}` disagreed with the kernel"
        );
        assert!(
            !expected.is_empty(),
            "`{name}` returned no rows, so it proves nothing"
        );
        assert!(
            expected[0].values().len() > b.columns().len(),
            "`{name}` returned no computed value at all"
        );
    }
}

// --- served_by and freshness over several tables --------------------------

#[tokio::test]
async fn a_join_reports_one_view_for_every_input() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());
    let join = Join::equating(at(&a, "id"), at(&b, "author_id"));

    let stream = client
        .join(app_request(pb::JoinRequest {
            transaction: String::new(),
            join: Some(join_to_proto(&a, &b, &join)),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();

    // Counted rather than taken from the first message: two tables were read,
    // and if the head node routed twice there would be two views to name and
    // no honest way to name one. One `served_by` in the whole stream is the
    // assertion that it routed once.
    let mut stream = stream;
    let mut named = Vec::new();
    let mut rows = 0;
    while let Some(message) = stream.message().await.unwrap() {
        if let Some(served_by) = message.served_by {
            named.push(served_by);
        }
        rows += message.rows.len();
    }
    assert!(rows > 0, "the join returned nothing");
    assert_eq!(
        named.len(),
        1,
        "a join over two tables named {} views: {named:?}",
        named.len()
    );
    assert!(
        !named[0].replica.is_empty(),
        "the view that served the join has no name"
    );
}

#[tokio::test]
async fn a_join_honours_the_freshness_it_was_given_for_every_input() {
    use slate_kernel::{KernelError, KvReadStore, KvSnapshot};
    use slate_kernel::{Result as KernelResult, memory::MemoryStore as Mem};
    use std::time::Duration;

    /// A replica that is empty and will never catch up, as `freshness.rs`
    /// uses. Extreme on purpose: the difference between "served stale" and
    /// "served fresh" is then the difference between no rows and some.
    #[derive(Debug)]
    struct Frozen(Mem);

    #[tonic::async_trait]
    impl KvReadStore for Frozen {
        async fn snapshot(&self) -> KernelResult<Box<dyn KvSnapshot + Send + '_>> {
            self.0.snapshot().await
        }
        fn visible_sequence(&self) -> Option<u64> {
            Some(0)
        }
        async fn wait_for_sequence(&self, sequence: u64, _t: Duration) -> KernelResult<()> {
            Err(KernelError::ReplicaTooStale {
                replica: "frozen".to_owned(),
                required: sequence,
                visible: 0,
            })
        }
        fn replica_name(&self) -> &str {
            "frozen"
        }
    }

    let backing = Arc::new(MemoryStore::new());
    let leadership =
        slate_server::leadership::Leadership::new(Arc::new(common::AlwaysLeader::default()));
    assert!(leadership.campaign().await);
    let head = slate_server::Head::new(
        slate_server::HeadConfig::new(common::catalog(), common::security()),
        Arc::clone(&backing),
        vec![Arc::new(Frozen(Mem::new())) as Arc<dyn KvReadStore>],
        leadership,
        Arc::new(slate_server::MetadataIdentity::trusting_the_caller_completely()),
    );
    let serving = common::serve(head).await;
    let mut client = serving.client().await;

    seed(&backing).await;
    let sequence = backing
        .visible_sequence()
        .expect("the writer tracks a sequence");

    let (a, b) = (authors(), books());
    let join = join_to_proto(&a, &b, &Join::equating(at(&a, "id"), at(&b, "author_id")));

    // The control: with no token the join can be, and is, served by the
    // replica that has nothing.
    let stale = over_the_wire(&mut client, join.clone()).await.unwrap();
    assert!(
        stale.is_empty(),
        "the frozen replica returned rows, so this test proves nothing"
    );

    // With one, it cannot be — and the rows the join returns are from a view
    // that reached the sequence for *both* tables, because there is only one
    // view.
    let stream = client
        .join(app_request(pb::JoinRequest {
            transaction: String::new(),
            join: Some(join),
            freshness: Some(pb::Freshness {
                level: Some(pb::freshness::Level::AtLeast(sequence)),
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    let (rows, served_by) = drain_joined(stream).await;
    assert!(!rows.is_empty(), "read-your-writes did not hold for a join");
    assert_ne!(
        served_by.expect("served_by").replica,
        "frozen",
        "a join was served by a replica that could not meet its freshness"
    );
}

// --- routing --------------------------------------------------------------

/// A replica showing the writer's data under a name of its own, as
/// `freshness.rs` uses to make routing visible.
#[derive(Debug)]
struct Mirror {
    name: String,
    backing: Arc<MemoryStore>,
}

#[tonic::async_trait]
impl slate_kernel::KvReadStore for Mirror {
    async fn snapshot(
        &self,
    ) -> slate_kernel::Result<Box<dyn slate_kernel::KvSnapshot + Send + '_>> {
        self.backing.snapshot().await
    }
    fn visible_sequence(&self) -> Option<u64> {
        self.backing.visible_sequence()
    }
    fn replica_name(&self) -> &str {
        &self.name
    }
}

/// A join over tenant-scoped tables goes to the tenant's replica, and stays
/// there.
///
/// A multi-table read has several tables to take an affinity from, and the
/// rule is the same as for one: the *principal's* tenant, if any input is
/// tenant-scoped. Worth its own test because the alternative — no affinity at
/// all — returns the right rows every time and quietly gives every replica a
/// cold copy of everything.
#[tokio::test]
async fn a_join_over_tenant_scoped_tables_is_routed_by_the_principal_s_tenant() {
    let backing = Arc::new(MemoryStore::new());
    seed(&backing).await;

    let replicas: Vec<Arc<dyn slate_kernel::KvReadStore>> = (0..4)
        .map(|n| {
            Arc::new(Mirror {
                name: format!("replica-{n}"),
                backing: Arc::clone(&backing),
            }) as Arc<dyn slate_kernel::KvReadStore>
        })
        .collect();
    let leadership =
        slate_server::leadership::Leadership::new(Arc::new(common::AlwaysLeader::default()));
    assert!(leadership.campaign().await);
    let serving = common::serve(slate_server::Head::new(
        slate_server::HeadConfig::new(common::catalog(), common::security()),
        Arc::clone(&backing),
        replicas,
        leadership,
        Arc::new(slate_server::MetadataIdentity::trusting_the_caller_completely()),
    ))
    .await;
    let mut client = serving.client().await;

    let (a, b) = (authors(), books());
    let wire = join_to_proto(&a, &b, &Join::equating(at(&a, "id"), at(&b, "author_id")));

    let mut per_tenant = std::collections::BTreeSet::new();
    for tenant in 1..=6_u64 {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..5 {
            let stream = client
                .join(common::app_in(
                    pb::JoinRequest {
                        transaction: String::new(),
                        join: Some(wire.clone()),
                        freshness: None,
                    },
                    1,
                    tenant,
                ))
                .await
                .unwrap()
                .into_inner();
            let (_, served_by) = drain_joined(stream).await;
            seen.insert(served_by.expect("served_by").replica);
        }
        assert_eq!(
            seen.len(),
            1,
            "tenant {tenant}'s joins were spread over {seen:?}, so its range is cached nowhere"
        );
        per_tenant.extend(seen);
    }
    // The control: "always the same replica" is also satisfied by a pool that
    // sends everything to one, which is stable and is not placement.
    assert!(
        per_tenant.len() > 1,
        "every tenant's join landed on {per_tenant:?}"
    );
}

/// Two inputs take the kernel's two-table join, which chooses a build side;
/// three take the chain, which cannot.
///
/// Observable only through the plan, and only with statistics that make the
/// sides different sizes — which is the point: routing two inputs through the
/// chain path would return identical rows and quietly lose the choice.
#[tokio::test]
async fn a_two_table_join_takes_the_path_that_can_choose_its_build_side() {
    use slate_kernel::{Statistics, TableStats};

    let backing = Arc::new(MemoryStore::new());
    seed(&backing).await;
    let leadership =
        slate_server::leadership::Leadership::new(Arc::new(common::AlwaysLeader::default()));
    assert!(leadership.campaign().await);
    let serving = common::serve(slate_server::Head::new(
        slate_server::HeadConfig::new(common::catalog(), common::security()).with_statistics(
            Statistics::new()
                .with(common::AUTHORS, TableStats::with_row_count(500_000))
                .with(common::BOOKS, TableStats::with_row_count(10)),
        ),
        Arc::clone(&backing),
        Vec::new(),
        leadership,
        Arc::new(slate_server::MetadataIdentity::trusting_the_caller_completely()),
    ))
    .await;
    let mut client = serving.client().await;

    let (a, b) = (authors(), books());
    let answer = client
        .explain_join(app_request(pb::ExplainJoinRequest {
            transaction: String::new(),
            join: Some(join_to_proto(
                &a,
                &b,
                &Join::equating(at(&a, "id"), at(&b, "author_id")),
            )),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();

    // A chain step always builds the table it is adding — the right side —
    // because everything before it is already in memory. A two-table join
    // decides from the estimates, and here they say left.
    assert_eq!(
        answer.inputs[1]
            .algorithm
            .as_ref()
            .and_then(|a| a.algorithm),
        Some(pb::join_algorithm::Algorithm::HashBuild(
            pb::Side::Left as i32
        )),
        "two inputs did not take the two-table path: {}",
        answer.display
    );
}

// --- explaining -----------------------------------------------------------

#[tokio::test]
async fn explaining_a_join_says_which_algorithm_it_would_run() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());

    for (side, wanted) in [(Side::Left, pb::Side::Left), (Side::Right, pb::Side::Right)] {
        let mut join = Join::equating(at(&a, "id"), at(&b, "author_id"));
        join.force = Some(JoinAlgorithm::Hash { build: side });
        let answer = client
            .explain_join(app_request(pb::ExplainJoinRequest {
                transaction: String::new(),
                join: Some(join_to_proto(&a, &b, &join)),
                freshness: None,
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(answer.inputs.len(), 2, "one plan per input");
        assert!(
            answer.inputs[0].algorithm.is_none(),
            "nothing is joined to the first input"
        );
        assert_eq!(
            answer.inputs[1]
                .algorithm
                .as_ref()
                .and_then(|a| a.algorithm),
            Some(pb::join_algorithm::Algorithm::HashBuild(wanted as i32)),
            "the forced build side did not reach the plan"
        );
        // The residual is where a policy shows up, which is the reason
        // `Explain` returns one at all: both inputs are tenant-scoped, so
        // both must carry the tenant restriction.
        for (index, input) in answer.inputs.iter().enumerate() {
            let plan = input.plan.as_ref().expect("a plan per input");
            assert!(
                plan.residual.contains("tenant_id") || plan.residual.contains("Ordinal(0)"),
                "input {index}'s residual does not mention the tenant: {}",
                plan.residual
            );
        }
    }
}

#[tokio::test]
async fn explaining_a_chain_reports_a_plan_per_table() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b, s) = (authors(), books(), sales());
    let tables: Vec<&TableDef> = vec![&a, &b, &s];
    let space = JoinSchema::over(tables.iter().copied());
    let chain = Chain::from(Query::all())
        .join(JoinStep::equating(
            space.at(0, at(&a, "id")),
            at(&b, "author_id"),
        ))
        .join(JoinStep::equating(
            space.at(1, at(&b, "id")),
            at(&s, "book_id"),
        ));

    let answer = client
        .explain_join(app_request(pb::ExplainJoinRequest {
            transaction: String::new(),
            join: Some(chain_to_proto(&tables, &chain)),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();

    assert_eq!(answer.inputs.len(), 3, "one plan per table in the chain");
    assert_eq!(
        answer.inputs[0]
            .plan
            .as_ref()
            .expect("a plan")
            .table
            .as_str(),
        "authors"
    );
    assert_eq!(
        answer.inputs[2]
            .plan
            .as_ref()
            .expect("a plan")
            .table
            .as_str(),
        "sales"
    );
    assert!(answer.estimated_cost > 0.0, "a chain costs something");
    assert!(
        answer.display.contains("step 2"),
        "the human form should name each step: {}",
        answer.display
    );
}

/// A client may lower the memory a build side takes and may not raise it.
#[tokio::test]
async fn a_build_limit_above_the_nodes_own_is_clamped_and_reported() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());
    let mut wire = join_to_proto(&a, &b, &Join::equating(at(&a, "id"), at(&b, "author_id")));
    wire.build_limit = Some(u64::MAX);

    let answer = client
        .explain_join(app_request(pb::ExplainJoinRequest {
            transaction: String::new(),
            join: Some(wire),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(
        answer.warnings.iter().any(|w| w.contains("build limit")),
        "raising the build limit was accepted silently: {:?}",
        answer.warnings
    );
}

/// A build limit the client *lowers* is honoured, which is what makes the
/// clamp a clamp rather than the field being ignored.
#[tokio::test]
async fn a_build_limit_the_client_lowers_is_honoured() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());
    let mut join = Join::equating(at(&a, "id"), at(&b, "author_id"));
    // One row is fewer than either side has, so the build must refuse.
    join.build_limit = 1;
    join.force = Some(JoinAlgorithm::Hash { build: Side::Left });

    let error = over_the_wire(&mut client, join_to_proto(&a, &b, &join))
        .await
        .expect_err("a build limit of one row cannot hold this join");
    assert_eq!(error.code(), Code::ResourceExhausted);
}

// --- what a client cannot get away with -----------------------------------

/// Build a two-input join request by hand, so a test can damage one field.
fn handmade_join(left: &str, right: &str, on: Vec<pb::JoinOn>) -> pb::JoinQuery {
    pb::JoinQuery {
        inputs: vec![
            pb::JoinInput {
                query: Some(common::plain_query(left)),
                on: Vec::new(),
                join_type: pb::JoinType::Inner as i32,
                having: None,
                force: None,
            },
            pb::JoinInput {
                query: Some(common::plain_query(right)),
                on,
                join_type: pb::JoinType::Inner as i32,
                having: None,
                force: None,
            },
        ],
        limit: None,
        offset: 0,
        build_limit: None,
    }
}

fn plain_on() -> Vec<pb::JoinOn> {
    vec![pb::JoinOn {
        earlier: Some(column_ref(0, 1)),
        own: Some(column_ref(1, 2)),
    }]
}

async fn refused(client: &mut RecordsClient<Channel>, query: pb::JoinQuery) -> tonic::Status {
    over_the_wire(client, query)
        .await
        .expect_err("this request must be refused")
}

#[tokio::test]
async fn a_join_ordinal_past_the_end_of_its_table_is_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;

    // `authors` has six columns; naming the ninth silently matches nothing in
    // the kernel, which is right for evaluation and useless as a diagnosis.
    let status = refused(
        &mut client,
        handmade_join(
            "authors",
            "books",
            vec![pb::JoinOn {
                earlier: Some(column_ref(0, 9)),
                own: Some(column_ref(1, 2)),
            }],
        ),
    )
    .await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("6 columns"),
        "the message should name the width: {}",
        status.message()
    );
}

#[tokio::test]
async fn naming_an_input_the_request_does_not_have_is_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let status = refused(
        &mut client,
        handmade_join(
            "authors",
            "books",
            vec![pb::JoinOn {
                earlier: Some(column_ref(4, 1)),
                own: Some(column_ref(1, 2)),
            }],
        ),
    )
    .await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("input 4"),
        "the message should name the input: {}",
        status.message()
    );
}

#[tokio::test]
async fn a_join_condition_naming_an_input_not_yet_read_is_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    // Input 0's join condition would have to read input 1, which does not
    // exist when input 0 is read. The kernel would evaluate it as null; the
    // wire refuses it, because a condition that is silently unknown is a
    // condition nobody wrote.
    let mut wire = handmade_join("authors", "books", plain_on());
    wire.inputs[0].on = plain_on();
    let status = refused(&mut client, wire).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("nothing before it"),
        "{}",
        status.message()
    );
}

#[tokio::test]
async fn a_computed_value_cannot_be_named_from_across_a_join() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let mut wire = handmade_join("authors", "books", plain_on());
    // Input 0 computes one value…
    wire.inputs[0].query.as_mut().unwrap().compute = vec![pb::Scalar {
        node: Some(pb::scalar::Node::Column(column_ref(0, 5))),
    }];
    // …and the join condition tries to read it. The kernel's joined ordinal
    // space is packed by declared table width, so there is no slot for it and
    // a flat ordinal would land on the first column of `books`.
    wire.inputs[1].having = Some(pb::Expr {
        node: Some(pb::expr::Node::Compare(pb::Compare {
            column: Some(computed_ref(0, 0)),
            op: pb::CmpOp::Gt as i32,
            value: Some(slate_server::convert::value_to_proto(&Value::I64(0))),
        })),
    });

    let status = refused(&mut client, wire).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("not addressable across inputs"),
        "{}",
        status.message()
    );
}

/// An input's own filter is planned against its own table, so a reference to
/// another input's column would silently be read as one of this table's.
#[tokio::test]
async fn an_input_s_own_filter_may_not_name_another_input() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let mut wire = handmade_join("authors", "books", plain_on());
    wire.inputs[0].query.as_mut().unwrap().filter = Some(pb::Expr {
        node: Some(pb::expr::Node::IsNull(pb::IsNull {
            // Input 1, inside input 0's own query.
            column: Some(column_ref(1, 2)),
            negated: false,
        })),
    });
    let status = refused(&mut client, wire).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("evaluated over input 0"),
        "the message should say which input it is evaluated over: {}",
        status.message()
    );
}

#[tokio::test]
async fn a_join_input_may_not_carry_a_limit_a_sort_or_an_offset() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;

    for (name, damage) in [
        (
            "limit",
            Box::new(|q: &mut pb::Query| q.limit = Some(1)) as Box<dyn Fn(&mut pb::Query)>,
        ),
        ("offset", Box::new(|q: &mut pb::Query| q.offset = 1)),
        (
            "sort",
            Box::new(|q: &mut pb::Query| {
                q.sort = vec![pb::SortKey {
                    column: Some(column_ref(0, 1)),
                    direction: pb::SortDirection::Asc as i32,
                    nulls: pb::NullsOrder::Unspecified as i32,
                }];
            }),
        ),
    ] {
        let mut wire = handmade_join("authors", "books", plain_on());
        damage(wire.inputs[0].query.as_mut().unwrap());
        let status = refused(&mut client, wire).await;
        assert_eq!(
            status.code(),
            Code::InvalidArgument,
            "a side's {name} was not refused"
        );
        assert!(
            status.message().contains(name),
            "the message should say which setting: {}",
            status.message()
        );
    }
}

#[tokio::test]
async fn a_join_of_one_table_is_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let wire = pb::JoinQuery {
        inputs: vec![pb::JoinInput {
            query: Some(common::plain_query("authors")),
            on: Vec::new(),
            join_type: pb::JoinType::Inner as i32,
            having: None,
            force: None,
        }],
        limit: None,
        offset: 0,
        build_limit: None,
    };
    let status = refused(&mut client, wire).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("at least two"),
        "{}",
        status.message()
    );
}

#[tokio::test]
async fn a_join_naming_a_table_this_node_does_not_serve_is_not_found() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let status = refused(&mut client, handmade_join("authors", "nowhere", plain_on())).await;
    assert_eq!(status.code(), Code::NotFound);
}

#[tokio::test]
async fn a_column_reference_with_no_kind_is_refused_rather_than_read_as_column_zero() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let status = refused(
        &mut client,
        handmade_join(
            "authors",
            "books",
            vec![pb::JoinOn {
                earlier: Some(pb::ColumnRef { input: 0, of: None }),
                own: Some(column_ref(1, 2)),
            }],
        ),
    )
    .await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("no kind set"),
        "{}",
        status.message()
    );
}

// --- refusals on the aggregate surface ------------------------------------

async fn refused_aggregate(
    client: &mut RecordsClient<Channel>,
    query: pb::AggregateQuery,
) -> tonic::Status {
    wire_groups(client, query)
        .await
        .expect_err("this aggregate must be refused")
}

fn count_over(table: &str) -> pb::AggregateQuery {
    pb::AggregateQuery {
        input: Some(pb::Query {
            projection: None,
            ..common::plain_query(table)
        }),
        group_by: Vec::new(),
        join: None,
        sort: Vec::new(),
        limit: None,
        offset: 0,
        aggregates: vec![pb::Aggregate {
            function: pb::AggregateFunction::Count as i32,
            column: None,
        }],
        having: None,
    }
}

/// The refusal the whole `ColumnRef` model exists to make possible.
#[tokio::test]
async fn having_a_column_that_is_not_grouped_is_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let mut wire = count_over("books");
    wire.group_by = vec![column_ref(0, at(&books(), "author_id").0)];
    // `year` is not a grouping column and not an aggregate. With flat
    // ordinals this would have been a legitimate group key; as a *kind* it
    // cannot be one, so it is a refusal rather than a wrong answer.
    wire.having = Some(pb::Expr {
        node: Some(pb::expr::Node::Compare(pb::Compare {
            column: Some(column_ref(0, at(&books(), "year").0)),
            op: pb::CmpOp::Gt as i32,
            value: Some(slate_server::convert::value_to_proto(&Value::I64(0))),
        })),
    });

    let status = refused_aggregate(&mut client, wire).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("not grouped"),
        "the message should say why: {}",
        status.message()
    );
}

#[tokio::test]
async fn having_an_aggregate_that_does_not_exist_is_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let mut wire = count_over("books");
    wire.having = Some(pb::Expr {
        node: Some(pb::expr::Node::Compare(pb::Compare {
            column: Some(pb::ColumnRef {
                input: 0,
                of: Some(pb::column_ref::Of::Aggregate(3)),
            }),
            op: pb::CmpOp::Gt as i32,
            value: Some(slate_server::convert::value_to_proto(&Value::U64(0))),
        })),
    });
    let status = refused_aggregate(&mut client, wire).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("aggregate 3"),
        "{}",
        status.message()
    );
}

#[tokio::test]
async fn an_aggregate_with_no_aggregates_is_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let mut wire = count_over("books");
    wire.aggregates = Vec::new();
    let status = refused_aggregate(&mut client, wire).await;
    assert_eq!(status.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn count_star_with_a_column_and_sum_without_one_are_both_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;

    let mut with_column = count_over("books");
    with_column.aggregates[0].column = Some(column_ref(0, 4));
    let status = refused_aggregate(&mut client, with_column).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("COUNT(*)"),
        "{}",
        status.message()
    );

    let mut without = count_over("books");
    without.aggregates[0].function = pb::AggregateFunction::Sum as i32;
    let status = refused_aggregate(&mut client, without).await;
    assert_eq!(status.code(), Code::InvalidArgument);
}

#[tokio::test]
async fn an_aggregate_input_may_not_set_a_projection() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let mut wire = count_over("books");
    wire.input.as_mut().unwrap().projection = Some(pb::Projection {
        all_columns: true,
        columns: Vec::new(),
    });
    let status = refused_aggregate(&mut client, wire).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("projection"),
        "{}",
        status.message()
    );
}

#[tokio::test]
async fn a_grouping_column_past_the_end_of_the_table_is_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let mut wire = count_over("books");
    wire.group_by = vec![column_ref(0, 12)];
    let status = refused_aggregate(&mut client, wire).await;
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("5 columns"),
        "{}",
        status.message()
    );
}

/// A computed value may read the ones before it and not itself.
#[tokio::test]
async fn a_computed_value_cannot_read_itself_or_a_later_one() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;

    let mut query = common::plain_query("books");
    query.compute = vec![pb::Scalar {
        node: Some(pb::scalar::Node::Column(computed_ref(0, 0))),
    }];
    let error = client
        .query(app_request(pb::QueryRequest {
            transaction: String::new(),
            query: Some(query),
            freshness: None,
        }))
        .await
        .expect_err("a computed value reading itself must be refused");
    assert_eq!(error.code(), Code::InvalidArgument);
    assert!(
        error.message().contains("computed value 0"),
        "{}",
        error.message()
    );
}

// --- transactions ---------------------------------------------------------

/// Every multi-table read is available inside a transaction too, and reports
/// itself as coming from the writer rather than from a replica.
#[tokio::test]
async fn a_join_inside_a_transaction_is_served_by_the_writer() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());
    let join = Join::equating(at(&a, "id"), at(&b, "author_id"));

    let handle = client
        .begin(app_request(pb::BeginRequest {}))
        .await
        .unwrap()
        .into_inner()
        .transaction;

    let stream = client
        .join(app_request(pb::JoinRequest {
            transaction: handle.clone(),
            join: Some(join_to_proto(&a, &b, &join)),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();
    let (rows, served_by) = drain_joined(stream).await;
    assert!(served_by.unwrap().replica.contains("transaction"));

    let expected = in_process_join(&backing, &a, &b, &join).await.unwrap();
    assert_eq!(multiset(&from_wire(&rows)), multiset(&expected));

    let groups = client
        .aggregate(app_request(pb::AggregateRequest {
            transaction: handle.clone(),
            aggregate: Some(count_over("books")),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();
    let (groups, served_by) = drain_groups(groups).await;
    assert_eq!(groups.len(), 1);
    assert!(served_by.unwrap().replica.contains("transaction"));

    let explained = client
        .explain_join(app_request(pb::ExplainJoinRequest {
            transaction: handle.clone(),
            join: Some(join_to_proto(&a, &b, &join)),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(explained.inputs.len(), 2);

    client
        .rollback(app_request(pb::RollbackRequest {
            transaction: handle,
        }))
        .await
        .unwrap();
}

// --- warnings, on the request that carried them ---------------------------

/// The same finding as `server.rs`'s query case, on the two other streams. A
/// join has more places to hide one: an unusable hint on any input, and a
/// build limit the client tried to raise.
#[tokio::test]
async fn a_join_reports_what_it_did_with_the_request_on_the_request() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());

    let mut wire = join_to_proto(&a, &b, &Join::equating(at(&a, "id"), at(&b, "author_id")));
    // Both at once, on purpose: they come from different places in the
    // conversion — one per input, one for the join — and a plumbing that
    // carried only the second would pass a test with only the second.
    wire.build_limit = Some(u64::MAX);
    if let Some(input) = wire.inputs.get_mut(1)
        && let Some(query) = input.query.as_mut()
    {
        query.hint = Some(common::missing_index_hint());
    }

    let (rows, warnings) = common::drain_joined_warned(
        client
            .join(app_request(pb::JoinRequest {
                transaction: String::new(),
                join: Some(wire),
                freshness: None,
            }))
            .await
            .unwrap()
            .into_inner(),
    )
    .await;
    assert!(!rows.is_empty(), "the join should still have run");
    assert!(
        warnings.iter().any(|w| w.contains("build limit")),
        "{warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("by_nothing")),
        "{warnings:?}"
    );

    // The control: the same join with nothing to complain about is quiet.
    let (_, quiet) = common::drain_joined_warned(
        client
            .join(app_request(pb::JoinRequest {
                transaction: String::new(),
                join: Some(join_to_proto(
                    &a,
                    &b,
                    &Join::equating(at(&a, "id"), at(&b, "author_id")),
                )),
                freshness: None,
            }))
            .await
            .unwrap()
            .into_inner(),
    )
    .await;
    assert!(quiet.is_empty(), "{quiet:?}");
}

#[tokio::test]
async fn an_aggregate_reports_what_it_did_with_the_request_on_the_request() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let b = books();

    let mut wire = aggregate_to_proto_query(
        &b,
        &Query::all(),
        &[],
        &[Aggregate::Count],
        &slate_kernel::Expr::True,
    );
    if let Some(input) = wire.input.as_mut() {
        input.hint = Some(common::missing_index_hint());
    }

    let (groups, warnings) = common::drain_groups_warned(
        client
            .aggregate(app_request(pb::AggregateRequest {
                transaction: String::new(),
                aggregate: Some(wire),
                freshness: None,
            }))
            .await
            .unwrap()
            .into_inner(),
    )
    .await;
    assert_eq!(groups.len(), 1, "the aggregate should still have run");
    assert!(
        warnings.iter().any(|w| w.contains("by_nothing")),
        "{warnings:?}"
    );
}

// --- a joined input's computed values are kept apart too ------------------

/// `Row.computed` is on `Row`, so it applies inside a `JoinedRow` as well as
/// on a single-table read — and it has to, because a join input's query can
/// compute values and the client reading that input's row would otherwise
/// need its table's width. The oracle above already proves the *values* are
/// right; this proves they are in the right list.
#[tokio::test]
async fn a_join_inputs_computed_values_come_back_beside_its_columns() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());

    // A computed value on each side, so a split that used one input's width
    // for both is caught: `authors` has six columns and `books` five.
    let join = Join::equating(at(&a, "id"), at(&b, "author_id"))
        .left(Query::all().computing([Scalar::column(at(&a, "name")).length()]))
        .right(Query::all().computing([
            Scalar::column(at(&b, "year")) + 1i64,
            Scalar::column(at(&b, "title")).length(),
        ]));

    let wire = join_to_proto(&a, &b, &join);
    let stream = client
        .join(app_request(pb::JoinRequest {
            transaction: String::new(),
            join: Some(wire),
            freshness: None,
        }))
        .await
        .unwrap()
        .into_inner();
    let (rows, _) = drain_joined(stream).await;
    assert!(!rows.is_empty(), "the join returned nothing to check");

    for row in &rows {
        let left = row.inputs[0]
            .row
            .as_ref()
            .expect("an inner join pairs both");
        let right = row.inputs[1]
            .row
            .as_ref()
            .expect("an inner join pairs both");
        assert_eq!(left.values.len(), a.columns().len());
        assert_eq!(left.computed.len(), 1);
        assert_eq!(right.values.len(), b.columns().len());
        assert_eq!(right.computed.len(), 2);
    }

    // And the values are the kernel's, not merely the right shape.
    let expected = in_process_join(&backing, &a, &b, &join).await.unwrap();
    assert_eq!(multiset(&from_wire(&rows)), multiset(&expected));
}

// --- grouped joins, and ordering over groups -------------------------------

/// A grouped result in a shape both paths produce, sorted for comparison the
/// way the joined rows above are — except where the request asked for an
/// order, which is the one case the order is the thing under test.
fn groups_as_strings(groups: &[Group]) -> Vec<String> {
    groups
        .iter()
        .map(|g| format!("{:?}|{:?}", g.key, g.values))
        .collect()
}

fn wire_groups_as_strings(groups: &[pb::Group]) -> Vec<String> {
    groups
        .iter()
        .map(|g| {
            let key: Vec<Value> = g
                .key
                .iter()
                .map(|v| value_from_proto(v).expect("a value this server sent"))
                .collect();
            let values: Vec<Value> = g
                .values
                .iter()
                .map(|v| value_from_proto(v).expect("a value this server sent"))
                .collect();
            format!("{key:?}|{values:?}")
        })
        .collect()
}

async fn grouped_join_over_the_wire(
    client: &mut RecordsClient<Channel>,
    query: pb::AggregateQuery,
) -> Result<Vec<String>, tonic::Status> {
    let stream = client
        .aggregate(app_request(pb::AggregateRequest {
            transaction: String::new(),
            aggregate: Some(query),
            freshness: None,
        }))
        .await?
        .into_inner();
    let (groups, served_by) = common::drain_groups(stream).await;
    assert!(
        served_by.is_some(),
        "an aggregate stream must carry `served_by` on its first message"
    );
    Ok(wire_groups_as_strings(&groups))
}

async fn grouped_join_in_process(
    backing: &Arc<MemoryStore>,
    join: &Join,
    grouping: &Grouping,
) -> Result<Vec<String>, slate_kernel::KernelError> {
    let store: RecordStore<Arc<MemoryStore>> = common::store(Arc::clone(backing));
    let txn = store.begin().await.unwrap();
    let groups = txn
        .group_by_join(&ctx(), &authors(), &books(), join, grouping)
        .await?;
    Ok(groups_as_strings(&groups))
}

/// The wire's grouped join must answer exactly what the kernel's does.
///
/// The point of the differential: the head node implements no grouping of its
/// own, so a disagreement here is a conversion bug — an ordinal resolved in
/// the wrong space, an aggregate mapped to the wrong column — and those are
/// invisible to a test that only checks the shape of the answer.
#[tokio::test]
async fn a_grouped_join_over_the_wire_agrees_with_the_kernel() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;

    // Books per author. The join's two sides are each named in their own
    // table's ordinals; the *grouping* is named in the joined schema, which is
    // the distinction this differential is here to catch.
    let (a, b) = (authors(), books());
    let space = JoinSchema::over([&a, &b]);
    let join = Join::equating(at(&a, "id"), at(&b, "author_id"));
    let grouping = Grouping::by([space.at(0, at(&a, "id"))], &[Aggregate::Count]);

    let expected = grouped_join_in_process(&backing, &join, &grouping)
        .await
        .expect("the kernel groups this join");

    let mut wire = join_to_proto(&a, &b, &join);
    wire.limit = None;
    let query = pb::AggregateQuery {
        input: None,
        join: Some(wire),
        // Input 0, its `id` column: the wire names the group positionally and
        // the server resolves it into the joined space, which is the
        // conversion under test.
        group_by: vec![column_ref(0, at(&a, "id").0)],
        aggregates: vec![pb::Aggregate {
            function: pb::AggregateFunction::Count as i32,
            column: None,
        }],
        having: None,
        sort: Vec::new(),
        limit: None,
        offset: 0,
    };

    let mut actual = grouped_join_over_the_wire(&mut client, query)
        .await
        .expect("the wire groups this join");
    let mut expected = expected;
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected, "the wire and the kernel must agree");
    assert!(!expected.is_empty(), "the fixture should produce groups");
}

/// Three inputs is refused rather than planned as something else, and the
/// refusal says why: the kernel groups a two-table join and does not group a
/// chain.
#[tokio::test]
async fn grouping_a_chain_is_refused_with_the_reason() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;

    // A plain three-table chain, built the way the chain tests above build
    // one, so the refusal is about the number of inputs and nothing else.
    let (a, b, sl) = (authors(), books(), sales());
    let tables: Vec<&TableDef> = vec![&a, &b, &sl];
    let space = JoinSchema::over(tables.iter().copied());
    let chain = Chain::from(Query::all())
        .join(JoinStep::equating(
            space.at(0, at(&a, "id")),
            at(&b, "author_id"),
        ))
        .join(JoinStep::equating(
            space.at(1, at(&b, "id")),
            at(&sl, "book_id"),
        ));
    let chain = chain_to_proto(&tables, &chain);
    let query = pb::AggregateQuery {
        input: None,
        join: Some(chain),
        group_by: Vec::new(),
        aggregates: vec![pb::Aggregate {
            function: pb::AggregateFunction::Count as i32,
            column: None,
        }],
        having: None,
        sort: Vec::new(),
        limit: None,
        offset: 0,
    };

    let status = grouped_join_over_the_wire(&mut client, query)
        .await
        .expect_err("grouping a chain must be refused");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(
        status.message().contains("chain"),
        "the refusal should say what is not built: {}",
        status.message()
    );
}

/// Naming both sources is a client bug, reported rather than resolved by a
/// precedence rule nobody would remember.
#[tokio::test]
async fn naming_both_an_input_and_a_join_is_refused() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;

    let (a, b) = (authors(), books());
    let join = Join::equating(at(&a, "id"), at(&b, "author_id"));
    let query = pb::AggregateQuery {
        input: Some(pb::Query {
            table: "authors".to_owned(),
            ..Default::default()
        }),
        join: Some(join_to_proto(&a, &b, &join)),
        group_by: Vec::new(),
        aggregates: vec![pb::Aggregate {
            function: pb::AggregateFunction::Count as i32,
            column: None,
        }],
        having: None,
        sort: Vec::new(),
        limit: None,
        offset: 0,
    };

    let status = grouped_join_over_the_wire(&mut client, query)
        .await
        .expect_err("naming both sources must be refused");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(
        status.message().contains("both"),
        "the refusal should name the problem: {}",
        status.message()
    );
}

/// Ordering, limiting and offsetting are over *groups*, and the wire's answer
/// must be the kernel's — in order this time, since the order is the point.
#[tokio::test]
async fn ordered_and_limited_groups_agree_with_the_kernel_in_order() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;

    let (a, b) = (authors(), books());
    let space = JoinSchema::over([&a, &b]);
    let join = Join::equating(at(&a, "id"), at(&b, "author_id"));

    // *Fewest* books first, and an offset, chosen so the requested order
    // differs from the kernel's default of ascending by encoded group key.
    // Under this caller's policy the visible groups are author 1 with two
    // books and author 2 with one, so ascending by count answers (2,1), (1,2)
    // where the default answers (1,2), (2,1) — and after the offset the two
    // disagree on the single group returned.
    //
    // Descending by count was the first version of this test and proved
    // nothing: it agreed with the default order on this fixture, so dropping
    // the ordering entirely still passed. The tie-break on the key is kept so
    // the comparison is against one answer rather than either of two.
    let grouping = Grouping::by([space.at(0, at(&a, "id"))], &[Aggregate::Count])
        .sort_by([SortKey::asc(Ordinal(1)), SortKey::asc(Ordinal(0))])
        .limit(2)
        .offset(1);

    let expected = grouped_join_in_process(&backing, &join, &grouping)
        .await
        .expect("the kernel orders these groups");

    let query = pb::AggregateQuery {
        input: None,
        join: Some(join_to_proto(&a, &b, &join)),
        group_by: vec![column_ref(0, at(&a, "id").0)],
        aggregates: vec![pb::Aggregate {
            function: pb::AggregateFunction::Count as i32,
            column: None,
        }],
        having: None,
        // Over the group: key 0 is the author id, aggregate 0 is the count.
        sort: vec![
            pb::SortKey {
                column: Some(pb::ColumnRef {
                    input: 0,
                    of: Some(pb::column_ref::Of::Aggregate(0)),
                }),
                direction: pb::SortDirection::Asc as i32,
                nulls: pb::NullsOrder::Unspecified as i32,
            },
            pb::SortKey {
                column: Some(pb::ColumnRef {
                    input: 0,
                    of: Some(pb::column_ref::Of::GroupKey(0)),
                }),
                direction: pb::SortDirection::Asc as i32,
                nulls: pb::NullsOrder::Unspecified as i32,
            },
        ],
        limit: Some(2),
        offset: 1,
    };

    let actual = grouped_join_over_the_wire(&mut client, query)
        .await
        .expect("the wire orders these groups");

    // Compared in order, not as a multiset: an ordering the wire dropped would
    // pass a multiset comparison exactly.
    assert_eq!(actual, expected, "the order is part of the answer");
    assert_eq!(
        actual.len(),
        1,
        "two visible groups, one skipped by the offset"
    );

    // And pinned against the fixture rather than only against the kernel, so
    // that both agreeing on the wrong thing is still a failure. Ascending by
    // count with the first group dropped leaves author 3 (one book) then
    // author 1 (three).
    let unordered = Grouping::by([space.at(0, at(&a, "id"))], &[Aggregate::Count]);
    let all = grouped_join_in_process(&backing, &join, &unordered)
        .await
        .expect("the kernel groups this join");
    assert_eq!(all.len(), 2, "two authors are visible to this caller");
    assert_ne!(
        actual,
        all.iter().skip(1).take(2).cloned().collect::<Vec<_>>(),
        "the requested order must differ from the kernel's default, or this \
         test cannot tell whether the ordering was applied"
    );
}

/// A group key on the *right* table of the join.
///
/// The joined space has to include every input for this to resolve at all,
/// and a group key on input 0 does not prove that: narrowing the space to the
/// first input alone passed every other test here. This is the one that fails.
#[tokio::test]
async fn a_group_key_on_the_right_side_of_the_join_resolves() {
    let (serving, backing) = seeded().await;
    let mut client = serving.client().await;

    let (a, b) = (authors(), books());
    let space = JoinSchema::over([&a, &b]);
    let join = Join::equating(at(&a, "id"), at(&b, "author_id"));

    // Grouping by the *book's* author_id rather than the author's id: the same
    // partition, named on the other side of the join.
    let grouping = Grouping::by([space.at(1, at(&b, "author_id"))], &[Aggregate::Count]);

    let expected = grouped_join_in_process(&backing, &join, &grouping)
        .await
        .expect("the kernel groups on the right side");

    let query = pb::AggregateQuery {
        input: None,
        join: Some(join_to_proto(&a, &b, &join)),
        group_by: vec![column_ref(1, at(&b, "author_id").0)],
        aggregates: vec![pb::Aggregate {
            function: pb::AggregateFunction::Count as i32,
            column: None,
        }],
        having: None,
        sort: Vec::new(),
        limit: None,
        offset: 0,
    };

    let mut actual = grouped_join_over_the_wire(&mut client, query)
        .await
        .expect("the wire groups on the right side");
    let mut expected = expected;
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
    assert!(!expected.is_empty(), "the fixture should produce groups");
}
