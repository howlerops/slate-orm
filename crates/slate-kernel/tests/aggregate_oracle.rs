//! Grouping agrees with folding the rows by hand.
//!
//! Aggregation sits behind the same access paths as everything else, and it
//! collapses the result — which is what makes its bugs quiet. A scan that
//! returns one row too few is visible; a `SUM` that is short by one row's worth
//! is a number, and numbers look like numbers.
//!
//! Two properties:
//!
//! 1. **The groups are right.** Compared against a fold written out over the
//!    rows themselves, sharing no code with the executor's hash grouping.
//! 2. **The access path does not matter.** The same grouping forced down a
//!    table scan and down each index must produce identical groups, which
//!    catches an index path that feeds grouping a differently-decoded row —
//!    the same shape as the `ORDER BY` bug that started all this.
//!
//! `Avg` is compared with a tolerance and the rest exactly. Averaging is the
//! only aggregate here that divides, and a sum of `i64` folded in a different
//! order is bit-identical while a mean is not.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::prelude::*;
use slate_kernel::{
    AccessHint, Action, Aggregate, CmpOp, Expr, Grant, Group, Query, RecordStore, Scalar,
    SecurityCatalog, SecurityContext, memory::MemoryStore,
};
use slate_schema::{Catalog, IndexDef, IndexId, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};
use std::collections::{BTreeMap, BTreeSet};

const SALES: TableId = TableId(1);
const ROWS: u64 = 200;
const INDEXES: [IndexId; 2] = [IndexId(10), IndexId(11)];

fn sales() -> TableDef {
    TableDef::builder("sales", SALES)
        .column("id", ValueType::U64)
        .column("region", ValueType::Str)
        .column("channel", ValueType::Str)
        .column("amount", ValueType::I64)
        .nullable_column("discount", ValueType::I64)
        .primary_key(["id"])
        .index(IndexDef::builder("by_region", IndexId(10)).column("region"))
        .index(IndexDef::builder("by_amount", IndexId(11)).column("amount"))
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    sales().ordinal_of(name).expect("column exists")
}

/// `discount` is null on a quarter of the rows, which is what separates
/// `COUNT(*)` from `COUNT(discount)` and decides whether `MIN` has anything to
/// return for a group.
fn row(id: u64) -> Row {
    Row::new(vec![
        Value::U64(id),
        Value::Str(format!("region-{}", id % 4)),
        Value::Str(format!("ch-{}", id % 3)),
        Value::I64((id % 17) as i64 - 8),
        if id.is_multiple_of(4) {
            Value::Null
        } else {
            Value::I64((id % 5) as i64)
        },
    ])
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([sales()]).expect("catalog");
    let security = SecurityCatalog::new().grant(Grant::new("r", SALES, Action::ALL));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    txn.insert_many(&root, &sales(), &(0..ROWS).map(row).collect::<Vec<_>>())
        .await
        .unwrap();
    txn.commit().await.unwrap();
    store
}

fn root() -> SecurityContext {
    SecurityContext::superuser()
}

// --- generators -----------------------------------------------------------

fn any_filter() -> impl Strategy<Value = Expr> {
    prop_oneof![
        Just(Expr::True),
        (0..4u64).prop_map(|r| Expr::eq(col("region"), Value::Str(format!("region-{r}")))),
        (-8i64..9).prop_map(|n| Expr::compare(col("amount"), CmpOp::Ge, Value::I64(n))),
        (0..ROWS).prop_map(|n| Expr::compare(col("id"), CmpOp::Lt, Value::U64(n))),
        Just(Expr::is_null(col("discount"))),
        Just(Expr::Not(Box::new(Expr::is_null(col("discount"))))),
        Just(Expr::like(col("region"), "region-1%")),
    ]
}

/// Grouping keys, including none at all — which collapses to a single group and
/// is the shape `SELECT COUNT(*) FROM t` takes.
fn any_grouping() -> impl Strategy<Value = Vec<Ordinal>> {
    prop_oneof![
        Just(vec![]),
        Just(vec![col("region")]),
        Just(vec![col("channel")]),
        Just(vec![col("region"), col("channel")]),
        Just(vec![col("discount")]),
        Just(vec![col("amount")]),
    ]
}

