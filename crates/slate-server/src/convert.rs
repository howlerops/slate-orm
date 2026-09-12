//! Between the wire types and the kernel's.
//!
//! Every function here is total in one direction and fallible in the other.
//! Going out, a kernel value always has a wire form. Coming in, it may not: the
//! bytes arrived from somewhere, and a decoder that guesses at what a client
//! meant is a decoder that quietly changes a query. So an unset `oneof`, an
//! unspecified enum, a sixteen-byte field holding fifteen bytes, and an ordinal
//! past the end of the table are all refused rather than defaulted.
//!
//! The two directions are written as separate functions rather than as `From`
//! impls in both directions, because the round trip is a property worth
//! testing and `tests/wire.rs` tests it: for any kernel value, converting out
//! and back is the identity. That is the check that catches a variant added on
//! one side and forgotten on the other, which is the failure this module exists
//! to have.
//!
//! # What is not on the wire
//!
//! [`Scalar`](slate_kernel::Scalar) computed columns, joins, chains and
//! aggregates all exist in the kernel and none of them are here. That is a
//! scope decision, not an oversight: each needs an ordinal space or a grouping
//! model of its own on the wire, and shipping half of one would be worse than
//! shipping none. A client that needs them today can compute over the rows a
//! query returns.

use crate::proto as pb;
use slate_kernel::query::{AccessHint, NullsOrder, Query, SortKey};
use slate_kernel::{CmpOp, Explanation, Expr, Freshness, Projection, ReadToken, ScanOrder};
use slate_schema::{Ordinal, Row, TableDef};
use slate_tuple::{Direction, Value};
use tonic::Status;
use uuid::Uuid;

/// A client sent something this server cannot read.
fn bad(message: impl Into<String>) -> Status {
    Status::invalid_argument(message)
}

// --- values ---------------------------------------------------------------

/// A kernel value in its wire form.
#[must_use]
pub fn value_to_proto(value: &Value) -> pb::Value {
    use pb::value::Kind;
    let kind = match value {
        Value::Null => Kind::NullValue(pb::NullValue::NullValue as i32),
        Value::Bool(b) => Kind::BoolValue(*b),
        Value::Bytes(bytes) => Kind::BytesValue(bytes.to_vec()),
        Value::Str(text) => Kind::StringValue(text.clone()),
        Value::I64(n) => Kind::Int64Value(*n),
        Value::U64(n) => Kind::Uint64Value(*n),
        Value::F64(x) => Kind::DoubleValue(*x),
        Value::Uuid(id) => Kind::UuidValue(id.as_bytes().to_vec()),
        Value::Vector(elements) => Kind::VectorValue(pb::Vector {
            elements: elements.clone(),
        }),
        // `Value` is `#[non_exhaustive]`, so this cannot be an exhaustive
        // match and a new variant will reach here rather than failing to
        // compile. Sending it as a null would be silent data loss, so it is an
        // error the server can be made to say out loud.
        other => Kind::StringValue(format!("<unrepresentable {}>", other.type_name())),
    };
    pb::Value { kind: Some(kind) }
}

/// A wire value as the kernel's.
pub fn value_from_proto(value: &pb::Value) -> Result<Value, Status> {
    use pb::value::Kind;
    let Some(kind) = &value.kind else {
        // Not treated as a null: proto3 cannot distinguish an unset field from
        // a zero one, so an unset `kind` is most likely a client built against
        // a schema this server does not have. Guessing "null" would turn its
        // predicate into a different predicate.
        return Err(bad(
            "a value arrived with no kind set; send an explicit null",
        ));
    };
    Ok(match kind {
        Kind::NullValue(_) => Value::Null,
        Kind::BoolValue(b) => Value::Bool(*b),
        Kind::BytesValue(bytes) => Value::Bytes(bytes::Bytes::from(bytes.clone())),
        Kind::StringValue(text) => Value::Str(text.clone()),
        Kind::Int64Value(n) => Value::I64(*n),
        Kind::Uint64Value(n) => Value::U64(*n),
        Kind::DoubleValue(x) => Value::F64(*x),
        Kind::UuidValue(bytes) => {
            let octets: [u8; 16] = bytes.as_slice().try_into().map_err(|_| {
                bad(format!(
                    "a uuid value must be exactly 16 bytes, got {}",
                    bytes.len()
                ))
            })?;
            Value::Uuid(Uuid::from_bytes(octets))
        }
        Kind::VectorValue(vector) => Value::Vector(vector.elements.clone()),
    })
}

