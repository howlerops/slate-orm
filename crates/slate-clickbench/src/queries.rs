//! The ClickBench queries, as far as this engine can express them.
//!
//! Twenty-four of the forty-three. The other nineteen need something that is
//! genuinely not built — [`UNSUPPORTED`] says which and why for each, because
//! "we ran the ones we could" is only honest if the ones we could not are
//! named.

use slate_kernel::{
    Aggregate, CmpOp, Expr, Group, KernelError, Query, RecordTransaction, SecurityContext, SortKey,
};
use slate_schema::{Ordinal, TableDef};
use slate_tuple::Value;

use crate::schema::col;

/// What a query produced, and how.
pub(crate) struct Outcome {
    /// Rows the caller would see.
    pub(crate) rows: usize,
    /// The access path, from `EXPLAIN`.
    pub(crate) plan: String,
}

/// A ClickBench query this engine can run.
pub(crate) struct Runnable {
    /// Its number in the official set.
    pub(crate) number: u32,
    /// The SQL, verbatim from ClickBench. Printed with the results so the
    /// table says what was actually run rather than only which number.
    pub(crate) sql: &'static str,
    /// Anything about how it is expressed here that differs from the SQL.
    pub(crate) note: Option<&'static str>,
}

/// A ClickBench query this engine cannot run, and what it would need.
pub(crate) struct Unsupported {
    /// Its number in the official set.
    pub(crate) number: u32,
    /// What is missing.
    pub(crate) needs: &'static str,
}

/// The nineteen that do not run, and why.
pub(crate) const UNSUPPORTED: &[Unsupported] = &[
    Unsupported {
        number: 5,
        needs: "COUNT(DISTINCT)",
    },
    Unsupported {
        number: 6,
        needs: "COUNT(DISTINCT)",
    },
    Unsupported {
        number: 9,
        needs: "COUNT(DISTINCT)",
    },
    Unsupported {
        number: 10,
        needs: "COUNT(DISTINCT)",
    },
    Unsupported {
        number: 11,
        needs: "COUNT(DISTINCT)",
    },
    Unsupported {
        number: 12,
        needs: "COUNT(DISTINCT)",
    },
    Unsupported {
        number: 14,
        needs: "COUNT(DISTINCT)",
    },
    Unsupported {
        number: 19,
        needs: "extract(minute FROM …): no expressions in GROUP BY",
    },
    Unsupported {
        number: 21,
        needs: "LIKE",
    },
    Unsupported {
        number: 22,
        needs: "LIKE",
    },
    Unsupported {
        number: 23,
        needs: "LIKE, COUNT(DISTINCT)",
    },
    Unsupported {
        number: 24,
        needs: "LIKE",
    },
    Unsupported {
        number: 28,
        needs: "length(), HAVING",
    },
    Unsupported {
        number: 29,
        needs: "REGEXP_REPLACE, length(), HAVING",
    },
    Unsupported {
        number: 30,
        needs: "arithmetic inside an aggregate",
    },
    Unsupported {
        number: 35,
        needs: "a literal as a grouping key",
    },
    Unsupported {
        number: 36,
        needs: "arithmetic in GROUP BY",
    },
    Unsupported {
        number: 40,
        needs: "CASE WHEN",
    },
    Unsupported {
        number: 43,
        needs: "DATE_TRUNC: no expressions in GROUP BY",
    },
];

fn s(text: &str) -> Value {
    Value::Str(text.to_owned())
}

fn not_empty(name: &str) -> Expr {
    Expr::compare(col(name), CmpOp::Ne, s(""))
}

/// `CounterID = 62 AND EventDate BETWEEN … AND IsRefresh = 0`, the shared
/// preamble of the last several queries. EventDate is days since the epoch,
/// which is how ClickBench stores it.
fn july_2013(counter: i64) -> Expr {
    Expr::eq(col("CounterID"), Value::I64(counter))
        .and(Expr::compare(
            col("EventDate"),
            CmpOp::Ge,
            Value::I64(15887),
        ))
        .and(Expr::compare(
            col("EventDate"),
            CmpOp::Le,
            Value::I64(15917),
        ))
}

/// Sort groups by an aggregate and keep the first `limit`.
///
/// Done here rather than in the kernel because `group_by` orders by the
/// grouping key and cannot order by what it computed. That is a real gap, and
/// doing it in the harness is the honest way to run the query while the gap
/// exists — the sort is over the groups, which are few, not over the rows.
fn top_by(mut groups: Vec<Group>, at: usize, limit: usize) -> usize {
    groups.sort_by(|a, b| {
        let key = |g: &Group| match g.values.get(at) {
            Some(Value::U64(n)) => *n as f64,
            Some(Value::I64(n)) => *n as f64,
            Some(Value::F64(n)) => *n,
            _ => 0.0,
        };
        key(b).total_cmp(&key(a))
    });
    groups.len().min(limit)
}