fn any_aggregates() -> impl Strategy<Value = Vec<Aggregate>> {
    let one = prop_oneof![
        Just(Aggregate::Count),
        Just(Aggregate::CountColumn(col("discount"))),
        Just(Aggregate::Min(col("amount"))),
        Just(Aggregate::Max(col("amount"))),
        Just(Aggregate::Sum(col("amount"))),
        Just(Aggregate::Avg(col("amount"))),
        Just(Aggregate::CountDistinct(col("channel"))),
        Just(Aggregate::CountDistinct(col("discount"))),
        Just(Aggregate::Min(col("discount"))),
        Just(Aggregate::Sum(col("discount"))),
    ];
    proptest::collection::vec(one, 1..4)
}

// --- the fold, written out --------------------------------------------------

/// Group and aggregate the rows directly, sharing nothing with the executor.
fn brute_force(
    filter: &Expr,
    grouping: &[Ordinal],
    aggregates: &[Aggregate],
) -> Vec<(Vec<Value>, Vec<Value>)> {
    let matching: Vec<Row> = (0..ROWS).map(row).filter(|r| filter.admits(r)).collect();

    // Keyed by the encoded grouping values, which is the same notion of
    // equality an index uses — and the only one that orders `Value` totally.
    let mut groups: BTreeMap<Vec<u8>, (Vec<Value>, Vec<Row>)> = BTreeMap::new();
    for r in matching {
        let key: Vec<Value> = grouping.iter().map(|o| r.values()[o.0].clone()).collect();
        let encoded = slate_tuple::encode(&key);
        groups
            .entry(encoded)
            .or_insert_with(|| (key, Vec::new()))
            .1
            .push(r);
    }

    let mut out: Vec<(Vec<Value>, Vec<Value>)> = groups
        .into_values()
        .map(|(key, rows)| {
            let values = aggregates.iter().map(|a| fold(a, &rows)).collect();
            (key, values)
        })
        .collect();
    out.sort_by_key(|(key, _)| slate_tuple::encode(key));
    out
}

fn fold(aggregate: &Aggregate, rows: &[Row]) -> Value {
    let non_null = |o: Ordinal| -> Vec<&Value> {
        rows.iter()
            .map(|r| &r.values()[o.0])
            .filter(|v| !v.is_null())
            .collect()
    };
    let as_i64 = |v: &Value| match v {
        Value::I64(n) => *n,
        other => panic!("expected an i64, got {other:?}"),
    };

    match aggregate {
        Aggregate::Count => Value::U64(rows.len() as u64),
        Aggregate::CountColumn(o) => Value::U64(non_null(*o).len() as u64),
        Aggregate::Min(o) => non_null(*o)
            .into_iter()
            .min()
            .cloned()
            .unwrap_or(Value::Null),
        Aggregate::Max(o) => non_null(*o)
            .into_iter()
            .max()
            .cloned()
            .unwrap_or(Value::Null),
        Aggregate::Sum(o) => {
            let values = non_null(*o);
            if values.is_empty() {
                Value::Null
            } else {
                Value::I64(values.into_iter().map(as_i64).sum())
            }
        }
        Aggregate::Avg(o) => {
            let values = non_null(*o);
            if values.is_empty() {
                Value::Null
            } else {
                let n = values.len() as f64;
                let total: i64 = values.into_iter().map(as_i64).sum();
                Value::F64(total as f64 / n)
            }
        }
        Aggregate::CountDistinct(o) => {
            let distinct: BTreeSet<Vec<u8>> = rows
                .iter()
                .map(|r| &r.values()[o.0])
                .filter(|v| !v.is_null())
                .map(|v| slate_tuple::encode(core::slice::from_ref(v)))
                .collect();
            Value::U64(distinct.len() as u64)
        }
    }
    // Deliberately exhaustive: a new `Aggregate` variant should fail to compile
    // here rather than quietly go untested.
}