/// A row in its wire form.
#[must_use]
pub fn row_to_proto(row: &Row) -> pb::Row {
    pb::Row {
        values: row.values().iter().map(value_to_proto).collect(),
    }
}

/// The values of a wire row, without checking them against any table.
pub fn values_from_proto(row: &pb::Row) -> Result<Vec<Value>, Status> {
    row.values.iter().map(value_from_proto).collect()
}

/// A wire row as the kernel's.
///
/// The width is not checked here; [`Row::validate`] does that against the
/// table, and it produces the better message.
pub fn row_from_proto(row: &pb::Row) -> Result<Row, Status> {
    Ok(Row::new(values_from_proto(row)?))
}

// --- predicates -----------------------------------------------------------

const fn op_to_proto(op: CmpOp) -> pb::CmpOp {
    match op {
        CmpOp::Eq => pb::CmpOp::Eq,
        CmpOp::Ne => pb::CmpOp::Ne,
        CmpOp::Lt => pb::CmpOp::Lt,
        CmpOp::Le => pb::CmpOp::Le,
        CmpOp::Gt => pb::CmpOp::Gt,
        CmpOp::Ge => pb::CmpOp::Ge,
    }
}

fn op_from_proto(op: i32) -> Result<CmpOp, Status> {
    match pb::CmpOp::try_from(op) {
        Ok(pb::CmpOp::Eq) => Ok(CmpOp::Eq),
        Ok(pb::CmpOp::Ne) => Ok(CmpOp::Ne),
        Ok(pb::CmpOp::Lt) => Ok(CmpOp::Lt),
        Ok(pb::CmpOp::Le) => Ok(CmpOp::Le),
        Ok(pb::CmpOp::Gt) => Ok(CmpOp::Gt),
        Ok(pb::CmpOp::Ge) => Ok(CmpOp::Ge),
        // Defaulting to equality would make a malformed comparison quietly
        // mean something.
        Ok(pb::CmpOp::Unspecified) | Err(_) => Err(bad(format!(
            "comparison operator {op} is not one this server knows"
        ))),
    }
}

const fn ordinal_to_proto(ordinal: Ordinal) -> u32 {
    // Ordinals are column positions in a table, so the truncation this cast
    // could in principle perform needs a table four billion columns wide.
    ordinal.0 as u32
}

const fn ordinal_from_proto(ordinal: u32) -> Ordinal {
    Ordinal(ordinal as usize)
}

