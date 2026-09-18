//! Arithmetic over exact decimals, and the expressions that are refused.
//!
//! `decimals.rs` establishes that a decimal column stores money exactly.  This
//! file is about what can be *computed* from one, which is a narrower set than
//! it looks, and about the refusals — because the alternative to a refusal here
//! is a number at a scale nobody stated, which is the drift the type exists to
//! prevent wearing a different hat.
//!
//! The rule the refusals enforce, in one sentence: an expression is
//! expressible when its answer is still a count of the *same* unit its
//! operands were counts of.  `price * quantity` is (cents × a plain count is
//! cents); `price * discount` is not (cents × cents is hundredths of a cent,
//! at a scale no column has).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::float_cmp
)]

use slate_kernel::memory::MemoryStore;
use slate_kernel::{
    Action, Aggregate, Chain, CmpOp, Expr, Grant, Join, JoinKey, JoinSchema, JoinStep, Principal,
    Query, RecordStore, Scalar, SecurityCatalog, SecurityContext,
};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Value, ValueType};

const LINES: TableId = TableId(1);
const ORDERS: TableId = TableId(2);

/// A line of an order: money at scale 2, a plain count, and a rate at scale 4.
///
/// Two different scales on one table is deliberate — `price + rate` is the
/// refusal that is easiest to write by accident, and it needs two scales to
/// exist at all.
fn lines() -> TableDef {
    TableDef::builder("lines", LINES)
        .column("id", ValueType::U64)
        .column("order_id", ValueType::U64)
        .decimal_column("price", 2)
        .column("quantity", ValueType::I64)
        .decimal_column("discount", 2)
        .decimal_column("rate", 4)
        .primary_key(["id"])
        .build()
        .unwrap()
}

fn orders() -> TableDef {
    TableDef::builder("orders", ORDERS)
        .column("id", ValueType::U64)
        .decimal_column("shipping", 2)
        .decimal_column("levy", 4)
        .primary_key(["id"])
        .build()
        .unwrap()
}

fn col(name: &str) -> Ordinal {
    lines().ordinal_of(name).expect("column exists")
}

fn context() -> SecurityContext {
    SecurityContext::new(Principal::new(Value::U64(1)).with_role("app"))
}

/// Four lines, priced so that the sums are checkable by hand.
async fn seeded() -> RecordStore<MemoryStore> {
    let catalog = Catalog::from_tables([lines(), orders()]).unwrap();
    let security = SecurityCatalog::new()
        .grant(Grant::new("app", LINES, Action::EVERYTHING))
        .grant(Grant::new("app", ORDERS, Action::EVERYTHING));
    let store = RecordStore::new(MemoryStore::new(), catalog, security);
    let txn = store.begin().await.unwrap();
    // price, quantity, discount, rate
    let cells: [(i64, i64, i64, i64); 4] = [
        (1295, 3, 100, 825),
        (999, 7, 0, 825),
        (1450, 1, 250, 600),
        (75, 40, 5, 600),
    ];
    for (index, (price, quantity, discount, rate)) in cells.iter().enumerate() {
        let id = index as u64 + 1;
        txn.insert(
            &context(),
            &lines(),
            &Row::new(vec![
                Value::U64(id),
                Value::U64(if id <= 2 { 1 } else { 2 }),
                Value::Decimal(*price),
                Value::I64(*quantity),
                Value::Decimal(*discount),
                Value::Decimal(*rate),
            ]),
        )
        .await
        .unwrap();
    }
    for id in 1..=2u64 {
        txn.insert(
            &context(),
            &orders(),
            &Row::new(vec![
                Value::U64(id),
                Value::Decimal(500 * id as i64),
                Value::Decimal(825),
            ]),
        )
        .await
        .unwrap();
    }
    txn.commit().await.unwrap();
    store
}

/// Run `compute` over every line and hand back the computed column.
async fn computed(store: &RecordStore<MemoryStore>, compute: Scalar) -> Vec<Value> {
    let table = lines();
    let at = Query::computed(&table, 0);
    let txn = store.begin().await.unwrap();
    let rows = txn
        .execute(&context(), &table, &Query::all().computing([compute]))
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    rows.iter()
        .map(|row| row.get(at).cloned().unwrap_or(Value::Null))
        .collect()
}

/// The refusal a query gets, or a panic naming what it returned instead.
async fn refusal(store: &RecordStore<MemoryStore>, compute: Scalar) -> String {
    let table = lines();
    let txn = store.begin().await.unwrap();
    match txn
        .execute(&context(), &table, &Query::all().computing([compute]))
        .await
    {
        Err(error) => error.to_string(),
        Ok(_) => panic!("expected a refusal, got a cursor"),
    }
}