// --- helpers --------------------------------------------------------------

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

fn normalise(groups: Vec<Group>) -> Vec<(Vec<Value>, Vec<Value>)> {
    let mut out: Vec<(Vec<Value>, Vec<Value>)> =
        groups.into_iter().map(|g| (g.key, g.values)).collect();
    out.sort_by_key(|(key, _)| slate_tuple::encode(key));
    out
}

/// Exact everywhere except a mean, which two summation orders need not agree on
/// to the last bit.
fn same(a: &[(Vec<Value>, Vec<Value>)], b: &[(Vec<Value>, Vec<Value>)]) -> Result<(), String> {
    if a.len() != b.len() {
        return Err(format!("{} groups against {}", a.len(), b.len()));
    }
    for ((ka, va), (kb, vb)) in a.iter().zip(b) {
        if ka != kb {
            return Err(format!("group key {ka:?} against {kb:?}"));
        }
        for (x, y) in va.iter().zip(vb) {
            let equal = match (x, y) {
                (Value::F64(p), Value::F64(q)) => {
                    (p - q).abs() < 1e-9 || (p.is_nan() && q.is_nan())
                }
                _ => x == y,
            };
            if !equal {
                return Err(format!("group {ka:?}: {va:?} against {vb:?}"));
            }
        }
    }
    Ok(())
}

// --- the properties -------------------------------------------------------

/// Grouping matches folding the rows by hand.
#[test]
fn grouping_agrees_with_a_brute_force_fold() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|(
        filter in any_filter(),
        grouping in any_grouping(),
        aggregates in any_aggregates(),
    )| {
        let expected = brute_force(&filter, &grouping, &aggregates);
        let got = normalise(rt.block_on(async {
            let txn = store.begin().await.unwrap();
            txn.group_by(
                &root(),
                &sales(),
                &Query::all().filter(filter.clone()),
                &grouping,
                &aggregates,
            )
            .await
            .unwrap()
        }));

        if let Err(why) = same(&got, &expected) {
            prop_assert!(
                false,
                "grouping by {grouping:?} with {aggregates:?} under {filter:?}: {why}"
            );
        }
    });
}

/// The access path does not change the groups.
///
/// Grouping reads whatever the access path decoded, so an index path that
/// leaves a grouping column or an aggregate's column undecoded would group
/// everything under null and produce a plausible, wrong answer.
#[test]
fn every_access_path_produces_the_same_groups() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|(
        filter in any_filter(),
        grouping in any_grouping(),
        aggregates in any_aggregates(),
    )| {
        let run = |hint: Option<AccessHint>| {
            let filter = filter.clone();
            let grouping = grouping.clone();
            let aggregates = aggregates.clone();
            rt.block_on(async {
                let mut query = Query::all().filter(filter);
                query.hint = hint;
                let txn = store.begin().await.unwrap();
                normalise(
                    txn.group_by(&root(), &sales(), &query, &grouping, &aggregates)
                        .await
                        .unwrap(),
                )
            })
        };

        let expected = run(Some(AccessHint::TableScan));
        for index in INDEXES {
            let got = run(Some(AccessHint::Index(index)));
            if let Err(why) = same(&got, &expected) {
                prop_assert!(false, "index {index:?} grouped differently: {why}");
            }
        }
        let chosen = run(None);
        if let Err(why) = same(&chosen, &expected) {
            prop_assert!(false, "the planner's choice grouped differently: {why}");
        }
    });
}

