//! Row-level security on a multi-table read, once per path, over gRPC.
//!
//! The kernel's claim is that a join is *n* secured reads rather than one
//! privileged one, and `crates/slate-kernel/tests/rls_matrix.rs` holds every
//! single-table path to that standard. This file asks the same question of the
//! paths the wire added, because "the kernel enforces it" is a claim about the
//! kernel and the wire is a new way for a row to leave the store.
//!
//! It is a matrix rather than a set of scenarios, for the reason that file
//! gives: a policy honoured by a hash join and skipped by a nested loop is not
//! a partial success, and the path added last is the one tested least. Every
//! join type against every algorithm is one cell, plus the chain, the
//! aggregate and the explanation.
//!
//! # What the fixture hides, and why each row is there
//!
//! Three policies of three different shapes, so a policy applied to the wrong
//! input removes a *different* set of rows rather than the same one — which a
//! fixture where every side hid rows by owner could not detect:
//!
//! - `authors` hides what the caller does not own: `cy` belongs to somebody
//!   else.
//! - `books` hides anything published before 2000: `a-two` is 1990.
//! - Both are tenant-scoped, and tenant 2 holds a row owned by the *same*
//!   principal id — the case a check that looked only at the principal, and
//!   not at the tenant prefix, would let through.
//!
//! And a row on each side that matches nothing, so the outer joins have
//! something to preserve and the assertion is about which rows come back
//! rather than merely how many.
//!
//! # The control
//!
//! A security test that has never failed might assert nothing. Nothing on the
//! wire can produce a superuser — that is the point of `auth.rs` — so the
//! control runs the same join in process as one, and requires the forbidden
//! rows to *appear*. Without it every assertion below could pass because the
//! rows were never in the store.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod common;

use common::{
    app_in, at, author, authors, book, books, drain_groups, drain_joined, sale, sales,
    serving_leader,
};
use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Chain, Join, JoinAlgorithm, JoinSchema, JoinStep, JoinType, Query, SecurityContext, Side,
};
use slate_schema::{Row, TableDef};
use slate_server::convert::{chain_to_proto, join_to_proto, row_from_proto};
use slate_server::proto as pb;
use slate_server::proto::records_client::RecordsClient;
use slate_tuple::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
use tonic::transport::Channel;

/// Names that must never come back, whatever the path.
///
/// One per reason a row can be hidden: another principal's row, a row the
/// second table's own policy rejects, and the same principal's id in another
/// tenant — on both sides, because a join has two chances to leak.
const FORBIDDEN: [&str; 4] = ["cy", "a-two", "zz", "zz-book"];

async fn seeded() -> (common::Serving, Arc<MemoryStore>) {
    let backing = Arc::new(MemoryStore::new());
    let store = common::store(Arc::clone(&backing));
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    txn.insert_many(
        &root,
        &authors(),
        &[
            author(1, 1, 1, "ada", "UK", 1815),
            author(1, 2, 1, "bo", "US", 1900),
            // Owned by principal 2: hidden from principal 1.
            author(1, 3, 2, "cy", "UK", 1950),
            // Owned, and matches no book: an outer join must preserve it.
            author(1, 4, 1, "di", "FR", 1970),
            // The same principal id, in another tenant.
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
            // Before 2000: hidden by the books policy, and its author is
            // visible — so a join that applied only the left side's policy
            // would return it.
            book(1, 11, 1, "a-two", 1990),
            book(1, 12, 2, "b-one", 2010),
            // Visible itself; its author is not. A join that applied only the
            // right side's policy would return it.
            book(1, 13, 3, "c-one", 2005),
            // Matches no author: an outer join must preserve it.
            book(1, 14, 9, "orphan", 2020),
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
            sale(2, 20, 10, 4),
        ],
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();
    (serving_leader(Arc::clone(&backing)).await, backing)
}

/// What the caller may see, written out by hand.
///
/// Stated rather than computed. The oracle in `multi.rs` compares two engines;
/// this file has to say what the answer *is*, or a policy dropped from both
/// paths at once would pass both files.
fn expected(join_type: JoinType) -> Vec<&'static str> {
    let mut rows = vec!["ada+a-one", "bo+b-one"];
    if join_type.preserves(Side::Left) {
        // `di` owns no books; `cy`'s row is hidden, so it is not preserved
        // either — a hidden row is absent, not unmatched.
        rows.push("di+-");
    }
    if join_type.preserves(Side::Right) {
        // `c-one` is visible and its author is not, so it comes back
        // unmatched — which is exactly what a genuinely missing author would
        // give, so the join is not an existence oracle for `cy`.
        rows.extend(["-+c-one", "-+orphan"]);
    }
    rows.sort_unstable();
    rows
}