// ---------------------------------------------------------------- what works

/// Cents times a count is cents. The one multiplication that is expressible,
/// and the one an invoice actually needs.
#[tokio::test]
async fn price_times_a_count_is_still_money() {
    let store = seeded().await;
    let values = computed(
        &store,
        Scalar::column(col("price")) * Scalar::column(col("quantity")),
    )
    .await;
    assert_eq!(
        values,
        vec![
            Value::Decimal(3885),
            Value::Decimal(6993),
            Value::Decimal(1450),
            Value::Decimal(3000),
        ],
        "a decimal times an integer stays a decimal, at the same scale"
    );
}

/// And a count times cents, which is the same expression written the other way
/// round. Multiplication commutes and the rule has to as well — the arm that
/// makes it so is a separate one in `decimal_op`, so it gets its own case.
#[tokio::test]
async fn a_count_times_price_is_the_same_money() {
    let store = seeded().await;
    let values = computed(
        &store,
        Scalar::column(col("quantity")) * Scalar::column(col("price")),
    )
    .await;
    assert_eq!(values[0], Value::Decimal(3885));
    assert_eq!(values[3], Value::Decimal(3000));
}

/// Cents minus cents is cents, when both are counts of the same cent.
#[tokio::test]
async fn subtracting_at_the_same_scale_is_money() {
    let store = seeded().await;
    let values = computed(
        &store,
        Scalar::column(col("price")) - Scalar::column(col("discount")),
    )
    .await;
    assert_eq!(
        values,
        vec![
            Value::Decimal(1195),
            Value::Decimal(999),
            Value::Decimal(1200),
            Value::Decimal(70),
        ]
    );
}

/// A literal decimal has no scale of its own and takes the column's.
///
/// This is the same thing a numeric literal does in SQL, and it is why
/// `Value::Decimal` on the wire needs no scale field: the schema on the far
/// side supplies it.
#[tokio::test]
async fn a_decimal_literal_takes_the_scale_beside_it() {
    let store = seeded().await;
    let values = computed(
        &store,
        Scalar::column(col("price")) + Scalar::literal(Value::Decimal(5)),
    )
    .await;
    assert_eq!(values[0], Value::Decimal(1300), "12.95 + 0.05 = 13.00");
    assert_eq!(values[3], Value::Decimal(80));
}

/// Dividing money by a count truncates toward zero, in both directions.
///
/// Rounding is the caller's decision and there is no scale to round *to* — the
/// answer is in the same cents the input was — so the kernel does the one
/// thing it can do without inventing a policy, and says so.
#[tokio::test]
async fn dividing_by_a_count_truncates_toward_zero() {
    let store = seeded().await;
    let values = computed(
        &store,
        Scalar::column(col("price")) / Scalar::literal(Value::I64(4)),
    )
    .await;
    // 1295/4 = 323.75 units, 999/4 = 249.75, 1450/4 = 362.5, 75/4 = 18.75.
    assert_eq!(
        values,
        vec![
            Value::Decimal(323),
            Value::Decimal(249),
            Value::Decimal(362),
            Value::Decimal(18),
        ]
    );

    // Toward zero, not toward negative infinity: -1295/4 is -323, not -324.
    let negative = computed(
        &store,
        (Scalar::literal(Value::Decimal(0)) - Scalar::column(col("price")))
            / Scalar::literal(Value::I64(4)),
    )
    .await;
    assert_eq!(negative[0], Value::Decimal(-323));
}

/// Summing a computed decimal gives a decimal, not a float.
///
/// **A claim that these four totals drift as `f64` is withdrawn.** The test was
/// written asserting that `38.85 + 69.93 + 14.50 + 30.00` added as doubles is
/// *not* 153.28, on the reasoning that cents are not representable in binary.
/// It is 153.28 — four addends is not enough for the error to escape the
/// rounding, and `decimals.rs`'s hundred dimes (which gives 9.99999999999998)
/// is what a drifting sum actually looks like.
///
/// So what this asserts is the narrower true thing, and it is the one that
/// regresses: the computed value is still a `Decimal` at the far end of an
/// aggregate. Before the decimal arm in `arithmetic`, `price * quantity`
/// widened to `f64` and `Sum` returned `F64(153.28)` — the right number, of the
/// wrong type, which is how this class of bug gets shipped.
#[tokio::test]
async fn summing_a_computed_decimal_is_exact() {
    let store = seeded().await;
    let table = lines();
    let at = Query::computed(&table, 0);
    let txn = store.begin().await.unwrap();
    let values = txn
        .aggregate(
            &context(),
            &table,
            &Query::all()
                .computing([Scalar::column(col("price")) * Scalar::column(col("quantity"))]),
            &[Aggregate::Sum(at)],
        )
        .await
        .unwrap();
    assert_eq!(
        values[0],
        Value::Decimal(3885 + 6993 + 1450 + 3000),
        "a sum over computed cents is cents"
    );
    assert!(
        !matches!(values[0], Value::F64(_)),
        "the aggregate widened the decimal to a float"
    );
}