/// Every query that runs, in ClickBench's numbering.
#[must_use]
pub(crate) fn runnable() -> Vec<Runnable> {
    vec![
        Runnable {
            number: 1,
            sql: "SELECT COUNT(*) FROM hits",
            note: None,
        },
        Runnable {
            number: 2,
            sql: "SELECT COUNT(*) FROM hits WHERE AdvEngineID <> 0",
            note: None,
        },
        Runnable {
            number: 3,
            sql: "SELECT SUM(AdvEngineID), COUNT(*), AVG(ResolutionWidth) FROM hits",
            note: None,
        },
        Runnable {
            number: 4,
            sql: "SELECT AVG(UserID) FROM hits",
            note: None,
        },
        Runnable {
            number: 7,
            sql: "SELECT MIN(EventDate), MAX(EventDate) FROM hits",
            note: None,
        },
        Runnable {
            number: 8,
            sql: "SELECT AdvEngineID, COUNT(*) FROM hits WHERE AdvEngineID <> 0 GROUP BY AdvEngineID ORDER BY COUNT(*) DESC",
            note: Some("ORDER BY on the aggregate is done over the groups, outside the kernel"),
        },
        Runnable {
            number: 13,
            sql: "SELECT SearchPhrase, COUNT(*) AS c FROM hits WHERE SearchPhrase <> '' GROUP BY SearchPhrase ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 15,
            sql: "SELECT SearchEngineID, SearchPhrase, COUNT(*) AS c FROM hits WHERE SearchPhrase <> '' GROUP BY SearchEngineID, SearchPhrase ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 16,
            sql: "SELECT UserID, COUNT(*) FROM hits GROUP BY UserID ORDER BY COUNT(*) DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 17,
            sql: "SELECT UserID, SearchPhrase, COUNT(*) FROM hits GROUP BY UserID, SearchPhrase ORDER BY COUNT(*) DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 18,
            sql: "SELECT UserID, SearchPhrase, COUNT(*) FROM hits GROUP BY UserID, SearchPhrase LIMIT 10",
            note: None,
        },
        Runnable {
            number: 20,
            sql: "SELECT UserID FROM hits WHERE UserID = 435090932899640449",
            note: None,
        },
        Runnable {
            number: 25,
            sql: "SELECT SearchPhrase FROM hits WHERE SearchPhrase <> '' ORDER BY EventTime LIMIT 10",
            note: None,
        },
        Runnable {
            number: 26,
            sql: "SELECT SearchPhrase FROM hits WHERE SearchPhrase <> '' ORDER BY SearchPhrase LIMIT 10",
            note: None,
        },
        Runnable {
            number: 27,
            sql: "SELECT SearchPhrase FROM hits WHERE SearchPhrase <> '' ORDER BY EventTime, SearchPhrase LIMIT 10",
            note: None,
        },
        Runnable {
            number: 31,
            sql: "SELECT SearchEngineID, ClientIP, COUNT(*) AS c, SUM(IsRefresh), AVG(ResolutionWidth) FROM hits WHERE SearchPhrase <> '' GROUP BY SearchEngineID, ClientIP ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 32,
            sql: "SELECT WatchID, ClientIP, COUNT(*) AS c, SUM(IsRefresh), AVG(ResolutionWidth) FROM hits WHERE SearchPhrase <> '' GROUP BY WatchID, ClientIP ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 33,
            sql: "SELECT WatchID, ClientIP, COUNT(*) AS c, SUM(IsRefresh), AVG(ResolutionWidth) FROM hits GROUP BY WatchID, ClientIP ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 34,
            sql: "SELECT URL, COUNT(*) AS c FROM hits GROUP BY URL ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 37,
            sql: "SELECT URL, COUNT(*) AS PageViews FROM hits WHERE CounterID = 62 AND EventDate >= '2013-07-01' AND EventDate <= '2013-07-31' AND DontCountHits = 0 AND IsRefresh = 0 AND URL <> '' GROUP BY URL ORDER BY PageViews DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 38,
            sql: "SELECT Title, COUNT(*) AS PageViews FROM hits WHERE CounterID = 62 AND … AND Title <> '' GROUP BY Title ORDER BY PageViews DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 39,
            sql: "SELECT URL, COUNT(*) AS PageViews FROM hits WHERE CounterID = 62 AND … AND IsLink <> 0 AND IsDownload = 0 GROUP BY URL ORDER BY PageViews DESC LIMIT 10 OFFSET 1000",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 41,
            sql: "SELECT URLHash, EventDate, COUNT(*) FROM hits WHERE CounterID = 62 AND … AND TraficSourceID IN (-1, 6) AND RefererHash = 3594120000172545465 GROUP BY URLHash, EventDate ORDER BY PageViews DESC LIMIT 10 OFFSET 100",
            note: Some("exercises the IN path; ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 42,
            sql: "SELECT WindowClientWidth, WindowClientHeight, COUNT(*) FROM hits WHERE CounterID = 62 AND … AND URLHash = 2868770270353813622 GROUP BY WindowClientWidth, WindowClientHeight ORDER BY PageViews DESC LIMIT 10 OFFSET 10000",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
    ]
}

type Txn<'a> = RecordTransaction<'a>;

/// Run one query by number.
pub(crate) async fn run(
    txn: &Txn<'_>,
    ctx: &SecurityContext,
    table: &TableDef,
    number: u32,
) -> Result<Outcome, KernelError> {
    let plan_of = |query: &Query| {
        txn.explain(ctx, table, query)
            .map(|e| e.access.to_string())
            .unwrap_or_else(|_| "?".to_owned())
    };

    // A grouped query, its filter, keys and aggregates.
    async fn grouped(
        txn: &Txn<'_>,
        ctx: &SecurityContext,
        table: &TableDef,
        filter: Expr,
        keys: &[Ordinal],
        aggregates: &[Aggregate],
        sort_by: Option<usize>,
        limit: usize,
        offset: usize,
    ) -> Result<Outcome, KernelError> {
        let query = Query::all().filter(filter);
        let plan = txn
            .explain(ctx, table, &query)
            .map(|e| e.access.to_string())
            .unwrap_or_else(|_| "?".to_owned());
        let groups = txn.group_by(ctx, table, &query, keys, aggregates).await?;
        let rows = match sort_by {
            Some(at) => top_by(groups, at, limit.saturating_add(offset))
                .saturating_sub(offset)
                .min(limit),
            None => groups.len().min(limit),
        };
        Ok(Outcome { rows, plan })
    }

    let count_star = [Aggregate::Count];

    match number {
        1 => {
            let query = Query::all().count_only();
            let plan = plan_of(&query);
            let n = txn.count(ctx, table, &query).await?;
            Ok(Outcome {
                rows: usize::try_from(n).unwrap_or(usize::MAX).min(1),
                plan,
            })
        }
        2 => {
            let query = Query::all()
                .filter(Expr::compare(col("AdvEngineID"), CmpOp::Ne, Value::I64(0)))
                .count_only();
            let plan = plan_of(&query);
            txn.count(ctx, table, &query).await?;
            Ok(Outcome { rows: 1, plan })
        }
        3 => {
            let query = Query::all();
            let plan = plan_of(&query);
            txn.aggregate(
                ctx,
                table,
                &query,
                &[
                    Aggregate::Sum(col("AdvEngineID")),
                    Aggregate::Count,
                    Aggregate::Avg(col("ResolutionWidth")),
                ],
            )
            .await?;
            Ok(Outcome { rows: 1, plan })
        }
        4 => {
            let query = Query::all();
            let plan = plan_of(&query);
            txn.aggregate(ctx, table, &query, &[Aggregate::Avg(col("UserID"))])
                .await?;
            Ok(Outcome { rows: 1, plan })
        }
        7 => {
            let query = Query::all();
            let plan = plan_of(&query);
            txn.aggregate(
                ctx,
                table,
                &query,
                &[
                    Aggregate::Min(col("EventDate")),
                    Aggregate::Max(col("EventDate")),
                ],
            )
            .await?;
            Ok(Outcome { rows: 1, plan })
        }
        8 => {
            grouped(
                txn,
                ctx,
                table,
                Expr::compare(col("AdvEngineID"), CmpOp::Ne, Value::I64(0)),
                &[col("AdvEngineID")],
                &count_star,
                Some(0),
                usize::MAX,
                0,
            )
            .await
        }
        13 => {
            grouped(
                txn,
                ctx,
                table,
                not_empty("SearchPhrase"),
                &[col("SearchPhrase")],
                &count_star,
                Some(0),
                10,
                0,
            )
            .await
        }
        15 => {
            grouped(
                txn,
                ctx,
                table,
                not_empty("SearchPhrase"),
                &[col("SearchEngineID"), col("SearchPhrase")],
                &count_star,
                Some(0),
                10,
                0,
            )
            .await
        }
        16 => {
            grouped(
                txn,
                ctx,
                table,
                Expr::True,
                &[col("UserID")],
                &count_star,
                Some(0),
                10,
                0,
            )
            .await
        }
        17 => {
            grouped(
                txn,
                ctx,
                table,
                Expr::True,
                &[col("UserID"), col("SearchPhrase")],
                &count_star,
                Some(0),
                10,
                0,
            )
            .await
        }
        18 => {
            grouped(
                txn,
                ctx,
                table,
                Expr::True,
                &[col("UserID"), col("SearchPhrase")],
                &count_star,
                None,
                10,
                0,
            )
            .await
        }
        20 => {
            let query = Query::all()
                .filter(Expr::eq(col("UserID"), Value::I64(435_090_932_899_640_449)))
                .select([col("UserID")]);
            let plan = plan_of(&query);
            let rows: usize = txn.execute(ctx, table, &query).await?.count().await?;
            Ok(Outcome { rows, plan })
        }
        25..=27 => {
            let sort: Vec<SortKey> = match number {
                25 => vec![SortKey::asc(col("EventTime"))],
                26 => vec![SortKey::asc(col("SearchPhrase"))],
                _ => vec![
                    SortKey::asc(col("EventTime")),
                    SortKey::asc(col("SearchPhrase")),
                ],
            };
            // The query selects one column. Saying so matters: with
            // `Projection::All` the executor decodes all 105 columns of every
            // surviving row, which is most of what these queries cost.
            let query = Query::all()
                .filter(not_empty("SearchPhrase"))
                .select([col("SearchPhrase")])
                .sort_by(sort)
                .limit(10);
            let plan = plan_of(&query);
            let rows: usize = txn.execute(ctx, table, &query).await?.count().await?;
            Ok(Outcome { rows, plan })
        }
        31..=33 => {
            let keys = if number == 31 {
                vec![col("SearchEngineID"), col("ClientIP")]
            } else {
                vec![col("WatchID"), col("ClientIP")]
            };
            let filter = if number == 33 {
                Expr::True
            } else {
                not_empty("SearchPhrase")
            };
            grouped(
                txn,
                ctx,
                table,
                filter,
                &keys,
                &[
                    Aggregate::Count,
                    Aggregate::Sum(col("IsRefresh")),
                    Aggregate::Avg(col("ResolutionWidth")),
                ],
                Some(0),
                10,
                0,
            )
            .await
        }
        34 => {
            grouped(
                txn,
                ctx,
                table,
                Expr::True,
                &[col("URL")],
                &count_star,
                Some(0),
                10,
                0,
            )
            .await
        }
        37..=38 => {
            let (key, present) = if number == 37 {
                (col("URL"), "URL")
            } else {
                (col("Title"), "Title")
            };
            grouped(
                txn,
                ctx,
                table,
                july_2013(62)
                    .and(Expr::eq(col("DontCountHits"), Value::I64(0)))
                    .and(Expr::eq(col("IsRefresh"), Value::I64(0)))
                    .and(not_empty(present)),
                &[key],
                &count_star,
                Some(0),
                10,
                0,
            )
            .await
        }
        39 => {
            grouped(
                txn,
                ctx,
                table,
                july_2013(62)
                    .and(Expr::eq(col("IsRefresh"), Value::I64(0)))
                    .and(Expr::compare(col("IsLink"), CmpOp::Ne, Value::I64(0)))
                    .and(Expr::eq(col("IsDownload"), Value::I64(0))),
                &[col("URL")],
                &count_star,
                Some(0),
                10,
                1_000,
            )
            .await
        }
        41 => {
            grouped(
                txn,
                ctx,
                table,
                july_2013(62)
                    .and(Expr::eq(col("IsRefresh"), Value::I64(0)))
                    .and(Expr::In {
                        column: col("TraficSourceID"),
                        values: vec![Value::I64(-1), Value::I64(6)],
                    })
                    .and(Expr::eq(
                        col("RefererHash"),
                        Value::I64(3_594_120_000_172_545_465),
                    )),
                &[col("URLHash"), col("EventDate")],
                &count_star,
                Some(0),
                10,
                100,
            )
            .await
        }
        42 => {
            grouped(
                txn,
                ctx,
                table,
                july_2013(62)
                    .and(Expr::eq(col("IsRefresh"), Value::I64(0)))
                    .and(Expr::eq(col("DontCountHits"), Value::I64(0)))
                    .and(Expr::eq(
                        col("URLHash"),
                        Value::I64(2_868_770_270_353_813_622),
                    )),
                &[col("WindowClientWidth"), col("WindowClientHeight")],
                &count_star,
                Some(0),
                10,
                10_000,
            )
            .await
        }
        other => panic!("query {other} is not in the runnable set"),
    }
}
