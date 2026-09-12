//! Every plan the cost model picks, in one file.
//!
//! Postgres' regression suite works substantially by diffing `EXPLAIN` output,
//! and pgrust's account of reimplementing it — where the planner was "the most
//! complicated part" and plan-shape divergence broke an entire attempt —  is a
//! reminder of why: a planner's *decisions* are its behaviour, and a change to
//! a cost constant is a change to all of them at once.
//!
//! This project learned the same thing the expensive way. Recalibrating the
//! cost model against real object storage was correct and necessary, and it
//! broke seven tests — discovered one at a time, over seven build-and-run
//! cycles, each looking like an isolated surprise rather than one deliberate
//! change with a wide blast radius. The information needed to review it at
//! once existed; nothing collected it.
//!
//! So this collects it. One corpus of query shapes against fixed statistics,
//! rendered as text, compared against a committed snapshot. A cost-model change
//! then shows up as a single reviewable diff: every decision that moved, and
//! every decision that did not, side by side.
//!
//! One limitation the snapshot makes visible rather than hides: at a thousand
//! rows a `Point Get` is chosen at cost 3.00 while a table scan is costed at
//! 1.12. That is not the planner ignoring its own model — a full primary-key
//! equality becomes a point get in place of the key-range candidate, so no
//! whole-table scan is generated to compete with it. It is also the right
//! answer: the cost unit counts *requests*, and at small sizes requests are not
//! the whole story — scanning a thousand rows to return one moves a thousand
//! rows' worth of bytes and decodes them. The unit earns its keep when requests
//! dominate, which is what large tables do.
//!
//! The snapshot is not an assertion that these plans are *right* — no snapshot
//! can be. `oracle.rs` says the plans return the same rows; `cost_calibration`
//! says what they really cost. This says only that a change to any of them was
//! deliberate, which is the thing that was missing.
//!
//! To update after an intended change:
//!
//! ```sh
//! UPDATE_PLAN_SNAPSHOT=1 cargo test -p slate-kernel --test plan_snapshots
//! ```
//!
//! and read the diff before committing it.

// Tests assert exact outcomes and are meant to panic when one is wrong.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use slate_kernel::{CmpOp, ColumnStats, Explanation, Expr, Query, SortKey, TableStats, plan_with};
use slate_schema::{IndexDef, IndexId, Ordinal, TableDef, TableId};
use slate_tuple::{Direction, Value, ValueType};

const EVENTS: TableId = TableId(1);
const SNAPSHOT: &str = include_str!("snapshots/plans.txt");

fn events() -> TableDef {
    TableDef::builder("events", EVENTS)
        .column("id", ValueType::U64)
        .column("kind", ValueType::Str)
        .column("weight", ValueType::I64)
        .nullable_column("note", ValueType::Str)
        .primary_key(["id"])
        .index(IndexDef::builder("by_kind", IndexId(10)).column("kind"))
        .index(IndexDef::builder("by_weight", IndexId(11)).column_with("weight", Direction::Desc))
        // Covering: holds everything a `kind`-only query reads, so it never
        // pays a point read and sits on the other side of the crossover from
        // every other index here.
        .index(
            IndexDef::builder("by_kind_weight", IndexId(12))
                .column("kind")
                .column("weight"),
        )
        .build()
        .expect("valid schema")
}

fn col(name: &str) -> Ordinal {
    events().ordinal_of(name).expect("column exists")
}

/// Statistics at three sizes, because the interesting decisions are the ones
/// that change with the table.
///
/// `kind` is given a distinct count that makes an equality select a small
/// *absolute* number of rows at every size — which, since the recalibration, is
/// what decides whether an index is worth its point reads.
fn stats(rows: u64) -> TableStats {
    TableStats::with_row_count(rows)
        .with_column(
            col("kind"),
            ColumnStats {
                distinct: rows / 10,
                null_fraction: 0.0,
            },
        )
        .with_column(
            col("weight"),
            ColumnStats {
                distinct: 100,
                null_fraction: 0.0,
            },
        )
}