/// A computed decimal may be read by a *later* computed value, and keeps its
/// scale when it is.
///
/// The first version of the plan-time check gave `scale_of` the table alone, so
/// ordinal 6 — the first computed value — answered "not a decimal" and
/// `computed_0 + price` was refused as a decimal added to a plain number. This
/// test is the one named in `check_scales`'s doc comment.
#[tokio::test]
async fn compute_reading_an_earlier_computed_decimal() {
    let store = seeded().await;
    let table = lines();
    let first = Query::computed(&table, 0);
    let second = Query::computed(&table, 1);
    let txn = store.begin().await.unwrap();
    let query = Query::all().computing([
        Scalar::column(col("price")) - Scalar::column(col("discount")),
        Scalar::column(first) + Scalar::column(col("discount")),
    ]);
    let rows = txn
        .execute(&context(), &table, &query)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    // (price - discount) + discount is price again, at scale 2 throughout.
    assert_eq!(rows[0].get(second), Some(&Value::Decimal(1295)));
    assert_eq!(rows[2].get(second), Some(&Value::Decimal(1450)));
}

/// And a *bad* expression reading an earlier computed decimal is still refused.
///
/// The half of the previous test that keeps the fix from being "trust whatever
/// a computed ordinal is": the scale of a computed value is carried forward,
/// so mixing it with the scale-4 column is the same refusal a column would get.
#[tokio::test]
async fn a_later_compute_mixing_scales_is_refused() {
    let store = seeded().await;
    let table = lines();
    let first = Query::computed(&table, 0);
    let txn = store.begin().await.unwrap();
    let query = Query::all().computing([
        Scalar::column(col("price")) - Scalar::column(col("discount")),
        Scalar::column(first) + Scalar::column(col("rate")),
    ]);
    let error = txn
        .execute(&context(), &table, &query)
        .await
        .expect_err("scale 2 and scale 4 do not add")
        .to_string();
    assert!(error.contains("different scales"), "{error}");
}

// ------------------------------------------------------------ what is refused

/// Two decimal literals added together are a decimal at a scale nobody stated.
///
/// The case that makes a decimal literal's scale-agnosticism a rule rather
/// than a shrug: a literal takes its scale from the column beside it, and with
/// no column beside it there is nothing to take.
#[tokio::test]
async fn two_decimal_literals_with_no_column_are_refused() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::literal(Value::Decimal(100)) + Scalar::literal(Value::Decimal(250)),
    )
    .await;
    assert!(error.contains("no column to take a scale from"), "{error}");

    // Where the same shape over plain integers is ordinary arithmetic.
    let values = computed(
        &store,
        Scalar::literal(Value::I64(100)) + Scalar::literal(Value::I64(250)),
    )
    .await;
    assert_eq!(values[0], Value::I64(350));
}

/// Two scales do not add, because the answer would be a count of neither unit.
#[tokio::test]
async fn adding_different_scales_is_refused() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::column(col("price")) + Scalar::column(col("rate")),
    )
    .await;
    assert!(error.contains("different scales"), "{error}");
    assert!(
        error.contains("lines"),
        "the message names the table: {error}"
    );
}

/// Money plus a plain number is refused rather than guessed at.
///
/// `price + 1` could mean a cent or a dollar and the kernel will not pick. A
/// caller who means a cent writes `Value::Decimal(1)`, which the test above
/// shows adopting the column's scale.
#[tokio::test]
async fn money_plus_a_plain_number_is_refused() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::column(col("price")) + Scalar::literal(Value::I64(1)),
    )
    .await;
    assert!(error.contains("decimal"), "{error}");
}

/// Cents times cents is hundredths of a cent, which no column is at.
#[tokio::test]
async fn money_times_money_is_refused() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::column(col("price")) * Scalar::column(col("discount")),
    )
    .await;
    assert!(error.contains("multiplying"), "{error}");
}

/// Dividing by money gives a ratio, which is not money at any scale.
#[tokio::test]
async fn dividing_by_money_is_refused() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::column(col("price")) / Scalar::column(col("discount")),
    )
    .await;
    assert!(error.contains("dividing"), "{error}");
}