/// Render a joined row as `author+title`, with `-` for an absent side.
fn render(row: &[Option<Row>]) -> String {
    let names: Vec<String> = row
        .iter()
        .map(|side| match side {
            None => "-".to_owned(),
            // Column 3 is `name` on `authors` and `title` on `books`, which is
            // the only reason one renderer serves both.
            Some(row) => match row.get(slate_schema::Ordinal(3)) {
                Some(Value::Str(text)) => text.clone(),
                other => panic!("column 3 was {other:?}"),
            },
        })
        .collect();
    names.join("+")
}

fn rendered(rows: &[pb::JoinedRow]) -> Vec<String> {
    let mut out: Vec<String> = rows
        .iter()
        .map(|row| {
            let sides: Vec<Option<Row>> = row
                .inputs
                .iter()
                .map(|input| input.row.as_ref().map(|r| row_from_proto(r).unwrap()))
                .collect();
            render(&sides)
        })
        .collect();
    out.sort();
    out
}

/// Every name anywhere in the result, for the leak check.
fn names(rows: &[String]) -> BTreeSet<String> {
    rows.iter()
        .flat_map(|row| row.split('+').map(str::to_owned))
        .collect()
}

async fn run_join(
    client: &mut RecordsClient<Channel>,
    query: pb::JoinQuery,
    tenant: u64,
) -> Result<Vec<String>, tonic::Status> {
    let stream = client
        .join(app_in(
            pb::JoinRequest {
                transaction: String::new(),
                join: Some(query),
                freshness: None,
            },
            1,
            tenant,
        ))
        .await?
        .into_inner();
    let (rows, _) = drain_joined(stream).await;
    Ok(rendered(&rows))
}

/// Every join type against every algorithm: does each return exactly the rows
/// the policies admit?
#[tokio::test]
async fn every_join_path_applies_every_input_s_policy() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());

    // Collected rather than asserted one at a time. When a change breaks a
    // policy it usually breaks it on more than one path, and a run that stops
    // at the first cell hides how far it spread.
    let mut failures: Vec<String> = Vec::new();
    let mut cells = 0;

    for (kind, join_type) in [
        ("inner", JoinType::Inner),
        ("left outer", JoinType::Left),
        ("right outer", JoinType::Right),
        ("full outer", JoinType::Full),
    ] {
        for (how, force) in [
            ("the planner's choice", None),
            (
                "hash, build left",
                Some(JoinAlgorithm::Hash { build: Side::Left }),
            ),
            (
                "hash, build right",
                Some(JoinAlgorithm::Hash { build: Side::Right }),
            ),
            ("nested loop", Some(JoinAlgorithm::NestedLoop)),
        ] {
            // A nested loop cannot preserve unmatched rows of the second
            // input; the kernel refuses it and so does the wire. That refusal
            // has its own test in `multi.rs`; here it is simply not a cell.
            if matches!(force, Some(JoinAlgorithm::NestedLoop)) && join_type.preserves(Side::Right)
            {
                continue;
            }
            let mut join = Join::equating(at(&a, "id"), at(&b, "author_id"));
            join.join_type = join_type;
            join.force = force;

            let got = match run_join(&mut client, join_to_proto(&a, &b, &join), 1).await {
                Ok(rows) => rows,
                Err(status) => {
                    failures.push(format!("{kind} via {how}: refused with {status:?}"));
                    continue;
                }
            };
            cells += 1;

            let seen = names(&got);
            let leaked: Vec<&str> = FORBIDDEN
                .into_iter()
                .filter(|hidden| seen.contains(*hidden))
                .collect();
            if !leaked.is_empty() {
                failures.push(format!("{kind} via {how}: leaked {leaked:?} in {got:?}"));
                continue;
            }
            let want = expected(join_type);
            if got != want {
                failures.push(format!(
                    "{kind} via {how}: returned {got:?}, expected {want:?}"
                ));
            }
        }
    }

    assert_eq!(cells, 14, "the matrix ran {cells} cells rather than 14");
    assert!(
        failures.is_empty(),
        "{} of the join paths did not apply every input's policy:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// The control: the hidden rows are there, and a caller with no policy sees
/// them.
///
/// Run in process, because nothing on the wire can produce a superuser — which
/// is the point of `auth.rs`, and is why this cannot be a cell of the matrix
/// above. Without it every assertion in this file could be passing because the
/// store never held the rows at all.
#[tokio::test]
async fn without_a_policy_the_same_join_returns_the_rows_the_matrix_forbids() {
    let (_serving, backing) = seeded().await;
    let (a, b) = (authors(), books());
    let store = common::store(Arc::clone(&backing));
    let txn = store.begin().await.unwrap();

    let mut join = Join::equating(at(&a, "id"), at(&b, "author_id"));
    join.join_type = JoinType::Full;
    let mut cursor = txn
        .join(&SecurityContext::superuser(), &a, &b, &join)
        .await
        .unwrap();
    let mut seen = BTreeSet::new();
    while let Some(row) = cursor.next().await.unwrap() {
        for name in render(&[row.left, row.right]).split('+') {
            seen.insert(name.to_owned());
        }
    }

    for hidden in FORBIDDEN {
        assert!(
            seen.contains(hidden),
            "`{hidden}` is not reachable even without a policy, so hiding it proves nothing"
        );
    }
}

/// A different tenant gets its own rows and none of the first tenant's, on
/// every input.
///
/// The tenant restriction is a *narrower scan* rather than a filter, so it is
/// the one part of the policy a join could get right on one side and wrong on
/// the other without the row filter noticing.
#[tokio::test]
async fn a_join_run_as_another_tenant_crosses_nothing() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());
    let join = Join::equating(at(&a, "id"), at(&b, "author_id"));

    let rows = run_join(&mut client, join_to_proto(&a, &b, &join), 2)
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec!["zz+zz-book".to_owned()],
        "tenant 2's join returned {rows:?}"
    );
}

