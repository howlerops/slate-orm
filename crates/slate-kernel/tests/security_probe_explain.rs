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
    Action, CmpOp, Expr, Grant, Join, Principal, Query, RecordStore, SecurityCatalog,
    SecurityContext, Statistics, memory::MemoryStore,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const PAYROLL: TableId = TableId(1);
const DIRECTORY: TableId = TableId(2);
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

/// A table Alice may explain freely, to join the restricted one to.
fn directory() -> TableDef {
    TableDef::builder("directory", DIRECTORY)
        .column("tenant_id", ValueType::U64)
        .column("id", ValueType::U64)
        .primary_key(["tenant_id", "id"])
        .tenant_column("tenant_id")
        .build()
        .expect("valid schema")
}

const DIRECTORY_ID: Ordinal = Ordinal(1);
const PAYROLL_ID: Ordinal = Ordinal(1);

fn security() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("app", PAYROLL, [Action::Read]))
}

/// The same deployment, having decided its readers should keep `EXPLAIN`.
fn security_granting_explain() -> SecurityCatalog {
    SecurityCatalog::new().grant(Grant::new("app", PAYROLL, [Action::Read, Action::Explain]))
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
    seeded_with(security()).await
}

async fn seeded_with(security: SecurityCatalog) -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([payroll()]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
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

/// Was the finding: tenant A read tenant B's salaries out of `EXPLAIN`.
/// A plan is costed against statistics covering the whole table, so a caller
/// who may `Read` is no longer thereby allowed to see one.
#[tokio::test]
async fn explain_is_refused_to_a_caller_holding_only_read() {
    let store = seeded().await;
    let txn = store.begin().await.unwrap();

    // The control is unchanged: Alice can see nothing at all in this table.
    let visible = txn
        .count(&alice(), &payroll(), &Query::all())
        .await
        .unwrap();
    assert_eq!(visible, 0, "Alice should see none of tenant B's rows");

    let refused = txn.explain(
        &alice(),
        &payroll(),
        &Query::all().filter(Expr::compare(SALARY, CmpOp::Ge, Value::I64(1_000))),
    );

    let message = refused
        .expect_err("a Read grant must no longer carry EXPLAIN")
        .to_string();
    assert!(
        message.contains("explain"),
        "the denial should name the action that was missing: {message}"
    );
    // The refusal must not leak what it refused to compute.
    assert!(
        !message.contains("1000") && !message.contains("1,000"),
        "the denial must not echo the probe's literal: {message}"
    );
}

/// What granting `Action::Explain` actually costs, stated as a test so that
/// nobody grants it casually: the disclosure is gated, not removed. A caller
/// who holds it recovers tenant B's smallest salary exactly, by binary search
/// over an estimate derived from rows they cannot read.
#[tokio::test]
async fn granting_explain_reopens_the_recovery_in_full() {
    let store = seeded_with(security_granting_explain()).await;
    let txn = store.begin().await.unwrap();

    let visible = txn
        .count(&alice(), &payroll(), &Query::all())
        .await
        .unwrap();
    assert_eq!(visible, 0, "Alice still reads none of tenant B's rows");

    let estimate = |v: i64| {
        txn.explain(
            &alice(),
            &payroll(),
            &Query::all().filter(Expr::compare(SALARY, CmpOp::Ge, Value::I64(v))),
        )
        .expect("the grant permits EXPLAIN")
        .estimated_rows
    };

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
    assert_eq!(
        lo,
        *salaries.first().unwrap(),
        "with the grant, the estimate's first step still sits on tenant B's \
         smallest salary — this is the cost of reopening it"
    );

    let across: Vec<f64> = (0..8).map(|i| estimate(1_000 + i * 240)).collect();
    assert!(
        across.windows(2).all(|w| w[0] > w[1]),
        "and the whole distribution is still traceable: {across:?}"
    );
}

/// A join is explained per side, so a caller who may explain one table must
/// not read the other's statistics by joining to it. Checking only the first
/// table would leave exactly that open: Alice holds `Explain` on `directory`
/// and only `Read` on `payroll`.
#[tokio::test]
async fn explain_on_one_table_does_not_carry_to_the_other_side_of_a_join() {
    let security = SecurityCatalog::new()
        .grant(Grant::new(
            "app",
            DIRECTORY,
            [Action::Read, Action::Explain],
        ))
        .grant(Grant::new("app", PAYROLL, [Action::Read]));

    let catalog = Catalog::from_tables([payroll(), directory()]).expect("catalog");
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();

    // The permitted side alone explains, so the refusal below is about the
    // other side rather than about EXPLAIN being off altogether.
    txn.explain(&alice(), &directory(), &Query::all())
        .expect("Alice holds Explain on directory");

    let refused = txn.explain_join(
        &alice(),
        &directory(),
        &payroll(),
        &Join::equating(DIRECTORY_ID, PAYROLL_ID),
    );

    let message = refused
        .expect_err("joining to payroll must not borrow directory's grant")
        .to_string();
    assert!(
        message.contains("payroll"),
        "the denial should name the table that was missing the grant: {message}"
    );

    // And in the other order, so the check cannot be passing merely because
    // the restricted table happens to be second.
    txn.explain_join(
        &alice(),
        &payroll(),
        &directory(),
        &Join::equating(PAYROLL_ID, DIRECTORY_ID),
    )
    .expect_err("nor with the restricted table first");
}