/// Rounding money to "the nearest integer" would have to pick which integer —
/// the unit or the whole — and either answer silently changes the scale.
#[tokio::test]
async fn rounding_money_is_refused() {
    let store = seeded().await;
    let error = refusal(&store, Scalar::column(col("price")).round()).await;
    assert!(error.contains("round"), "{error}");
}

/// A string function over money is a category error and is named as one.
#[tokio::test]
async fn a_string_function_over_money_is_refused() {
    let store = seeded().await;
    let error = refusal(&store, Scalar::column(col("price")).length()).await;
    assert!(error.contains("decimal"), "{error}");
}

/// Two branches of a `case` at different scales would give the same column two
/// meanings, row by row, with nothing in the answer to say which.
#[tokio::test]
async fn case_branches_at_different_scales_are_refused() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::Case {
            branches: vec![(
                Expr::compare(col("quantity"), CmpOp::Gt, Value::I64(2)),
                Scalar::column(col("price")),
            )],
            otherwise: Box::new(Scalar::column(col("rate"))),
        },
    )
    .await;
    assert!(error.contains("scale"), "{error}");
}

/// The same, through `coalesce`, which is a `case` with the conditions implied.
#[tokio::test]
async fn coalesce_across_scales_is_refused() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::Coalesce(vec![
            Scalar::column(col("price")),
            Scalar::column(col("rate")),
        ]),
    )
    .await;
    assert!(error.contains("scale"), "{error}");
}

// ----------------------------------------------------- the other three paths

/// A join's computed value is in the joined ordinal space and is checked there.
///
/// `validate_compute` does the scale check beside the ordinal check for exactly
/// this: a joined `compute` never passes through `Reads::execute`, so the
/// single-table check cannot see it.
#[tokio::test]
async fn a_joined_compute_is_checked() {
    let store = seeded().await;
    let (lines, orders) = (lines(), orders());
    let schema = JoinSchema::of(&lines, &orders);
    let txn = store.begin().await.unwrap();

    // lines.price is scale 2; orders.levy is scale 4.
    let join = Join::on([JoinKey::new(col("order_id"), Ordinal(0))])
        .computing([
            Scalar::column(schema.left(col("price"))) + Scalar::column(schema.right(Ordinal(2)))
        ]);
    let error = txn
        .join(&context(), &lines, &orders, &join)
        .await
        .expect_err("scale 2 and scale 4 do not add across a join either")
        .to_string();
    assert!(error.contains("different scales"), "{error}");

    // And the expressible one goes through: price + shipping, both scale 2.
    let ok = Join::on([JoinKey::new(col("order_id"), Ordinal(0))]).computing([Scalar::column(
        schema.left(col("price")),
    ) + Scalar::column(
        schema.right(Ordinal(1)),
    )]);
    let rows = txn
        .join(&context(), &lines, &orders, &ok)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 4);
    for row in &rows {
        assert!(
            matches!(&row.computed.first(), Some(Value::Decimal(_))),
            "{:?}",
            &row.computed
        );
    }
}

/// A chain's computed value, the n-way version of the same space.
#[tokio::test]
async fn a_chained_compute_is_checked() {
    let store = seeded().await;
    let (lines, orders) = (lines(), orders());
    let tables = [&lines, &orders];
    let schema = JoinSchema::over(tables);
    let txn = store.begin().await.unwrap();

    let chain = Chain::from(Query::all())
        .join(JoinStep::equating(col("order_id"), Ordinal(0)))
        .computing([
            Scalar::column(schema.at(0, col("price"))) * Scalar::column(schema.at(1, Ordinal(1)))
        ]);
    let error = txn
        .chain(&context(), &tables, &chain)
        .await
        .expect_err("money times money is refused in a chain too")
        .to_string();
    assert!(error.contains("multiplying"), "{error}");
}

/// And a chain *step*'s own `Query::compute`, in that step's table's space.
///
/// Same mechanism, same reason it needs its own test: a step is a `Query`
/// against one table and reaches `plan` like any other.
#[tokio::test]
async fn a_chain_steps_own_compute_is_checked() {
    let store = seeded().await;
    let (lines, orders) = (lines(), orders());
    let tables = [&lines, &orders];
    let txn = store.begin().await.unwrap();

    // orders.shipping is scale 2 and orders.levy is scale 4, both named in the
    // orders table's own ordinals because that is the space a step's query is
    // written in.
    let chain =
        Chain::from(Query::all()).join(JoinStep::equating(col("order_id"), Ordinal(0)).query(
            Query::all().computing([Scalar::column(Ordinal(1)) + Scalar::column(Ordinal(2))]),
        ));
    let error = txn
        .chain(&context(), &tables, &chain)
        .await
        .expect_err("a step's compute is checked in that step's own space")
        .to_string();
    assert!(error.contains("different scales"), "{error}");
    assert!(
        error.contains("orders"),
        "and names the step's table: {error}"
    );
}

