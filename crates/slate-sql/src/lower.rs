//! The query spec, lowered onto the kernel's own types.
//!
//! # Where the line is
//!
//! Everything here turns a spec into something `slate_kernel` understands: a
//! `Query`, an `Expr`, a `Scalar`, an `Aggregate`, a `Window`, a `Value`. What
//! stayed in `slate-wasm` turns the same specs into *strings for a grid* —
//! column headers, aggregate labels, rendered cells, the keyspace viewer.
//!
//! That is the rule the split was made by, and it is worth stating because the
//! two kinds sat interleaved: `windows` builds a `KernelWindow` and
//! `window_header` builds the text above the column it produces. A daemon
//! resolving a view needs the first and has no grid to fill.
//!
//! # Why it is not in `sql`
//!
//! `sql::parse` produces a `QuerySpec` and stops. The spec is also what the
//! three SDKs send over gRPC, so lowering it is not a SQL concern — SQL is one
//! of two front ends onto the same shape, and this is the back of both.

use crate::{AggregateSpec, ComputeSpec, FilterSpec, QuerySpec, WindowSpec};
use slate_kernel::{
    Aggregate, CalendarPart, CalendarUnit, CmpOp, Expr, Query, Scalar, SortKey, TimeUnit,
    window::{Window as KernelWindow, WindowFunction},
};
use slate_schema::{Ordinal, TableDef};
use slate_tuple::Value;