/// `HAVING` keeps exactly the groups the predicate admits.
///
/// Checked by filtering the unfiltered result the same way, so this tests the
/// `HAVING` path rather than re-deriving what the groups should be.
#[test]
fn having_keeps_exactly_the_groups_the_predicate_admits() {
    let rt = runtime();
    let store = rt.block_on(seeded());

    proptest!(|(
        filter in any_filter(),
        grouping in any_grouping(),
        threshold in 0..12u64,
    )| {
        let aggregates = vec![Aggregate::Count, Aggregate::Sum(col("amount"))];
        // The count is the first aggregate, which sits after the grouping keys.
        let having = Expr::compare(
            Group::aggregate(grouping.len(), 0),
            CmpOp::Gt,
            Value::U64(threshold),
        );

        let (all, kept) = rt.block_on(async {
            let txn = store.begin().await.unwrap();
            let query = Query::all().filter(filter.clone());
            let all = txn
                .group_by(&root(), &sales(), &query, &grouping, &aggregates)
                .await
                .unwrap();
            let kept = txn
                .group_by_having(&root(), &sales(), &query, &grouping, &aggregates, &having)
                .await
                .unwrap();
            (all, kept)
        });

        let expected: Vec<(Vec<Value>, Vec<Value>)> = normalise(all)
            .into_iter()
            .filter(|(_, values)| matches!(values[0], Value::U64(n) if n > threshold))
            .collect();
        let got = normalise(kept);

        if let Err(why) = same(&got, &expected) {
            prop_assert!(false, "HAVING COUNT(*) > {threshold} on {grouping:?}: {why}");
        }
    });
}

/// Grouping by a computed value is grouping.
///
/// A computed column lands in the same ordinal space as the table's own, which
/// is what lets grouping take one without knowing what an expression is — so
/// the thing worth checking is that it really is indistinguishable.
#[test]
fn grouping_by_a_computed_value_matches_grouping_by_an_equal_column() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let table = sales();

    // `amount * 1` computed, against `amount` itself: same partition, so the
    // groups must match one for one.
    let computed = Query::computed(&table, 0);
    let groups = rt.block_on(async {
        let txn = store.begin().await.unwrap();
        let by_computed = txn
            .group_by(
                &root(),
                &table,
                &Query::all().computing([Scalar::column(col("amount")) * 1i64]),
                &[computed],
                &[Aggregate::Count, Aggregate::Sum(col("amount"))],
            )
            .await
            .unwrap();
        let by_column = txn
            .group_by(
                &root(),
                &table,
                &Query::all(),
                &[col("amount")],
                &[Aggregate::Count, Aggregate::Sum(col("amount"))],
            )
            .await
            .unwrap();
        (normalise(by_computed), normalise(by_column))
    });

    assert!(!groups.0.is_empty(), "premise: there are groups");
    same(&groups.0, &groups.1).expect("a computed key grouped differently from the column");
}

/// The generators have to produce more than one group and more than one row per
/// group, or none of the above is testing aggregation.
#[test]
fn the_generated_groupings_are_not_degenerate() {
    let rt = runtime();
    let store = rt.block_on(seeded());
    let seen = std::cell::RefCell::new(Vec::new());

    proptest!(ProptestConfig::with_cases(400), |(
        filter in any_filter(),
        grouping in any_grouping(),
    )| {
        let groups = rt.block_on(async {
            let txn = store.begin().await.unwrap();
            txn.group_by(
                &root(),
                &sales(),
                &Query::all().filter(filter.clone()),
                &grouping,
                &[Aggregate::Count],
            )
            .await
            .unwrap()
        });
        let rows: u64 = groups
            .iter()
            .map(|g| match g.values[0] {
                Value::U64(n) => n,
                _ => 0,
            })
            .sum();
        seen.borrow_mut().push((groups.len(), rows));
    });

    let seen = seen.into_inner();
    let multi_group = seen.iter().filter(|(g, _)| *g > 1).count();
    let multi_row = seen
        .iter()
        .filter(|(g, r)| *g > 0 && *r as usize > *g)
        .count();
    assert!(
        multi_group * 3 > seen.len(),
        "only {multi_group} of {} cases produced more than one group",
        seen.len()
    );
    // Observed at essentially 100%; the bar is 70%.
    assert!(
        multi_row * 10 > seen.len() * 7,
        "only {multi_row} of {} cases had groups with more than one row",
        seen.len()
    );
}