/// A predicate in its wire form.
#[must_use]
pub fn expr_to_proto(expr: &Expr) -> pb::Expr {
    use pb::expr::Node;
    let node = match expr {
        Expr::True => Node::Literal(true),
        Expr::False => Node::Literal(false),
        Expr::Compare { column, op, value } => Node::Compare(pb::Compare {
            column: ordinal_to_proto(*column),
            op: op_to_proto(*op) as i32,
            value: Some(value_to_proto(value)),
        }),
        Expr::CompareColumns { left, op, right } => Node::CompareColumns(pb::CompareColumns {
            left: ordinal_to_proto(*left),
            op: op_to_proto(*op) as i32,
            right: ordinal_to_proto(*right),
        }),
        Expr::IsNull { column, negated } => Node::IsNull(pb::IsNull {
            column: ordinal_to_proto(*column),
            negated: *negated,
        }),
        Expr::Like {
            column,
            pattern,
            negated,
            insensitive,
        } => Node::Like(pb::Like {
            column: ordinal_to_proto(*column),
            pattern: pattern.clone(),
            negated: *negated,
            insensitive: *insensitive,
        }),
        Expr::Matches {
            column,
            pattern,
            negated,
            insensitive,
        } => Node::Matches(pb::Matches {
            column: ordinal_to_proto(*column),
            pattern: pattern.clone(),
            negated: *negated,
            insensitive: *insensitive,
        }),
        Expr::In { column, values } => Node::InList(pb::InList {
            column: ordinal_to_proto(*column),
            values: values.iter().map(value_to_proto).collect(),
        }),
        Expr::And(parts) => Node::Conjunction(pb::ExprList {
            exprs: parts.iter().map(expr_to_proto).collect(),
        }),
        Expr::Or(parts) => Node::Disjunction(pb::ExprList {
            exprs: parts.iter().map(expr_to_proto).collect(),
        }),
        Expr::Not(inner) => Node::Negation(Box::new(expr_to_proto(inner))),
        // `Expr` is `#[non_exhaustive]`. A predicate this server cannot
        // represent must not become `True`, which would widen the result — and
        // on a table under a policy, widening is the failure that matters. It
        // becomes `False`, which is the safe direction, and the caller sees an
        // empty result rather than someone else's rows.
        _ => Node::Literal(false),
    };
    pb::Expr { node: Some(node) }
}

/// A wire predicate as the kernel's.
pub fn expr_from_proto(expr: &pb::Expr) -> Result<Expr, Status> {
    use pb::expr::Node;
    let Some(node) = &expr.node else {
        return Err(bad("an expression arrived with no node set"));
    };
    Ok(match node {
        Node::Literal(true) => Expr::True,
        Node::Literal(false) => Expr::False,
        Node::Compare(compare) => Expr::Compare {
            column: ordinal_from_proto(compare.column),
            op: op_from_proto(compare.op)?,
            value: match &compare.value {
                Some(value) => value_from_proto(value)?,
                None => return Err(bad("a comparison arrived with no value")),
            },
        },
        Node::CompareColumns(compare) => Expr::CompareColumns {
            left: ordinal_from_proto(compare.left),
            op: op_from_proto(compare.op)?,
            right: ordinal_from_proto(compare.right),
        },
        Node::IsNull(is_null) => Expr::IsNull {
            column: ordinal_from_proto(is_null.column),
            negated: is_null.negated,
        },
        Node::Like(like) => Expr::Like {
            column: ordinal_from_proto(like.column),
            pattern: like.pattern.clone(),
            negated: like.negated,
            insensitive: like.insensitive,
        },
        Node::Matches(matches) => Expr::Matches {
            column: ordinal_from_proto(matches.column),
            pattern: matches.pattern.clone(),
            negated: matches.negated,
            insensitive: matches.insensitive,
        },
        Node::InList(in_list) => Expr::In {
            column: ordinal_from_proto(in_list.column),
            values: in_list
                .values
                .iter()
                .map(value_from_proto)
                .collect::<Result<_, _>>()?,
        },
        Node::Conjunction(list) => Expr::And(
            list.exprs
                .iter()
                .map(expr_from_proto)
                .collect::<Result<_, _>>()?,
        ),
        Node::Disjunction(list) => Expr::Or(
            list.exprs
                .iter()
                .map(expr_from_proto)
                .collect::<Result<_, _>>()?,
        ),
        Node::Negation(inner) => Expr::Not(Box::new(expr_from_proto(inner)?)),
    })
}

// --- queries --------------------------------------------------------------