/// Turn the UI's description into a kernel `Query`.
pub fn build(spec: &QuerySpec, table: &TableDef) -> Result<Query, String> {
    let mut query = Query::all();

    // First, because everything below may name a computed ordinal and the
    // kernel only knows what those mean once the query carries the
    // expressions that produce them.
    if !spec.compute.is_empty() {
        query = query.computing(computes(&spec.compute, table)?);
    }

    // After the computed values, because a window may partition by or order on
    // one and its own ordinal is counted past them. Refused beside a grouping
    // rather than dropped: the kernel's `narrowed` drops a window on the way
    // into a grouped read, which is right for the kernel — a window over the
    // rows going *into* a fold is a value nobody sees — and would be silent
    // here, where the reader wrote both and one of them just stopped
    // happening.
    if !spec.window.is_empty() {
        if !spec.group_by.is_empty() || !spec.aggregates.is_empty() {
            return Err(
                "a window and a GROUP BY answer different questions: a grouping \
                        returns one row per group and a window returns one value per input \
                        row, so a query asking for both has nowhere to put the window's \
                        answer. Keep one."
                    .to_owned(),
            );
        }
        query = query.windowing(windows(&spec.window, spec.compute.len(), table)?);
    }

    // `filter` and `filters` are both accepted and both ANDed in. The single
    // form is not deprecated shorthand — it is what a one-condition panel
    // sends, and refusing it would break the shape this binding shipped with.
    let conditions: Vec<&FilterSpec> = spec.filter.iter().chain(spec.filters.iter()).collect();
    if !conditions.is_empty() {
        let mut parts = Vec::with_capacity(conditions.len());
        for condition in conditions {
            parts.push(comparison(condition, table)?);
        }
        // `Expr::all` rather than folding with `and`: it is the kernel's own
        // constructor for a conjunction, and the planner reads conjuncts out
        // of it to look for scan bounds. A hand-folded tree of nested `And`s
        // is the same predicate and gives the planner more work to undo.
        query = query.filter(Expr::all(parts));
    }
    // ORed conditions, as one conjunct beside the ANDed ones.
    //
    // `Expr::Or` directly rather than a fold, for the reason above: the
    // planner's `conjuncts()` flattens `And` and stops at anything else, so a
    // disjunction arrives as one opaque conjunct it will evaluate per row
    // rather than turn into a scan bound. That is correct and it is the cost
    // of the feature — `a = 1 OR b = 2` has no single key range — and it is
    // why this is a filter and not an access path.
    //
    // Conjoined with whatever `filters` produced rather than replacing it, so
    // a spec carrying both means `(all) AND (any)`. The parser never produces
    // both today; the lowering is written for the shape a widened parser
    // would send, so that widening does not silently change what an existing
    // spec means.
    if !spec.any_of.is_empty() {
        let mut parts = Vec::with_capacity(spec.any_of.len());
        for condition in &spec.any_of {
            parts.push(comparison(condition, table)?);
        }
        // `Expr::and` on the predicate built above, taken out of the builder
        // first: `Query::filter` consumes the query, so reading the field it
        // is about to overwrite has to happen before the call rather than
        // inside it.
        let existing = std::mem::replace(&mut query.filter, Expr::True);
        query = query.filter(existing.and(Expr::Or(parts)));
    }
    if !spec.sort.is_empty() {
        let keys: Vec<SortKey> = spec
            .sort
            .iter()
            .map(|s| {
                let column = Ordinal(s.column as usize);
                if s.descending {
                    SortKey::desc(column)
                } else {
                    SortKey::asc(column)
                }
            })
            .collect();
        query = query.sort_by(keys);
    }
    if !spec.columns.is_empty() {
        query = query.select(spec.columns.iter().map(|c| Ordinal(*c as usize)));
    }
    if let Some(limit) = spec.limit {
        query = query.limit(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    if spec.offset > 0 {
        query = query.offset(usize::try_from(spec.offset).unwrap_or(usize::MAX));
    }
    Ok(query)
}

/// One `column op literal`, with the literal parsed to the column's type.
///
/// Typed rather than coerced, because the kernel's value order is type-first —
/// that is what makes the key encoding sortable — so handing a `Str` to a `U64`
/// column would not fail, it would compare the *types* and match nothing.
/// Getting this wrong is silent, so it is done once, here.
pub fn comparison(filter: &FilterSpec, table: &TableDef) -> Result<Expr, String> {
    let column = Ordinal(filter.column as usize);
    let def = table
        .column(column)
        .ok_or_else(|| format!("{} has no column {}", table.name(), filter.column))?;

    if filter.op == "in" || filter.op == "notIn" {
        // Typed one at a time against this column, like every other literal.
        // The subquery form has already been run by `resolve_subqueries` and
        // its rows rendered into `values`; by here the two are the same thing.
        let mut values = Vec::with_capacity(filter.values.len());
        for text in &filter.values {
            values.push(literal(text, def.value_type(), def.scale()).map_err(|why| {
                if filter.subquery.is_some() {
                    // Without this the message is about a value the reader
                    // never typed, and points at the outer column rather than
                    // at the subquery that produced it.
                    format!("the subquery produced a value this column cannot hold: {why}")
                } else {
                    why
                }
            })?);
        }
        let inside = Expr::In { column, values };
        // `NOT IN` is the `IN` negated, and nothing more. `Truth::negate` maps
        // unknown to unknown, so a null candidate makes this unknown exactly
        // where the standard says it should — and `Expr::conjuncts` stops at a
        // `Not`, so the planner never mistakes the complement of a point set
        // for a range. See the long note in `sql.rs`, which is where this was
        // refused until it was checked.
        return Ok(if filter.op == "notIn" {
            Expr::Not(Box::new(inside))
        } else {
            inside
        });
    }

    // Patterns are strings whatever the column is.
    match filter.op.as_str() {
        "like" => return Ok(Expr::like(column, filter.value.clone())),
        "ilike" => return Ok(Expr::ilike(column, filter.value.clone())),
        // The search text, not a term list: `Expr::contains` tokenizes with
        // the same function the write path tokenizes the column with. This
        // binding splitting it first would be a fifth tokenizer.
        "contains" => return Ok(Expr::contains(column, &filter.value)),
        "matches" => {
            let expr = Expr::matches(column, filter.value.clone());
            if let Some(bad) = expr.regex_error() {
                return Err(format!("that regular expression is not valid: {bad}"));
            }
            return Ok(expr);
        }
        _ => {}
    }

    let value = literal(&filter.value, def.value_type(), def.scale())?;
    // `Expr::compare` rather than the `eq`/`lt` shorthands: those names exist
    // on `Expr` as *combinators over expressions*, not comparison
    // constructors, and reaching for them here compiled into something else
    // entirely.
    let op = match filter.op.as_str() {
        "eq" => CmpOp::Eq,
        "ne" => CmpOp::Ne,
        "lt" => CmpOp::Lt,
        "le" => CmpOp::Le,
        "gt" => CmpOp::Gt,
        "ge" => CmpOp::Ge,
        other => return Err(format!("no such operator: {other}")),
    };
    Ok(Expr::compare(column, op, value))
}

pub fn literal(
    text: &str,
    kind: slate_tuple::ValueType,
    scale: Option<u8>,
) -> Result<Value, String> {
    use slate_tuple::ValueType as T;
    match kind {
        // `WHERE price > 19.99` used to fall through to the catch-all and
        // become `Value::Str("19.99")`, which compares below every decimal in
        // the cross-type order and so matched *nothing*, silently. A query
        // that returns no rows for a reason nobody can see is the worst answer
        // available, and it was the one this gave.
        //
        // The scale comes from the column, because it is the only thing that
        // knows: `19.99` is 1999 units at scale 2 and 199900 at scale 4. A
        // decimal column with no scale is impossible — `ColumnDef::scale`
        // returns `Some` for every `Decimal` — so `None` here means the caller
        // passed the wrong column, which is a bug rather than a bad query.
        T::Decimal => {
            let scale = scale.ok_or_else(|| {
                "a decimal column with no scale — this is a bug in the front end, not in \
                 the query"
                    .to_owned()
            })?;
            Value::decimal_from_str(text, scale)
        }
        T::U64 => text
            .trim()
            .parse::<u64>()
            .map(Value::U64)
            .map_err(|_| format!("{text:?} is not a non-negative whole number")),
        T::I64 => text
            .trim()
            .parse::<i64>()
            .map(Value::I64)
            .map_err(|_| format!("{text:?} is not a whole number")),
        T::F64 => text
            .trim()
            .parse::<f64>()
            .map(Value::F64)
            .map_err(|_| format!("{text:?} is not a number")),
        T::Bool => text
            .trim()
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|_| format!("{text:?} is not true or false")),
        _ => Ok(Value::Str(text.to_owned())),
    }
}

/// Lower the UI's aggregate list onto the kernel's, resolving ordinals
/// against the table the aggregates read.
///
/// An empty list is empty. It used to mean `count(*)`, on the argument that a
/// bare list of keys looks broken rather than minimal — which was a guess about
/// what a reader wants, made in the one place that could not be overridden, and
/// it is what `SELECT DISTINCT` needs to be able to ask for.
pub fn aggregates(specs: &[AggregateSpec], table: &TableDef) -> Result<Vec<Aggregate>, String> {
    // An empty list means no aggregates, not `count(*)`. See the note on the
    // join path: the default made `SELECT author_id FROM books GROUP BY
    // author_id` come back two columns wide, one of which the query does not
    // mention, and made `SELECT DISTINCT` inexpressible.
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let column = Ordinal(spec.column as usize);
        if !matches!(spec.kind.as_str(), "count") && table.column(column).is_none() {
            return Err(format!("{} has no column {}", table.name(), spec.column));
        }
        out.push(match spec.kind.as_str() {
            "count" => Aggregate::Count,
            // `count(column)` is not `count(*)`: it skips nulls. Keeping them
            // apart here is why the parser bothers to tell them apart.
            "count_column" => Aggregate::CountColumn(column),
            "min" => Aggregate::Min(column),
            "max" => Aggregate::Max(column),
            "sum" => Aggregate::Sum(column),
            "avg" => Aggregate::Avg(column),
            "count_distinct" => Aggregate::CountDistinct(column),
            other => return Err(format!("no such aggregate: {other}")),
        });
    }
    Ok(out)
}