/// A *side* of a join has its own `Query::compute`, in that table's own space.
///
/// It is checked because a side is planned through the same `plan` a
/// single-table read is. Two explicit calls in `Join::validate` were written
/// for this first, on the belief that a side is planned rather than executed;
/// removing them left this test green, which is what said they were dead.
#[tokio::test]
async fn a_join_sides_own_compute_is_checked() {
    let store = seeded().await;
    let (lines, orders) = (lines(), orders());
    let txn = store.begin().await.unwrap();
    let join = Join::on([JoinKey::new(col("order_id"), Ordinal(0))])
        .left(Query::all().computing([Scalar::column(col("price")) + Scalar::column(col("rate"))]));
    let error = txn
        .join(&context(), &lines, &orders, &join)
        .await
        .expect_err("a side's compute is checked in that side's own space")
        .to_string();
    assert!(error.contains("different scales"), "{error}");
}

/// `explain` refuses what `execute` refuses — the check is in `plan`.
///
/// This is why the check moved out of `Reads::execute`. With it there,
/// `explain` on the same query returned a plan, and a caller reading the plan
/// would have had every reason to think the query would run. A plan for a
/// query that cannot run is worse than an error, because it looks like an
/// answer.
#[tokio::test]
async fn explaining_an_inexpressible_decimal_is_refused_too() {
    let store = seeded().await;
    let table = lines();
    let txn = store.begin().await.unwrap();
    let query =
        Query::all().computing([Scalar::column(col("price")) + Scalar::column(col("rate"))]);
    let error = txn
        .explain(&SecurityContext::superuser(), &table, &query)
        .expect_err("explain plans the same query and refuses the same way")
        .to_string();
    assert!(error.contains("different scales"), "{error}");
}

/// And `explain_join`, which is where a side's own compute is first seen.
#[tokio::test]
async fn explaining_a_join_with_a_bad_side_compute_is_refused() {
    let store = seeded().await;
    let (lines, orders) = (lines(), orders());
    let txn = store.begin().await.unwrap();
    let join = Join::on([JoinKey::new(col("order_id"), Ordinal(0))])
        .left(Query::all().computing([Scalar::column(col("price")) + Scalar::column(col("rate"))]));
    let error = txn
        .explain_join(&SecurityContext::superuser(), &lines, &orders, &join)
        .expect_err("a side is planned, and planning is where the check is")
        .to_string();
    assert!(error.contains("different scales"), "{error}");
}

/// **A defect, demonstrated before it was fixed:** `price * Decimal(3)` planned
/// cleanly and computed `Null` on every row.
///
/// `Mul` and `Div` asked only whether each side *had* a scale, and a decimal
/// literal has none — so `(Some(2), None)` read as "money times a plain
/// number", which is the expressible case. The evaluator then saw
/// `Units × Units`, which `decimal_op` has no arm for, and returned `Null`.
/// Plan-time yes, run-time null: the exact shape the plan-time check exists to
/// prevent, in the one place it was not asked.
///
/// `Add`/`Sub` never had it because they consult `mentions_decimal`, which is
/// what tells a decimal literal from an absent scale. `Mul`/`Div` now do too.
#[tokio::test]
async fn money_times_a_decimal_literal_is_refused_not_nulled() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::column(col("price")) * Scalar::literal(Value::Decimal(3)),
    )
    .await;
    assert!(error.contains("multiplying"), "{error}");
    assert!(
        error.contains("whole number"),
        "and says what to write instead: {error}"
    );
}

/// The same hole on the other operator.
#[tokio::test]
async fn money_divided_by_a_decimal_literal_is_refused_not_nulled() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::column(col("price")) / Scalar::literal(Value::Decimal(4)),
    )
    .await;
    assert!(error.contains("dividing"), "{error}");
}

/// And two decimal literals multiplied, which had the same shape with neither
/// side carrying a scale.
#[tokio::test]
async fn two_decimal_literals_multiplied_are_refused() {
    let store = seeded().await;
    let error = refusal(
        &store,
        Scalar::literal(Value::Decimal(300)) * Scalar::literal(Value::Decimal(400)),
    )
    .await;
    assert!(error.contains("multiplying"), "{error}");
}