/// A query in its wire form, over `table`.
///
/// Takes the table rather than just its name so that an access hint can be
/// written as an index *name*: an [`IndexId`](slate_schema::IndexId) means
/// nothing outside the catalog that issued it, and a wire format carrying one
/// would break the first time a client and a server were built from different
/// versions of the schema.
#[must_use]
pub fn query_to_proto(table: &TableDef, query: &Query) -> pb::Query {
    pb::Query {
        table: table.name().to_owned(),
        filter: Some(expr_to_proto(&query.filter)),
        order: match query.order {
            ScanOrder::Ascending => pb::ScanOrder::Ascending as i32,
            ScanOrder::Descending => pb::ScanOrder::Descending as i32,
        },
        projection: Some(match &query.projection {
            Projection::All => pb::Projection {
                all_columns: true,
                columns: Vec::new(),
            },
            Projection::Columns(columns) => pb::Projection {
                all_columns: false,
                columns: columns.iter().map(|c| ordinal_to_proto(*c)).collect(),
            },
        }),
        sort: query
            .sort
            .iter()
            .map(|key| pb::SortKey {
                column: ordinal_to_proto(key.column),
                direction: match key.direction {
                    Direction::Asc => pb::SortDirection::Asc as i32,
                    Direction::Desc => pb::SortDirection::Desc as i32,
                },
                nulls: match key.nulls {
                    NullsOrder::First => pb::NullsOrder::First as i32,
                    NullsOrder::Last => pb::NullsOrder::Last as i32,
                },
            })
            .collect(),
        limit: query.limit.map(|limit| limit as u64),
        offset: query.offset as u64,
        hint: query.hint.map(|hint| pb::AccessHint {
            path: Some(match hint {
                AccessHint::TableScan => pb::access_hint::Path::TableScan(true),
                AccessHint::Index(id) => pb::access_hint::Path::Index(
                    table
                        .index(id)
                        .map_or_else(|| format!("#{}", id.0), |index| index.name().to_owned()),
                ),
            }),
        }),
        // Computed columns are not on the wire; see the module docs.
    }
}

/// A wire query as the kernel's, checked against `table`.
///
/// Returns the query and anything the server did that the request did not ask
/// for — currently only an ignored hint. Warnings rather than errors because
/// the kernel's rule is that a hint is advice: a query that stops working
/// because an index was renamed is worse than one that gets slower. But a hint
/// silently doing nothing is undebuggable, so it is reported.
pub fn query_from_proto(
    query: &pb::Query,
    table: &TableDef,
) -> Result<(Query, Vec<String>), Status> {
    let mut warnings = Vec::new();
    let width = table.columns().len();

    let filter = match &query.filter {
        Some(filter) => expr_from_proto(filter)?,
        None => Expr::True,
    };
    for column in filter.columns() {
        check_ordinal(column, width, table, "the filter")?;
    }

    let order = match pb::ScanOrder::try_from(query.order) {
        Ok(pb::ScanOrder::Ascending) => ScanOrder::Ascending,
        Ok(pb::ScanOrder::Descending) => ScanOrder::Descending,
        Err(_) => {
            return Err(bad(format!(
                "scan order {} is not one this server knows",
                query.order
            )));
        }
    };

    let projection = match &query.projection {
        None => Projection::All,
        Some(projection) if projection.all_columns => Projection::All,
        Some(projection) => {
            let mut columns = Vec::with_capacity(projection.columns.len());
            for column in &projection.columns {
                let ordinal = ordinal_from_proto(*column);
                check_ordinal(ordinal, width, table, "the projection")?;
                columns.push(ordinal);
            }
            Projection::Columns(columns)
        }
    };

    let mut sort = Vec::with_capacity(query.sort.len());
    for key in &query.sort {
        let column = ordinal_from_proto(key.column);
        check_ordinal(column, width, table, "the sort")?;
        let direction = match pb::SortDirection::try_from(key.direction) {
            Ok(pb::SortDirection::Asc) => Direction::Asc,
            Ok(pb::SortDirection::Desc) => Direction::Desc,
            Err(_) => {
                return Err(bad(format!(
                    "sort direction {} is not one this server knows",
                    key.direction
                )));
            }
        };
        let nulls = match pb::NullsOrder::try_from(key.nulls) {
            Ok(pb::NullsOrder::First) => NullsOrder::First,
            Ok(pb::NullsOrder::Last) => NullsOrder::Last,
            // Unspecified is the natural place for the direction, which is
            // where the storage already puts them — the only arrangement an
            // index can serve without materialising the whole result.
            Ok(pb::NullsOrder::Unspecified) | Err(_) => NullsOrder::natural_for(direction),
        };
        sort.push(SortKey {
            column,
            direction,
            nulls,
        });
    }

    let hint = match query.hint.as_ref().and_then(|hint| hint.path.as_ref()) {
        None => None,
        Some(pb::access_hint::Path::TableScan(true)) => Some(AccessHint::TableScan),
        Some(pb::access_hint::Path::TableScan(false)) => None,
        Some(pb::access_hint::Path::Index(name)) => match table.index_by_name(name) {
            Some(index) => Some(AccessHint::Index(index.id())),
            None => {
                warnings.push(format!(
                    "hint ignored: table `{}` has no index named `{name}`",
                    table.name()
                ));
                None
            }
        },
    };

    Ok((
        Query {
            filter,
            order,
            projection,
            sort,
            limit: query.limit.map(|limit| limit as usize),
            offset: query.offset as usize,
            hint,
            compute: Vec::new(),
        },
        warnings,
    ))
}

