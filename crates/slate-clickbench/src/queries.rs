//! The ClickBench queries, as far as this engine can express them.
//!
//! Twenty-four of the forty-three. The other nineteen need something that is
//! genuinely not built — [`UNSUPPORTED`] says which and why for each, because
//! "we ran the ones we could" is only honest if the ones we could not are
//! named.

use slate_kernel::{
    Aggregate, CmpOp, Expr, Group, KernelError, Query, RecordTransaction, Scalar, SecurityContext,
    SortKey, TimeUnit,
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
    /// What the query actually computed, abbreviated.
    ///
    /// Printed because a benchmark that only reports timings cannot be
    /// checked. A wrong answer produced quickly is the easiest kind of
    /// benchmark result to get.
    pub(crate) answer: String,
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

/// The one that still does not run.
///
/// `REGEXP_REPLACE` needs a regular-expression engine, which is a dependency
/// rather than a feature of this layer, and one query is not a reason to take
/// one on. Everything else the other eighteen needed — scalar expressions,
/// `HAVING`, `COUNT(DISTINCT)`, `LIKE` — is built.
pub(crate) const UNSUPPORTED: &[Unsupported] = &[Unsupported {
    number: 29,
    needs: "REGEXP_REPLACE, length(), HAVING",
}];

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
fn top_by(mut groups: Vec<Group>, at: usize, limit: usize) -> (usize, String) {
    groups.sort_by(|a, b| {
        let key = |g: &Group| match g.values.get(at) {
            Some(Value::U64(n)) => *n as f64,
            Some(Value::I64(n)) => *n as f64,
            Some(Value::F64(n)) => *n,
            _ => 0.0,
        };
        key(b).total_cmp(&key(a))
    });
    let best = groups
        .first()
        .map_or_else(String::new, |g| describe(&g.values));
    (groups.len().min(limit), best)
}

/// A short rendering of some values, for the results table.
fn describe(values: &[Value]) -> String {
    values
        .iter()
        .map(|v| match v {
            Value::Str(s) if s.len() > 18 => format!("{}…", &s[..18]),
            other => format!("{other:?}")
                .replace("I64(", "")
                .replace("U64(", "")
                .replace("F64(", "")
                .replace("Str(", "")
                .replace(')', ""),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every query that runs, in ClickBench's numbering.
#[must_use]
pub(crate) fn runnable() -> Vec<Runnable> {
    let mut all = vec![
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
        Runnable {
            number: 5,
            sql: "SELECT COUNT(DISTINCT UserID) FROM hits",
            note: None,
        },
        Runnable {
            number: 6,
            sql: "SELECT COUNT(DISTINCT SearchPhrase) FROM hits",
            note: None,
        },
        Runnable {
            number: 9,
            sql: "SELECT RegionID, COUNT(DISTINCT UserID) AS u FROM hits GROUP BY RegionID ORDER BY u DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 10,
            sql: "SELECT RegionID, SUM(AdvEngineID), COUNT(*) AS c, AVG(ResolutionWidth), COUNT(DISTINCT UserID) FROM hits GROUP BY RegionID ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 11,
            sql: "SELECT MobilePhoneModel, COUNT(DISTINCT UserID) AS u FROM hits WHERE MobilePhoneModel <> '' GROUP BY MobilePhoneModel ORDER BY u DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 12,
            sql: "SELECT MobilePhone, MobilePhoneModel, COUNT(DISTINCT UserID) AS u FROM hits WHERE MobilePhoneModel <> '' GROUP BY MobilePhone, MobilePhoneModel ORDER BY u DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 14,
            sql: "SELECT SearchPhrase, COUNT(DISTINCT UserID) AS u FROM hits WHERE SearchPhrase <> '' GROUP BY SearchPhrase ORDER BY u DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 21,
            sql: "SELECT COUNT(*) FROM hits WHERE URL LIKE '%google%'",
            note: None,
        },
        Runnable {
            number: 22,
            sql: "SELECT SearchPhrase, MIN(URL), COUNT(*) AS c FROM hits WHERE URL LIKE '%google%' AND SearchPhrase <> '' GROUP BY SearchPhrase ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 23,
            sql: "SELECT SearchPhrase, MIN(URL), MIN(Title), COUNT(*) AS c, COUNT(DISTINCT UserID) FROM hits WHERE Title LIKE '%Google%' AND URL NOT LIKE '%.google.%' AND SearchPhrase <> '' GROUP BY SearchPhrase ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 24,
            sql: "SELECT * FROM hits WHERE URL LIKE '%google%' ORDER BY EventTime LIMIT 10",
            note: Some("SELECT * really does decode all 105 columns, unlike Q25-27"),
        },
        Runnable {
            number: 19,
            sql: "SELECT UserID, extract(minute FROM EventTime) AS m, SearchPhrase, COUNT(*) FROM hits GROUP BY UserID, m, SearchPhrase ORDER BY COUNT(*) DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 28,
            sql: "SELECT CounterID, AVG(length(URL)) AS l, COUNT(*) AS c FROM hits WHERE URL <> '' GROUP BY CounterID HAVING COUNT(*) > 100000 ORDER BY l DESC LIMIT 25",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 30,
            sql: "SELECT SUM(ResolutionWidth), SUM(ResolutionWidth + 1), … SUM(ResolutionWidth + 89) FROM hits",
            note: Some("all 90 sums, over 90 computed columns"),
        },
        Runnable {
            number: 35,
            sql: "SELECT 1, URL, COUNT(*) AS c FROM hits GROUP BY 1, URL ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 36,
            sql: "SELECT ClientIP, ClientIP - 1, ClientIP - 2, ClientIP - 3, COUNT(*) AS c FROM hits GROUP BY ClientIP, ClientIP - 1, ClientIP - 2, ClientIP - 3 ORDER BY c DESC LIMIT 10",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 40,
            sql: "SELECT TraficSourceID, SearchEngineID, AdvEngineID, CASE WHEN (SearchEngineID = 0 AND AdvEngineID = 0) THEN Referer ELSE '' END AS Src, URL AS Dst, COUNT(*) FROM hits WHERE CounterID = 62 AND … GROUP BY … ORDER BY PageViews DESC LIMIT 10 OFFSET 1000",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
        Runnable {
            number: 43,
            sql: "SELECT DATE_TRUNC('minute', EventTime) AS M, COUNT(*) FROM hits WHERE CounterID = 62 AND … GROUP BY M ORDER BY M LIMIT 10 OFFSET 1000",
            note: None,
        },
        Runnable {
            number: 29,
            sql: "SELECT REGEXP_REPLACE(Referer, '^https?://(?:www\\.)?([^/]+)/.*$', '\\1') AS k, AVG(length(Referer)) AS l, COUNT(*) AS c, MIN(Referer) FROM hits WHERE Referer <> '' GROUP BY k HAVING COUNT(*) > 100000 ORDER BY l DESC LIMIT 25",
            note: Some("ORDER BY on the aggregate is done over the groups"),
        },
    ];
    // Sorted, so the results table reads in ClickBench's order however the
    // list happens to be maintained.
    all.sort_by_key(|r| r.number);
    all
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
        let total = groups.len();
        let (rows, answer) = match sort_by {
            Some(at) => {
                let (kept, best) = top_by(groups, at, limit.saturating_add(offset));
                (kept.saturating_sub(offset).min(limit), best)
            }
            None => (total.min(limit), String::new()),
        };
        Ok(Outcome {
            rows,
            plan,
            answer: format!(
                "{total} groups{}",
                if answer.is_empty() {
                    String::new()
                } else {
                    format!(", top {answer}")
                }
            ),
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn grouped_computing(
        txn: &Txn<'_>,
        ctx: &SecurityContext,
        table: &TableDef,
        filter: Expr,
        compute: Vec<Scalar>,
        keys: &[Ordinal],
        aggregates: &[Aggregate],
        sort_by: Option<usize>,
        limit: usize,
        offset: usize,
        having: Expr,
    ) -> Result<Outcome, KernelError> {
        let query = Query::all().filter(filter).computing(compute);
        let plan = txn
            .explain(ctx, table, &query)
            .map(|e| e.access.to_string())
            .unwrap_or_else(|_| "?".to_owned());
        let groups = txn
            .group_by_having(ctx, table, &query, keys, aggregates, &having)
            .await?;
        let total = groups.len();
        let (rows, answer) = match sort_by {
            Some(at) => {
                let (kept, best) = top_by(groups, at, limit.saturating_add(offset));
                (kept.saturating_sub(offset).min(limit), best)
            }
            None => (total.saturating_sub(offset).min(limit), String::new()),
        };
        Ok(Outcome {
            rows,
            plan,
            answer: format!(
                "{total} groups{}",
                if answer.is_empty() {
                    String::new()
                } else {
                    format!(", top {answer}")
                }
            ),
        })
    }

    let count_star = [Aggregate::Count];

    match number {
        19 => {
            let minute = Query::computed(table, 0);
            grouped_computing(
                txn,
                ctx,
                table,
                Expr::True,
                vec![Scalar::column(col("EventTime")).extract(TimeUnit::Minute)],
                &[col("UserID"), minute, col("SearchPhrase")],
                &count_star,
                Some(0),
                10,
                0,
                Expr::True,
            )
            .await
        }
        28 => {
            let length = Query::computed(table, 0);
            grouped_computing(
                txn,
                ctx,
                table,
                not_empty("URL"),
                vec![Scalar::column(col("URL")).length()],
                &[col("CounterID")],
                &[Aggregate::Avg(length), Aggregate::Count],
                Some(0),
                25,
                0,
                // HAVING COUNT(*) > 100000: one grouping column, so the count
                // is the second aggregate at ordinal 1 + 1.
                Expr::compare(Group::aggregate(1, 1), CmpOp::Gt, Value::U64(100_000)),
            )
            .await
        }
        29 => {
            let host = Query::computed(table, 0);
            let length = Query::computed(table, 1);
            grouped_computing(
                txn,
                ctx,
                table,
                not_empty("Referer"),
                vec![
                    Scalar::column(col("Referer"))
                        .regexp_replace(r"^https?://(?:www\.)?([^/]+)/.*$", r"\1"),
                    Scalar::column(col("Referer")).length(),
                ],
                &[host],
                &[
                    Aggregate::Avg(length),
                    Aggregate::Count,
                    Aggregate::Min(col("Referer")),
                ],
                Some(0),
                25,
                0,
                Expr::compare(Group::aggregate(1, 1), CmpOp::Gt, Value::U64(100_000)),
            )
            .await
        }
        30 => {
            // Ninety sums of ninety computed columns, which is the query.
            let width = col("ResolutionWidth");
            let computed: Vec<Scalar> = (0..90)
                .map(|n| Scalar::column(width) + i64::from(n))
                .collect();
            let aggregates: Vec<Aggregate> = (0..90)
                .map(|n| Aggregate::Sum(Query::computed(table, n)))
                .collect();
            let query = Query::all().computing(computed);
            let plan = txn
                .explain(ctx, table, &query)
                .map(|e| e.access.to_string())
                .unwrap_or_else(|_| "?".to_owned());
            let values = txn.aggregate(ctx, table, &query, &aggregates).await?;
            Ok(Outcome {
                rows: 1,
                plan,
                // The first two of ninety, which is enough to check: the second
                // is the first plus one per row.
                answer: describe(values.get(..2).unwrap_or(&values)),
            })
        }
        35 => {
            let one = Query::computed(table, 0);
            grouped_computing(
                txn,
                ctx,
                table,
                Expr::True,
                vec![Scalar::literal(Value::I64(1))],
                &[one, col("URL")],
                &count_star,
                Some(0),
                10,
                0,
                Expr::True,
            )
            .await
        }
        36 => {
            let ip = col("ClientIP");
            let computed: Vec<Scalar> = (1..=3).map(|n| Scalar::column(ip) - n).collect();
            let keys = vec![
                ip,
                Query::computed(table, 0),
                Query::computed(table, 1),
                Query::computed(table, 2),
            ];
            grouped_computing(
                txn,
                ctx,
                table,
                Expr::True,
                computed,
                &keys,
                &count_star,
                Some(0),
                10,
                0,
                Expr::True,
            )
            .await
        }
        40 => {
            let src = Query::computed(table, 0);
            let case = Scalar::Case {
                branches: vec![(
                    Expr::eq(col("SearchEngineID"), Value::I64(0))
                        .and(Expr::eq(col("AdvEngineID"), Value::I64(0))),
                    Scalar::column(col("Referer")),
                )],
                otherwise: Box::new(Scalar::literal(Value::Str(String::new()))),
            };
            grouped_computing(
                txn,
                ctx,
                table,
                july_2013(62).and(Expr::eq(col("IsRefresh"), Value::I64(0))),
                vec![case],
                &[
                    col("TraficSourceID"),
                    col("SearchEngineID"),
                    col("AdvEngineID"),
                    src,
                    col("URL"),
                ],
                &count_star,
                Some(0),
                10,
                1_000,
                Expr::True,
            )
            .await
        }
        43 => {
            let minute = Query::computed(table, 0);
            grouped_computing(
                txn,
                ctx,
                table,
                july_2013(62)
                    .and(Expr::eq(col("IsRefresh"), Value::I64(0)))
                    .and(Expr::eq(col("DontCountHits"), Value::I64(0))),
                vec![Scalar::column(col("EventTime")).date_trunc(TimeUnit::Minute)],
                &[minute],
                &count_star,
                // Ordered by the grouping key, which `group_by` already gives.
                None,
                10,
                1_000,
                Expr::True,
            )
            .await
        }
        1 => {
            let query = Query::all().count_only();
            let plan = plan_of(&query);
            let n = txn.count(ctx, table, &query).await?;
            Ok(Outcome {
                rows: usize::try_from(n).unwrap_or(usize::MAX).min(1),
                plan,
                answer: String::new(),
            })
        }
        2 => {
            let query = Query::all()
                .filter(Expr::compare(col("AdvEngineID"), CmpOp::Ne, Value::I64(0)))
                .count_only();
            let plan = plan_of(&query);
            let n = txn.count(ctx, table, &query).await?;
            Ok(Outcome {
                rows: 1,
                plan,
                answer: n.to_string(),
            })
        }
        3 => {
            let query = Query::all();
            let plan = plan_of(&query);
            let values = txn
                .aggregate(
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
            Ok(Outcome {
                rows: 1,
                plan,
                answer: describe(&values),
            })
        }
        4 => {
            let query = Query::all();
            let plan = plan_of(&query);
            let values = txn
                .aggregate(ctx, table, &query, &[Aggregate::Avg(col("UserID"))])
                .await?;
            Ok(Outcome {
                rows: 1,
                plan,
                answer: describe(&values),
            })
        }
        5 | 6 => {
            let column = if number == 5 {
                col("UserID")
            } else {
                col("SearchPhrase")
            };
            let query = Query::all();
            let plan = plan_of(&query);
            let values = txn
                .aggregate(ctx, table, &query, &[Aggregate::CountDistinct(column)])
                .await?;
            Ok(Outcome {
                rows: 1,
                plan,
                answer: describe(&values),
            })
        }
        7 => {
            let query = Query::all();
            let plan = plan_of(&query);
            let values = txn
                .aggregate(
                    ctx,
                    table,
                    &query,
                    &[
                        Aggregate::Min(col("EventDate")),
                        Aggregate::Max(col("EventDate")),
                    ],
                )
                .await?;
            Ok(Outcome {
                rows: 1,
                plan,
                answer: describe(&values),
            })
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
        9 => {
            grouped(
                txn,
                ctx,
                table,
                Expr::True,
                &[col("RegionID")],
                &[Aggregate::CountDistinct(col("UserID"))],
                Some(0),
                10,
                0,
            )
            .await
        }
        10 => {
            grouped(
                txn,
                ctx,
                table,
                Expr::True,
                &[col("RegionID")],
                &[
                    Aggregate::Sum(col("AdvEngineID")),
                    Aggregate::Count,
                    Aggregate::Avg(col("ResolutionWidth")),
                    Aggregate::CountDistinct(col("UserID")),
                ],
                Some(1),
                10,
                0,
            )
            .await
        }
        11 | 12 => {
            let keys = if number == 11 {
                vec![col("MobilePhoneModel")]
            } else {
                vec![col("MobilePhone"), col("MobilePhoneModel")]
            };
            grouped(
                txn,
                ctx,
                table,
                not_empty("MobilePhoneModel"),
                &keys,
                &[Aggregate::CountDistinct(col("UserID"))],
                Some(0),
                10,
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
        14 => {
            grouped(
                txn,
                ctx,
                table,
                not_empty("SearchPhrase"),
                &[col("SearchPhrase")],
                &[Aggregate::CountDistinct(col("UserID"))],
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
            Ok(Outcome {
                rows,
                plan,
                answer: String::new(),
            })
        }
        21 => {
            let query = Query::all()
                .filter(Expr::like(col("URL"), "%google%"))
                .count_only();
            let plan = plan_of(&query);
            let n = txn.count(ctx, table, &query).await?;
            Ok(Outcome {
                rows: 1,
                plan,
                answer: n.to_string(),
            })
        }
        22 => {
            grouped(
                txn,
                ctx,
                table,
                Expr::like(col("URL"), "%google%").and(not_empty("SearchPhrase")),
                &[col("SearchPhrase")],
                &[Aggregate::Min(col("URL")), Aggregate::Count],
                Some(1),
                10,
                0,
            )
            .await
        }
        23 => {
            grouped(
                txn,
                ctx,
                table,
                Expr::like(col("Title"), "%Google%")
                    .and(Expr::not_like(col("URL"), "%.google.%"))
                    .and(not_empty("SearchPhrase")),
                &[col("SearchPhrase")],
                &[
                    Aggregate::Min(col("URL")),
                    Aggregate::Min(col("Title")),
                    Aggregate::Count,
                    Aggregate::CountDistinct(col("UserID")),
                ],
                Some(2),
                10,
                0,
            )
            .await
        }
        24 => {
            // `SELECT *`, so every column really is decoded — the shape Q25-27
            // used to have by accident, kept here because the query asks for it.
            let query = Query::all()
                .filter(Expr::like(col("URL"), "%google%"))
                .sort_by([SortKey::asc(col("EventTime"))])
                .limit(10);
            let plan = plan_of(&query);
            let rows: usize = txn.execute(ctx, table, &query).await?.count().await?;
            Ok(Outcome {
                rows,
                plan,
                answer: String::new(),
            })
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
            Ok(Outcome {
                rows,
                plan,
                answer: String::new(),
            })
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