/// Lower the UI's window list onto the kernel's.
///
/// `computes` is how many computed values the query carries, which is what
/// makes an ordinal past the table's own columns legal here: a window may
/// partition by or order on `hour(pickup_time)`, and that column exists only
/// because the query computes it. Checked rather than trusted, because an
/// ordinal past the row's width reads a value that is not there — which the
/// kernel answers as null rather than refusing, and a column of nulls is
/// indistinguishable on screen from a partition that happened to be empty.
pub fn windows(
    specs: &[WindowSpec],
    computes: usize,
    table: &TableDef,
) -> Result<Vec<KernelWindow>, String> {
    let width = table.columns().len() + computes;
    let check = |ordinal: u32, what: &str| -> Result<Ordinal, String> {
        if ordinal as usize >= width {
            return Err(format!(
                "a window\'s {what} names ordinal {ordinal}, and this query is {width} \
                 values wide"
            ));
        }
        Ok(Ordinal(ordinal as usize))
    };
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let offset = usize::try_from(spec.offset).unwrap_or(usize::MAX);
        let function = match spec.function.as_str() {
            "row_number" => WindowFunction::RowNumber,
            "rank" => WindowFunction::Rank,
            "dense_rank" => WindowFunction::DenseRank,
            "lag" => WindowFunction::Lag {
                column: check(spec.column, "column")?,
                offset,
            },
            "lead" => WindowFunction::Lead {
                column: check(spec.column, "column")?,
                offset,
            },
            "aggregate" => {
                let Some(aggregate) = &spec.aggregate else {
                    return Err(
                        "a window aggregate has no aggregate: `aggregate` names the kind \
                         and the column, and without it there is nothing to compute"
                            .to_owned(),
                    );
                };
                // Through the same lowering a `GROUP BY` uses, so `sum(x)`
                // means one thing whichever clause asked for it and a new
                // aggregate has one place to be added.
                let mut one = aggregates(std::slice::from_ref(aggregate), table)?;
                WindowFunction::Over(one.remove(0))
            }
            other => {
                return Err(format!(
                    "no such window function: `{other}` — this has row_number, rank, \
                     dense_rank, lag, lead and aggregate"
                ));
            }
        };
        let mut partition = Vec::with_capacity(spec.partition_by.len());
        for ordinal in &spec.partition_by {
            partition.push(check(*ordinal, "PARTITION BY")?);
        }
        let mut order = Vec::with_capacity(spec.order.len());
        for key in &spec.order {
            let column = check(key.column, "ORDER BY")?;
            order.push(if key.descending {
                SortKey::desc(column)
            } else {
                SortKey::asc(column)
            });
        }
        // The specifications with no meaning — an unordered rank, a running
        // COUNT(DISTINCT), an offset of zero — are refused by `Window::new`
        // rather than restated here. One statement of each rule, in the layer
        // that has to enforce it anyway.
        out.push(KernelWindow::new(function, partition, order).map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// One `ComputeSpec` as the kernel's `Scalar`.
///
/// The names are SQL's where SQL has one — `EXTRACT(HOUR FROM t)` and
/// `EXTRACT(DAY FROM t)` mean hour-of-day and day-of-*month*, so `day` here is
/// the day of the month and not the day of the epoch, which is what
/// `TimeUnit::Day` would give. Getting that backwards would be silent: both
/// return an integer and both look plausible in a column.
pub fn compute_scalar(spec: &ComputeSpec, table: &TableDef, base: usize) -> Result<Scalar, String> {
    use slate_tuple::ValueType as T;
    let column = Ordinal(spec.column as usize);
    let def = table
        .column(column)
        .ok_or_else(|| format!("{} has no column {}", table.name(), spec.column))?;
    let kind = def.value_type();
    // The column is named in its own table's ordinals and read in the space the
    // expression is evaluated in -- the same ordinals on one table or on a
    // join's left side, shifted past every left column on its right side.
    let mut value = Scalar::Column(Ordinal(base + column.0));

    // The zone shift, if there is one. Adding seconds to a timestamp and then
    // reading the calendar out of the result *is* what a fixed-offset
    // conversion is, so this needs no new `Scalar` variant and no new wire
    // field — see `ComputeSpec::offset`.
    //
    // `arithmetic` promotes two integers to `I64` and saturates rather than
    // wrapping, so a `U64` column shifted below the epoch becomes a negative
    // `I64` and `CalendarPart`'s floor division handles it, rather than
    // wrapping to the year 584942417355.
    if spec.offset != 0 || !spec.zone.is_empty() {
        if spec.function == "round" {
            return Err("round() takes no timezone: it is not a time function".to_owned());
        }
        if spec.offset != 0 && !spec.zone.is_empty() {
            // Not reachable from the parser, which produces one or the other.
            // Refused rather than given a precedence, because a spec built by
            // hand with both set means the caller believes something untrue
            // about which one wins, and answering either way confirms it.
            return Err(format!(
                "{}() was given both a fixed offset and the zone {:?}; they are two \
                 answers to one question",
                spec.function, spec.zone
            ));
        }
        value = if spec.zone.is_empty() {
            Scalar::Add(
                Box::new(value),
                Box::new(Scalar::Literal(slate_tuple::Value::I64(spec.offset))),
            )
        } else {
            // Checked here as well as in the parser, because a spec can arrive
            // from JavaScript without passing through the parser at all, and
            // `ZoneShift` answers null for a zone it does not know — which on
            // screen is indistinguishable from an empty column.
            if !slate_kernel::zones::has(&spec.zone) {
                return Err(format!(
                    "no such timezone: {:?}. IANA names are case-sensitive, and \
                     this has {}",
                    spec.zone,
                    slate_kernel::zones::listing()
                ));
            }
            value.in_zone(spec.zone.clone())
        };
    }

    // Checked per function rather than once, because they do not agree on what
    // they take: a timestamp is an integer of seconds — there is no date type
    // — and `round` is for the columns that are not. Refused here rather than
    // left to the kernel, which would return null per row: right for a value
    // of the wrong shape, wrong for a query that could never have worked, and
    // indistinguishable on screen from a column that is genuinely empty.
    let timestamp = |scalar: Scalar| {
        if matches!(kind, T::I64 | T::U64) {
            Ok(scalar)
        } else {
            Err(format!(
                "{}() needs a timestamp, and {} is {:?} — timestamps here are \
                 seconds since the epoch in an integer column",
                spec.function,
                def.name(),
                kind
            ))
        }
    };

    match spec.function.as_str() {
        "hour" => timestamp(value.extract(TimeUnit::Hour)),
        "minute" => timestamp(value.extract(TimeUnit::Minute)),
        "second" => timestamp(value.extract(TimeUnit::Second)),
        "year" => timestamp(value.calendar_part(CalendarPart::Year)),
        "month" => timestamp(value.calendar_part(CalendarPart::Month)),
        // `day` is the day of the *month*, as `EXTRACT(DAY FROM t)` is in SQL
        // — not `TimeUnit::Day`, which counts days since the epoch. Both
        // return an integer and both look plausible in a column, so getting
        // this backwards would be silent.
        "day" => timestamp(value.calendar_part(CalendarPart::DayOfMonth)),
        "day_of_week" => timestamp(value.calendar_part(CalendarPart::DayOfWeek)),
        // Midnight of the day, as epoch seconds — so grouping by it gives one
        // group per calendar day, ordered as the days are.
        "date" => timestamp(value.date_trunc(TimeUnit::Day)),
        // The first instant of the month or the year. `date()` is a division
        // by 86,400; these are not, because neither a month nor a year has a
        // fixed length — see `CalendarUnit`. Grouping by one gives a group per
        // calendar month, ordered as the months are, which is what `year()`
        // and `month()` cannot do on their own: those return 2024 and 2,
        // so ordering by `month()` puts every January of every year together.
        "month_start" => timestamp(value.calendar_trunc(CalendarUnit::Month)),
        "year_start" => timestamp(value.calendar_trunc(CalendarUnit::Year)),
        "round" => {
            if matches!(kind, T::F64 | T::I64 | T::U64) {
                Ok(value.round())
            } else {
                Err(format!(
                    "round() needs a number, and {} is {kind:?}",
                    def.name()
                ))
            }
        }
        other => Err(format!("no such function: {other}")),
    }
}

/// Every computed column a spec asks for, in order.
pub fn computes(specs: &[ComputeSpec], table: &TableDef) -> Result<Vec<Scalar>, String> {
    specs.iter().map(|c| compute_scalar(c, table, 0)).collect()
}

/// The joined-space ordinal an `input`/`column` pair names.
///
/// Zero is the left table, whose ordinals are already the joined row's; one is
/// the right, shifted past every left column. Any other input is refused rather
/// than treated as one of the two -- a join here has exactly two sides, and
/// guessing would turn a typo into a query about a different column.
pub fn joined_ordinal(
    input: u32,
    column: u32,
    left: &TableDef,
    right: &TableDef,
) -> Result<Ordinal, String> {
    chained_ordinal(input, column, &[left, right])
}

/// The same, over any number of inputs.
///
/// A chain's joined space is every table's columns concatenated in order, so
/// an input's base is the sum of the widths before it. Two tables is the
/// degenerate case rather than a separate scheme, which is why
/// [`joined_ordinal`] is one line.
pub fn chained_ordinal(input: u32, column: u32, tables: &[&TableDef]) -> Result<Ordinal, String> {
    let at = input as usize;
    let table = tables.get(at).ok_or_else(|| {
        format!(
            "this read has {} input{}, 0 to {}; input {input} is outside that",
            tables.len(),
            if tables.len() == 1 { "" } else { "s" },
            tables.len().saturating_sub(1)
        )
    })?;
    if table.column(Ordinal(column as usize)).is_none() {
        return Err(format!("{} has no column {column}", table.name()));
    }
    Ok(Ordinal(base_of(at, tables) + column as usize))
}

/// Where input `at`'s columns begin in the joined space.
pub fn base_of(at: usize, tables: &[&TableDef]) -> usize {
    tables
        .iter()
        .take(at)
        .map(|table| table.columns().len())
        .sum()
}

/// The join's computed values, each reading whichever side it names.
pub fn joined_computes(
    specs: &[ComputeSpec],
    left: &TableDef,
    right: &TableDef,
) -> Result<Vec<Scalar>, String> {
    chained_computes(specs, &[left, right])
}

/// The same, over any number of inputs.
pub fn chained_computes(
    specs: &[ComputeSpec],
    tables: &[&TableDef],
) -> Result<Vec<Scalar>, String> {
    specs
        .iter()
        .map(|c| {
            let at = c.input as usize;
            let table = tables.get(at).ok_or_else(|| {
                format!(
                    "a computed value names input {}, and this read has {}",
                    c.input,
                    tables.len()
                )
            })?;
            compute_scalar(c, table, base_of(at, tables))
        })
        .collect()
}

/// The type a group-space ordinal holds, for `HAVING`.
///
/// This exists because of one hazard, and it is a silent one. `Value` orders
/// by *class* before it orders by magnitude, and `F64` ranks above the integer
/// variants — so `F64(19.5) > I64(20)` is **true**, by rank, with the numbers
/// playing no part. `I64` and `U64` share a rank and compare through `i128`,
/// so they interoperate; a float against an integer does not.
///
/// `HAVING avg(fare) > 20` therefore has to parse `20` as `F64(20.0)`, and
/// parsing it from the table column's type — as `WHERE` correctly does — would
/// give `I64(20)` and admit every group. Nothing would error and the answer
/// would be wrong, which is why the type comes from the aggregate rather than
/// from the column it reads.
pub fn group_value_type(
    ordinal: u32,
    keys: &[Ordinal],
    aggregates: &[Aggregate],
    inputs: &[&TableDef],
) -> Result<(slate_tuple::ValueType, Option<u8>), String> {
    use slate_tuple::ValueType as T;
    let index = ordinal as usize;
    // A slice of tables rather than one, so a *joined* key resolves: a group
    // key on a join is an ordinal of the joined row, and which table it lands
    // in is the sum of the widths before it. A single table is the one-input
    // case of the same walk, which is why both paths share this rather than
    // growing a second copy — the reason the joined path had no HAVING at all
    // was that everything downstream of here took one `TableDef`.
    let column_type = |c: Ordinal| {
        let mut at = c.0;
        for table in inputs {
            let width = table.columns().len();
            if at < width {
                // The scale travels with the type, because a decimal literal
                // in `HAVING` has to be read at the scale of whatever the
                // aggregate produced — `having sum(price) > 100.00` is a
                // count of the *column's* cents. `HAVING` over a decimal was
                // comparing `I64(100)` to a `Decimal` before this, which
                // admits nothing and says nothing.
                return table
                    .column(Ordinal(at))
                    .map(|d| (d.value_type(), d.scale()))
                    .ok_or_else(|| format!("{} has no column {}", table.name(), at));
            }
            at -= width;
        }
        Err(format!("no input holds joined column {}", c.0))
    };
    let width: usize = inputs.iter().map(|t| t.columns().len()).sum();
    if let Some(key) = keys.get(index) {
        // A group key past the table's own columns is a computed one, and
        // every function `compute_scalar` offers returns an integer.
        //
        // No query can currently tell this branch from reading the *source*
        // column's type, and a mutation replacing it with `if false` passes
        // the whole suite — because `compute_scalar` refuses a non-integer
        // source, so both readings land on `I64` or `U64`, which share a class
        // rank and compare through `i128`. It is here for the first function
        // that returns a double, where the two stop agreeing and the
        // disagreement is silent (see `having`, which explains the rank
        // hazard). Written down rather than deleted, and written down rather
        // than covered by a test that does not exist.
        if key.0 >= width {
            return Ok((T::I64, None));
        }
        return column_type(*key);
    }
    let aggregate = aggregates.get(index - keys.len()).ok_or_else(|| {
        format!(
            "HAVING names position {index}, and the group has only {} columns",
            keys.len() + aggregates.len()
        )
    })?;
    Ok(match aggregate {
        // A count is a cardinality: unsigned, whatever it counted.
        Aggregate::Count | Aggregate::CountColumn(_) | Aggregate::CountDistinct(_) => {
            (T::U64, None)
        }
        // A minimum is one of the values, so it is whatever they are — scale
        // included.
        Aggregate::Min(c) | Aggregate::Max(c) => column_type(*c)?,
        // `Total::sum` returns `I64` for any integer column and `F64` for a
        // real one, so a `U64` column's sum is compared as `I64` — same rank,
        // so that is a distinction without a difference here. Over a decimal
        // it returns a `Decimal` at the column's scale, which is the whole
        // point of summing money, and is why this arm cannot just be "integer
        // or float".
        Aggregate::Sum(c) => match column_type(*c)? {
            (T::F64, _) => (T::F64, None),
            (T::Decimal, scale) => (T::Decimal, scale),
            _ => (T::I64, None),
        },
        // Always a double, even over integers and decimals: `Total::average`
        // divides, and an average of cents is not cents.
        Aggregate::Avg(_) => (T::F64, None),
    })
}

/// `HAVING`, as one `Expr` over the group.
pub fn having(
    specs: &[FilterSpec],
    keys: &[Ordinal],
    aggregates: &[Aggregate],
    inputs: &[&TableDef],
) -> Result<Expr, String> {
    let mut out = Expr::True;
    for spec in specs {
        let (kind, scale) = group_value_type(spec.column, keys, aggregates, inputs)?;
        let column = Ordinal(spec.column as usize);
        let expr = match spec.op.as_str() {
            "like" => Expr::like(column, spec.value.clone()),
            "ilike" => Expr::ilike(column, spec.value.clone()),
            "contains" => Expr::contains(column, &spec.value),
            "matches" => {
                let expr = Expr::matches(column, spec.value.clone());
                if let Some(bad) = expr.regex_error() {
                    return Err(format!("that regular expression is not valid: {bad}"));
                }
                expr
            }
            other => {
                let op = match other {
                    "eq" => CmpOp::Eq,
                    "ne" => CmpOp::Ne,
                    "lt" => CmpOp::Lt,
                    "le" => CmpOp::Le,
                    "gt" => CmpOp::Gt,
                    "ge" => CmpOp::Ge,
                    _ => return Err(format!("no such operator: {other}")),
                };
                Expr::compare(column, op, literal(&spec.value, kind, scale)?)
            }
        };
        out = match out {
            Expr::True => expr,
            existing => existing.and(expr),
        };
    }
    Ok(out)
}

/// One aggregate, by the name the spec uses, over an ordinal already resolved
/// into the joined space.
///
/// Shared by the join and the chain paths. It was written out in the join path
/// and copying it was the alternative, which is how two lists of aggregate
/// names come to disagree about whether `count_column` is spelled with an
/// underscore.
pub fn aggregate_of(kind: &str, column: Ordinal) -> Result<Aggregate, String> {
    Ok(match kind {
        "count" => Aggregate::Count,
        "count_column" => Aggregate::CountColumn(column),
        "count_distinct" => Aggregate::CountDistinct(column),
        "min" => Aggregate::Min(column),
        "max" => Aggregate::Max(column),
        "sum" => Aggregate::Sum(column),
        "avg" => Aggregate::Avg(column),
        other => return Err(format!("no such aggregate: {other}")),
    })
}

pub fn conditions(specs: &[FilterSpec], table: &TableDef) -> Result<Query, String> {
    if specs.is_empty() {
        return Ok(Query::all());
    }
    let mut parts = Vec::with_capacity(specs.len());
    for spec in specs {
        parts.push(comparison(spec, table)?);
    }
    Ok(Query::all().filter(Expr::all(parts)))
}