/// Refuse an ordinal that is not a column of the table.
///
/// The kernel treats a missing column as unknown rather than as an error,
/// which is right for evaluation — a joined row genuinely has columns some
/// predicates do not reach. At the wire boundary it is a client bug, and one
/// worth naming: a predicate on ordinal 12 of a nine-column table silently
/// matches nothing, and nothing else in the system will ever say why.
fn check_ordinal(
    ordinal: Ordinal,
    width: usize,
    table: &TableDef,
    what: &str,
) -> Result<(), Status> {
    if ordinal.0 < width {
        return Ok(());
    }
    Err(bad(format!(
        "{what} names column {} of table `{}`, which has {width} columns",
        ordinal.0,
        table.name()
    )))
}

// --- freshness ------------------------------------------------------------

/// How fresh a read has to be. An absent message means [`Freshness::Any`],
/// which is the kernel's default and the cheapest read.
pub fn freshness_from_proto(freshness: Option<&pb::Freshness>) -> Result<Freshness, Status> {
    let Some(level) = freshness.and_then(|f| f.level.as_ref()) else {
        return Ok(Freshness::Any);
    };
    Ok(match level {
        pb::freshness::Level::Any(_) => Freshness::Any,
        pb::freshness::Level::AtLeast(sequence) => Freshness::AtLeast(ReadToken::new(*sequence)),
        pb::freshness::Level::Latest(true) => Freshness::Latest,
        // `latest: false` is what a client that zeroed the field sends. It is
        // not a request for the writer, and reading it as one would send every
        // such read to the scarcest resource in the deployment.
        pb::freshness::Level::Latest(false) => Freshness::Any,
    })
}

/// A freshness in its wire form.
#[must_use]
pub const fn freshness_to_proto(freshness: Freshness) -> pb::Freshness {
    let level = match freshness {
        Freshness::Any => pb::freshness::Level::Any(true),
        Freshness::AtLeast(token) => pb::freshness::Level::AtLeast(token.sequence()),
        Freshness::Latest => pb::freshness::Level::Latest(true),
    };
    pb::Freshness { level: Some(level) }
}

// --- explanations ---------------------------------------------------------

/// An explanation in its wire form.
#[must_use]
pub fn explanation_to_proto(
    explanation: &Explanation,
    warnings: Vec<String>,
    served_by: Option<pb::ServedBy>,
) -> pb::ExplainResponse {
    pb::ExplainResponse {
        table: explanation.table.clone(),
        access: explanation.access.to_string(),
        residual: explanation.residual.clone(),
        descending: explanation.order == ScanOrder::Descending,
        estimated_rows: explanation.estimated_rows,
        estimated_cost: explanation.estimated_cost,
        limit: explanation.limit.map(|limit| limit as u64),
        offset: explanation.offset as u64,
        sorts: explanation.sorts,
        index_only: explanation.is_index_only(),
        display: explanation.to_string(),
        warnings,
        served_by,
    }
}
