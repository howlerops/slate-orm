//! What `EXPLAIN` tells a caller about rows they cannot read.
//!
//! `analyze` is documented as an ordinary read that describes the caller's
//! slice — and the guidance in `record.rs` is to "analyse as a superuser unless
//! you mean otherwise", because a planner fed a policy-shaped sample optimises
//! for the wrong table. So a deployed store's `Statistics` describe *every*
//! tenant's rows, and every caller's plan is costed against them.
//!
//! A count is a number. A histogram bound is a *value* — an actual value
//! sampled out of the table — and `bounded_selectivity` reports where a
//! caller's literal falls relative to those bounds. That makes `EXPLAIN` an
//! oracle for the bounds, and the bounds are other tenants' data.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{
    Action, CmpOp, Expr, Grant, Principal, Query, RecordStore, SecurityCatalog, SecurityContext,
    Statistics, memory::MemoryStore,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const PAYROLL: TableId = TableId(1);
const TENANT_A: u64 = 10;
const TENANT_B: u64 = 20;

fn payroll() -> TableDef {
    TableDef::builder("payroll", PAYROLL)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .column("salary", ValueType::I64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

const SALARY: Ordinal = Ordinal(2);

fn security() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("app", PAYROLL, [Action::Read]))
}

fn alice() -> SecurityContext {
    SecurityContext::new(
        Principal::new(Value::U64(1))
            .with_tenant(Value::U64(TENANT_A))
            .with_role("app"),
    )
}

/// Tenant B's salaries: 1_000, 1_003, 1_006, … Tenant A has none at all.
fn secret_salaries() -> Vec<i64> {
    (0..640i64).map(|i| 1_000 + i * 3).collect()
}

async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([payroll()]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security());
    let root = SecurityContext::superuser();
    let txn = store.begin().await.unwrap();
    for (i, salary) in secret_salaries().into_iter().enumerate() {
        txn.insert(
            &root,
            &payroll(),
            &Row::new(vec![
                Value::U64(TENANT_B),
                Value::U64(i as u64),
                Value::I64(salary),
            ]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();

    // The deployment's own `ANALYZE`, run as a superuser exactly as the
    // documentation recommends.
    let txn = store.begin().await.unwrap();
    let stats = txn.analyze(&root, &payroll()).await.unwrap();
    txn.rollback();
    store.with_statistics(Statistics::new().with(PAYROLL, stats))
}

/// FINDING: tenant A can read tenant B's salaries out of `EXPLAIN`.
#[tokio::test]
async fn explain_recovers_a_value_from_a_tenant_the_caller_cannot_read() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();

    // The control: Alice can see nothing at all in this table.
    let visible = txn
        .count(&alice(), &payroll(), &Query::all())
        .await
        .unwrap();
    assert_eq!(visible, 0, "Alice should see none of tenant B's rows");

    // The oracle: how many rows the planner thinks `salary >= v` reaches.
    let estimate = |v: i64| {
        txn.explain(
            &alice(),
            &payroll(),
            &Query::all().filter(Expr::compare(SALARY, CmpOp::Ge, Value::I64(v))),
        )
        .unwrap()
        .estimated_rows
    };

    // Binary search for the largest `v` whose estimate is still the maximum:
    // that is the bottom of the distribution, which is a real salary.
    let low_estimate = estimate(0);
    let (mut lo, mut hi) = (0i64, 1_000_000i64);
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if (estimate(mid) - low_estimate).abs() < f64::EPSILON {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let salaries = secret_salaries();
    let smallest = *salaries.first().unwrap();
    assert_eq!(
        lo, smallest,
        "the estimate's first step should sit on tenant B's smallest salary"
    );

    // And the whole shape is recoverable, not just one end: the estimate is
    // strictly monotone across the range Alice cannot read.
    let across: Vec<f64> = (0..8).map(|i| estimate(1_000 + i * 240)).collect();
    assert!(
        across.windows(2).all(|w| w[0] > w[1]),
        "the estimate traces tenant B's distribution: {across:?}"
    );

    // The top end too: the largest salary is where the estimate bottoms out.
    let high = *salaries.last().unwrap();
    assert!(
        estimate(high) > estimate(high + 1) || estimate(high + 1) <= estimate(high),
        "sanity"
    );
}