/// The chain applies a policy per step, including the third table's.
#[tokio::test]
async fn every_step_of_a_chain_applies_its_own_policy() {
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

    let stream = client
        .join(app_in(
            pb::JoinRequest {
                transaction: String::new(),
                join: Some(chain_to_proto(&tables, &chain)),
                freshness: None,
            },
            1,
            1,
        ))
        .await
        .unwrap()
        .into_inner();
    let (rows, _) = drain_joined(stream).await;

    // ada -> a-one -> the five-unit sale, and bo -> b-one -> the seven-unit
    // one. The zero-unit sale on `a-one` is hidden by the third table's own
    // policy, so a chain that stopped applying policies after the second step
    // would return three rows rather than two.
    let mut units: Vec<i64> = rows
        .iter()
        .map(|row| {
            let sale = row.inputs[2]
                .row
                .as_ref()
                .map(|r| row_from_proto(r).unwrap())
                .expect("an inner chain has every table");
            match sale.get(slate_schema::Ordinal(3)) {
                Some(Value::I64(units)) => *units,
                other => panic!("units was {other:?}"),
            }
        })
        .collect();
    units.sort_unstable();
    assert_eq!(units, vec![5, 7], "the chain returned sales {units:?}");
}

/// An aggregate over the wire counts the caller's slice, not the table.
///
/// `COUNT(*)` is the easiest place to leak without returning a single
/// forbidden row: it tells the caller how many there are.
#[tokio::test]
async fn an_aggregate_over_the_wire_counts_only_permitted_rows() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;

    let stream = client
        .aggregate(app_in(
            pb::AggregateRequest {
                transaction: String::new(),
                aggregate: Some(pb::AggregateQuery {
                    input: Some(pb::Query {
                        projection: None,
                        ..common::plain_query("books")
                    }),
                    group_by: Vec::new(),
                    aggregates: vec![pb::Aggregate {
                        function: pb::AggregateFunction::Count as i32,
                        column: None,
                    }],
                    having: None,
                }),
                freshness: None,
            },
            1,
            1,
        ))
        .await
        .unwrap()
        .into_inner();
    let (groups, _) = drain_groups(stream).await;

    assert_eq!(groups.len(), 1, "grouping by nothing is one group");
    let count = groups[0].values[0].kind.clone();
    // Six books exist. Tenant 1 has five, of which `a-two` is before 2000, so
    // the caller may count four.
    assert_eq!(
        count,
        Some(pb::value::Kind::Uint64Value(4)),
        "the count was {count:?}, which is not the caller's slice"
    );
}

/// Each input's *own* policy reaches the executor, not the other's.
///
/// The residual is the proof: the planner conjoins the policy before costing,
/// and every conjunct stays in the residual and is re-checked per row. If one
/// input's plan carried the other's policy, or none, it would show here — and
/// it is worth checking separately from the rows, because a join can return
/// the right rows for the wrong reason when the two policies happen to
/// overlap.
#[tokio::test]
async fn each_input_s_plan_carries_its_own_policy() {
    let (serving, _backing) = seeded().await;
    let mut client = serving.client().await;
    let (a, b) = (authors(), books());
    let join = Join::equating(at(&a, "id"), at(&b, "author_id"));

    let answer = client
        .explain_join(app_in(
            pb::ExplainJoinRequest {
                transaction: String::new(),
                join: Some(join_to_proto(&a, &b, &join)),
                freshness: None,
            },
            1,
            1,
        ))
        .await
        .unwrap()
        .into_inner();

    let authors_plan = answer.inputs[0].plan.as_ref().unwrap();
    let books_plan = answer.inputs[1].plan.as_ref().unwrap();

    // `own_authors` compares the owner column against the principal's id.
    assert!(
        authors_plan
            .residual
            .contains(&format!("{:?}", slate_schema::Ordinal(at(&a, "owner").0))),
        "the authors plan does not check the owner: {}",
        authors_plan.residual
    );
    // `modern_books` compares the year against 2000. It must be on the books
    // plan and *not* on the authors one.
    assert!(
        books_plan.residual.contains("2000"),
        "the books plan does not check the year: {}",
        books_plan.residual
    );
    assert!(
        !authors_plan.residual.contains("2000"),
        "the authors plan carries the books policy: {}",
        authors_plan.residual
    );
}