/// The shapes worth watching: each is a place the cost model makes a choice.
fn corpus() -> Vec<(&'static str, Query)> {
    vec![
        ("everything", Query::all()),
        (
            "primary key equality",
            Query::all().filter(Expr::eq(col("id"), Value::U64(42))),
        ),
        (
            "primary key range",
            Query::all().filter(
                Expr::compare(col("id"), CmpOp::Ge, Value::U64(100)).and(Expr::compare(
                    col("id"),
                    CmpOp::Lt,
                    Value::U64(200),
                )),
            ),
        ),
        (
            "IN over the key",
            Query::all().filter(Expr::In {
                column: col("id"),
                values: (1..=4).map(Value::U64).collect(),
            }),
        ),
        (
            "IN over the key, large set",
            Query::all().filter(Expr::In {
                column: col("id"),
                values: (1..=2000).map(Value::U64).collect(),
            }),
        ),
        (
            "indexed equality",
            Query::all().filter(Expr::eq(col("kind"), Value::Str("k".into()))),
        ),
        (
            "indexed equality, projecting only indexed columns",
            Query::all()
                .filter(Expr::eq(col("kind"), Value::Str("k".into())))
                .select([col("kind"), col("weight")]),
        ),
        (
            "indexed range on a descending index",
            Query::all().filter(Expr::compare(col("weight"), CmpOp::Gt, Value::I64(50))),
        ),
        (
            "anchored LIKE",
            Query::all().filter(Expr::like(col("kind"), "pre%")),
        ),
        (
            "unanchored LIKE",
            Query::all().filter(Expr::like(col("kind"), "%mid%")),
        ),
        (
            "two columns conjoined",
            Query::all().filter(
                Expr::eq(col("kind"), Value::Str("k".into())).and(Expr::compare(
                    col("weight"),
                    CmpOp::Gt,
                    Value::I64(10),
                )),
            ),
        ),
        (
            "a disjunction, which no single bound serves",
            Query::all().filter(Expr::Or(vec![
                Expr::eq(col("kind"), Value::Str("k".into())),
                Expr::compare(col("weight"), CmpOp::Lt, Value::I64(5)),
            ])),
        ),
        (
            "contradictory bounds",
            Query::all().filter(
                Expr::compare(col("id"), CmpOp::Gt, Value::U64(100)).and(Expr::compare(
                    col("id"),
                    CmpOp::Lt,
                    Value::U64(10),
                )),
            ),
        ),
        (
            "sorted by an indexed column",
            Query::all().sort_by([SortKey::asc(col("weight"))]),
        ),
        (
            "sorted by an unindexed column",
            Query::all().sort_by([SortKey::asc(col("note"))]),
        ),
        (
            "top ten by an indexed column",
            Query::all()
                .sort_by([SortKey::desc(col("weight"))])
                .limit(10),
        ),
        (
            "indexed equality with a small limit",
            Query::all()
                .filter(Expr::eq(col("kind"), Value::Str("k".into())))
                .limit(3),
        ),
        (
            "is null on a nullable column",
            Query::all().filter(Expr::is_null(col("note"))),
        ),
    ]
}

/// Render every plan in the corpus, at every table size.
fn render() -> String {
    let table = events();
    let mut out = String::new();
    out.push_str(
        "# Plans the cost model picks, by table size.\n\
         #\n\
         # Generated by `plan_snapshots.rs`; see that file before editing.\n\
         # Regenerate with UPDATE_PLAN_SNAPSHOT=1 and review the diff.\n",
    );

    for rows in [1_000u64, 1_000_000, 1_000_000_000] {
        let stats = stats(rows);
        out.push_str(&format!("\n## {rows} rows\n\n"));
        for (name, query) in corpus() {
            let plan = plan_with(
                &table,
                &query.filter,
                query.order,
                &query.projection,
                &stats,
                query.limit,
            );
            let explained = Explanation::of(&table, &plan, &query);
            out.push_str(&format!("{name}\n    {explained}\n"));
        }
    }
    out
}

/// The plans have not changed unless someone meant them to.
#[test]
fn plans_match_the_committed_snapshot() {
    let rendered = render();

    if std::env::var("UPDATE_PLAN_SNAPSHOT").is_ok() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots/plans.txt");
        std::fs::write(path, &rendered).expect("write the snapshot");
        eprintln!("snapshot rewritten at {path}; review the diff before committing");
        return;
    }

    if rendered == SNAPSHOT {
        return;
    }

    // Show the whole blast radius at once, which is the point of the file.
    let mut differences = Vec::new();
    for (line, (got, want)) in rendered.lines().zip(SNAPSHOT.lines()).enumerate() {
        if got != want {
            differences.push(format!(
                "  line {}:\n    was: {want}\n    now: {got}",
                line + 1
            ));
        }
    }
    let (a, b) = (rendered.lines().count(), SNAPSHOT.lines().count());
    if a != b {
        differences.push(format!(
            "  the snapshot has {b} lines and the plans render {a}"
        ));
    }

    panic!(
        "{} plan(s) changed:\n{}\n\n\
         If the change was intended, regenerate with:\n  \
         UPDATE_PLAN_SNAPSHOT=1 cargo test -p slate-kernel --test plan_snapshots\n\
         and read the diff. A cost-model change should move the plans it meant \
         to and no others.",
        differences.len(),
        differences.join("\n")
    );
}

/// The corpus has to exercise more than one decision.
///
/// A snapshot where every query picks a table scan would be perfectly stable
/// and would notice nothing.
#[test]
fn the_corpus_produces_a_variety_of_plans() {
    let rendered = render();
    for expected in ["Point Get", "Index Only Scan", "Index Scan", "Table Scan"] {
        assert!(
            rendered.contains(expected),
            "no query in the corpus produced a {expected}, so the snapshot \
             would not notice that decision changing"
        );
    }
}
