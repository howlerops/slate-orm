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
//! # One ordinal space, resolved here rather than by the client
//!
//! The kernel addresses everything by a flat ordinal: a joined row packs its
//! tables by width, a computed value sits after its table's own columns, a
//! group is its keys followed by its aggregates. Three shapes, one arithmetic
//! — and every term of that arithmetic is a table width, which is exactly what
//! this protocol refuses to publish.
//!
//! So the wire does not carry flat ordinals. It carries [`pb::ColumnRef`],
//! which names a producer and an index inside it, and [`Space`] turns one into
//! the ordinal the kernel wants. The client never adds a width to anything, and
//! a column added to an early table cannot silently re-point a predicate over a
//! later one. The `.proto` records the alternatives that were rejected.
//!
//! Two refusals fall out of the same choice rather than needing a rule of their
//! own: a `HAVING` naming an ungrouped column, and a cross-input condition
//! naming a computed value the joined space has no slot for. Both are *kind*
//! mismatches here, where a flat ordinal would have made them indistinguishable
//! from a legitimate reference that happened to land in range.

use crate::fingerprint;
use crate::proto as pb;
use crate::session::{GroupedExplanation, MultiRow};
use slate_kernel::query::{AccessHint, NullsOrder, Query, SortKey};
use slate_kernel::{
    Aggregate, CalendarPart, CalendarUnit, CmpOp, DEFAULT_BUILD_LIMIT, Explanation, Expr,
    Freshness, Group, Grouping, Join, JoinAlgorithm, JoinExplanation, JoinKey, JoinStep, JoinType,
    Metric, Projection, ReadToken, ScanOrder, Side, TimeUnit,
};
use slate_kernel::{Chain, ChainPlan, ChainRow, JoinSchema, Scalar};
use slate_schema::{Catalog, Ordinal, Row, TableDef, TableId};
use slate_tuple::{Direction, Value};
use tonic::Status;
use uuid::Uuid;

/// A client sent something this server cannot read.
fn bad(message: impl Into<String>) -> Status {
    Status::invalid_argument(message)
}

/// Check a `oneof` arm that carries no payload.
///
/// [`pb::Unit`] has exactly one value, which is the whole point of it: the arm
/// is selected or it is not, and there is no second spelling meaning "selected
/// but no". Any other value is a client built against a schema this server
/// does not have, and is refused rather than read as `UNIT` — the same rule
/// every other enum here follows.
fn unit(value: i32, what: &str) -> Result<(), Status> {
    if pb::Unit::try_from(value) == Ok(pb::Unit::Unit) {
        return Ok(());
    }
    Err(bad(format!(
        "{what} carries {value}, and the only value it can carry is UNIT"
    )))
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

/// A stored row in its wire form: no computed values, so no split to make.
///
/// For a row that came out of a query that computed values, use
/// [`row_to_proto_split`] — this one would send them as if they were columns.
#[must_use]
pub fn row_to_proto(row: &Row) -> pb::Row {
    pb::Row {
        values: row.values().iter().map(value_to_proto).collect(),
        computed: Vec::new(),
    }
}

/// A row in its wire form, with the values past `stored` sent as computed
/// values rather than as columns.
///
/// The kernel's row is flat: a query's computed values sit after its table's
/// own columns, at `width + i`, which is where `Query::computed` puts them.
/// The wire is not, and this is where the two part company. Sending the flat
/// row would have made the *client* compute `width + i` to read a computed
/// value — from a width this protocol does not publish, and one that moves the
/// day a column is added to the table. That is exactly the arithmetic
/// `ColumnRef` removed from the request side, and it was still here on the way
/// back; `JoinedRow` and `Group` had already been split for the same reason.
///
/// `stored` is the table's declared width, taken from the catalog rather than
/// from the row, and the split is saturating: a row shorter than its table
/// (which nothing produces today) comes back with no computed values rather
/// than panicking, because a head node should not be able to be brought down
/// by a row it can describe.
#[must_use]
pub fn row_to_proto_split(row: &Row, stored: usize) -> pb::Row {
    let values = row.values();
    let at = stored.min(values.len());
    let (columns, computed) = values.split_at(at);
    pb::Row {
        values: columns.iter().map(value_to_proto).collect(),
        computed: computed.iter().map(value_to_proto).collect(),
    }
}

/// The values of a wire row, without checking them against any table.
///
/// Refuses a row that carries computed values, which no request may: a
/// computed value is produced by a read and is not something a client can
/// store or look a row up by. Dropping them silently would let an insert
/// appear to accept values it discarded.
pub fn values_from_proto(row: &pb::Row) -> Result<Vec<Value>, Status> {
    if !row.computed.is_empty() {
        return Err(bad(format!(
            "a row in this request carries {} computed value(s); computed values are \
             produced by a read and cannot be written or looked up by",
            row.computed.len()
        )));
    }
    row.values.iter().map(value_from_proto).collect()
}

/// A wire row as the kernel's.
///
/// The width is not checked here; [`Row::validate`] does that against the
/// table, and it produces the better message.
pub fn row_from_proto(row: &pb::Row) -> Result<Row, Status> {
    Ok(Row::new(values_from_proto(row)?))
}

/// A wire row as the kernel's flat one: its columns, then its computed values.
///
/// The inverse of [`row_to_proto_split`], and the only place the two lists are
/// put back together. A client never needs this — it reads the two lists as
/// two lists — but the round-trip property in `tests/wire.rs` and the oracle
/// in `tests/multi.rs` compare against kernel rows, which are flat.
pub fn flat_row_from_proto(row: &pb::Row) -> Result<Row, Status> {
    let mut values = row
        .values
        .iter()
        .map(value_from_proto)
        .collect::<Result<Vec<_>, _>>()?;
    for value in &row.computed {
        values.push(value_from_proto(value)?);
    }
    Ok(Row::new(values))
}

/// The primary key of `table` from a wire row, checked against the key it
/// declares.
///
/// The check is the point. `GetRequest.primary_key` and
/// `DeleteRequest.primary_keys` used to be decoded and encoded straight into a
/// lookup, so a key of the wrong arity — or of the right arity and the wrong
/// integer width, which encodes to different bytes because the tuple codec is
/// type-first — found nothing and came back as `found: false`. That answer is
/// *deliberately* indistinguishable from "a row your policy hides", which is
/// right and is why it must not also mean "your key was malformed": a client
/// bug, a legitimate miss and an authorisation outcome were one answer. The
/// write path already refused by name; these two now do too.
pub fn primary_key_from_proto(row: &pb::Row, table: &TableDef) -> Result<Vec<Value>, Status> {
    let values = values_from_proto(row)?;
    let types = table.primary_key_types();
    if values.len() != types.len() {
        return Err(bad(format!(
            "table `{}` has a primary key of {} column(s) ({}), and this key has {}",
            table.name(),
            types.len(),
            key_columns(table),
            values.len()
        )));
    }
    for (at, (value, expected)) in values.iter().zip(&types).enumerate() {
        match value.value_type() {
            Some(actual) if actual == *expected => {}
            // A null cannot be in a primary key, and a value of the wrong type
            // encodes to a different key rather than to a coerced one — the
            // tuple codec orders type first, which is what makes it sortable.
            // Either way the lookup would be a guaranteed miss.
            _ => {
                return Err(bad(format!(
                    "column {at} of table `{}`'s primary key ({}) expects {expected}, \
                     got {}",
                    table.name(),
                    key_columns(table),
                    value.type_name()
                )));
            }
        }
    }
    Ok(values)
}

/// A table's key columns, named, for a message about one.
fn key_columns(table: &TableDef) -> String {
    table
        .primary_key()
        .iter()
        .map(|ordinal| {
            table
                .columns()
                .get(ordinal.0)
                .map_or("?", |column| column.name())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// --- column references ----------------------------------------------------

/// One input of a read: the table it reads, and how many values it computes.
#[derive(Debug, Clone, Copy)]
pub struct Input<'t> {
    table: &'t TableDef,
    computed: usize,
}

impl<'t> Input<'t> {
    /// An input reading `table` and computing `computed` extra values.
    #[must_use]
    pub const fn new(table: &'t TableDef, computed: usize) -> Self {
        Self { table, computed }
    }

    fn width(self) -> usize {
        self.table.columns().len()
    }
}

/// The shape of the rows a predicate, sort key, projection or aggregate is
/// written against, and the resolution of a [`pb::ColumnRef`] into it.
///
/// Three shapes, because there are three ways a row gets built here, and each
/// is a concatenation of producers. Keeping them one type rather than three is
/// what lets a single [`Expr`] serve all of them — the kernel's arrangement,
/// and the reason three-valued logic has exactly one implementation.
#[derive(Debug)]
pub struct Space<'t> {
    shape: Shape<'t>,
}

#[derive(Debug)]
enum Shape<'t> {
    /// One input, addressed in its own table's ordinals. `index` is the
    /// position a reference has to name — zero for a plain query, and the
    /// input's own position inside a join, so that one numbering serves the
    /// whole request.
    Local { input: Input<'t>, index: usize },
    /// Several inputs, packed exactly as [`JoinSchema`] packs them.
    ///
    /// `visible` is how many of them the expression being converted may name.
    /// A condition on the third input may read the first two and itself; it
    /// may not read the fourth, which has not been read yet and would evaluate
    /// as null rather than failing.
    Joined {
        inputs: Vec<Input<'t>>,
        visible: usize,
        /// How many values the **join itself** computes, and how many of them
        /// this expression may name.
        ///
        /// Two numbers rather than one because a computed value may read the
        /// ones before it and not itself: converting `compute[i]` happens in a
        /// space where `computed` is `i`, so naming itself or a later one lands
        /// on the same refusal as naming one that does not exist. Everything
        /// converted after the join is complete sees all of them.
        computed: usize,
    },
    /// A grouped result: the keys, then the aggregates.
    Groups { keys: usize, aggregates: usize },
}

impl<'t> Space<'t> {
    /// A single-table query with no computed values.
    #[must_use]
    pub const fn table(table: &'t TableDef) -> Self {
        Self::input(table, 0, 0)
    }

    /// The `index`th input of a request, reading `table` and computing
    /// `computed` values.
    #[must_use]
    pub const fn input(table: &'t TableDef, computed: usize, index: usize) -> Self {
        Self {
            shape: Shape::Local {
                input: Input::new(table, computed),
                index,
            },
        }
    }

    /// The joined space over `inputs`, of which the first `visible` may be
    /// named.
    #[must_use]
    pub const fn joined(inputs: Vec<Input<'t>>, visible: usize) -> Self {
        Self {
            shape: Shape::Joined {
                inputs,
                visible,
                computed: 0,
            },
        }
    }

    /// The same, where the join itself computes `computed` values that this
    /// expression may name. See [`Shape::Joined`].
    #[must_use]
    pub const fn joined_computing(inputs: Vec<Input<'t>>, visible: usize, computed: usize) -> Self {
        Self {
            shape: Shape::Joined {
                inputs,
                visible,
                computed,
            },
        }
    }

    /// A grouped result of `keys` grouping columns and `aggregates`
    /// aggregates.
    #[must_use]
    pub const fn groups(keys: usize, aggregates: usize) -> Self {
        Self {
            shape: Shape::Groups { keys, aggregates },
        }
    }

    /// Where each input starts, for a joined space.
    fn offset(inputs: &[Input<'t>], index: usize) -> usize {
        inputs.iter().take(index).map(|i| i.width()).sum()
    }

    /// The ordinal `reference` names, or a refusal saying why it names none.
    ///
    /// `what` is the part of the request being converted, so a message can say
    /// *where* the bad reference was as well as what was wrong with it. A
    /// predicate on a column that does not exist is otherwise a query that
    /// silently matches nothing, which is the one diagnosis nothing else in the
    /// system will ever offer.
    pub fn resolve(
        &self,
        reference: Option<&pb::ColumnRef>,
        what: &str,
    ) -> Result<Ordinal, Status> {
        use pb::column_ref::Of;
        let Some(reference) = reference else {
            return Err(bad(format!("{what} names no column")));
        };
        let Some(of) = &reference.of else {
            // Same reasoning as an unset `Value.kind`: proto3 cannot tell an
            // unset field from a zero one, so defaulting this to "column 0"
            // would turn a client built against a newer schema into a query
            // about the wrong column.
            return Err(bad(format!(
                "{what} has a column reference with no kind set; name a column, \
                 a computed value, a group key or an aggregate"
            )));
        };
        let asked = reference.input as usize;
        match &self.shape {
            Shape::Local { input, index } => match of {
                Of::Column(column) => {
                    Self::check_input(asked, *index, what)?;
                    let column = *column as usize;
                    if column >= input.width() {
                        return Err(bad(format!(
                            "{what} names column {column} of table `{}`, which has {} columns",
                            input.table.name(),
                            input.width()
                        )));
                    }
                    Ok(Ordinal(column))
                }
                Of::Computed(at) => {
                    Self::check_input(asked, *index, what)?;
                    let at = *at as usize;
                    if at >= input.computed {
                        // Also the rule that a computed value may only read
                        // earlier ones: converting the `i`th is done in a space
                        // that knows about `i` of them, so naming itself or a
                        // later one lands here.
                        return Err(bad(format!(
                            "{what} names computed value {at}, and only {} are \
                             available at that point",
                            input.computed
                        )));
                    }
                    Ok(Ordinal(input.width() + at))
                }
                Of::GroupKey(_) | Of::Aggregate(_) => Err(bad(format!(
                    "{what} names a group key or an aggregate, but it is evaluated \
                     over rows rather than over groups"
                ))),
                Of::JoinedComputed(at) => Err(bad(format!(
                    "{what} names the join's computed value {at}, and this reads one \
                     table rather than a joined row"
                ))),
            },
            Shape::Joined {
                inputs,
                visible,
                computed,
            } => match of {
                Of::Column(column) => {
                    if asked >= *visible {
                        return Err(bad(format!(
                            "{what} names input {asked}, which is not read by that point; \
                             {visible} inputs are available there"
                        )));
                    }
                    let input = inputs.get(asked).ok_or_else(|| {
                        bad(format!(
                            "{what} names input {asked}, but the request has {}",
                            inputs.len()
                        ))
                    })?;
                    let column = *column as usize;
                    if column >= input.width() {
                        return Err(bad(format!(
                            "{what} names column {column} of table `{}`, which has {} columns",
                            input.table.name(),
                            input.width()
                        )));
                    }
                    Ok(Ordinal(Self::offset(inputs, asked) + column))
                }
                // The kernel's joined space is packed by *declared* table
                // width, so a computed value — which sits past its table's
                // declared columns — has no slot in it. Reading it as a flat
                // ordinal would land on the next table's first column and
                // answer a different question. It is refused rather than
                // supported, because supporting it is a kernel change.
                Of::Computed(at) => Err(bad(format!(
                    "{what} names computed value {at} of input {asked}; a computed value \
                     is not addressable across inputs, because the joined ordinal space \
                     is packed by declared table width and has no slot for one. Put the \
                     expression in the join's own `compute`, which is evaluated over the \
                     joined row and can read any input, and name it with `joined_computed`."
                ))),
                // A value belonging to the *join* does have a slot: past every
                // input's columns, which is where the join defines the row to
                // end. That is the whole difference between this kind and
                // `computed` above, and the reason both exist.
                Of::JoinedComputed(at) => {
                    Self::check_input(asked, 0, what)?;
                    let at = *at as usize;
                    if at >= *computed {
                        return Err(bad(format!(
                            "{what} names the join's computed value {at}, and {computed} \
                             are available at that point; a computed value may read the \
                             ones before it but not itself or a later one"
                        )));
                    }
                    Ok(Ordinal(Self::offset(inputs, inputs.len()) + at))
                }
                Of::GroupKey(_) | Of::Aggregate(_) => Err(bad(format!(
                    "{what} names a group key or an aggregate, but it is evaluated \
                     over joined rows rather than over groups"
                ))),
            },
            Shape::Groups { keys, aggregates } => match of {
                Of::GroupKey(at) => {
                    Self::check_input(asked, 0, what)?;
                    let at = *at as usize;
                    if at >= *keys {
                        return Err(bad(format!(
                            "{what} names group key {at}, and the query groups by {keys}"
                        )));
                    }
                    Ok(Ordinal(at))
                }
                Of::Aggregate(at) => {
                    Self::check_input(asked, 0, what)?;
                    let at = *at as usize;
                    if at >= *aggregates {
                        return Err(bad(format!(
                            "{what} names aggregate {at}, and the query has {aggregates}"
                        )));
                    }
                    Ok(Ordinal(keys + at))
                }
                // SQL's "column must appear in the GROUP BY clause", and the
                // reason `ColumnRef` distinguishes kinds at all: a flat
                // ordinal here would have been a legitimate group key.
                Of::Column(_) | Of::Computed(_) | Of::JoinedComputed(_) => Err(bad(format!(
                    "{what} names a column that is not grouped; a condition over groups \
                     may only name a group key or an aggregate — a computed value of the \
                     join included, which becomes a group key by appearing in `group_by`"
                ))),
            },
        }
    }

    /// A reference that must name a stored column, for the two places where
    /// nothing else can work: a join equality and a projection.
    ///
    /// A join key is compared against stored values through the index, and a
    /// projection decides which stored columns are decoded — a computed value
    /// is neither, and comes back regardless.
    pub fn resolve_stored_column(
        &self,
        reference: Option<&pb::ColumnRef>,
        what: &str,
    ) -> Result<Ordinal, Status> {
        match reference.and_then(|r| r.of.as_ref()) {
            Some(pb::column_ref::Of::Computed(at)) => {
                return Err(bad(format!(
                    "{what} names computed value {at}; only a stored column can appear there"
                )));
            }
            // A join's computed value is not stored either, and it is *more*
            // obviously not: it does not exist until the pair is formed, so a
            // join key compared against it through an index would be comparing
            // against nothing. Refused by kind rather than by range, for the
            // reason `ColumnRef` has kinds at all.
            Some(pb::column_ref::Of::JoinedComputed(at)) => {
                return Err(bad(format!(
                    "{what} names the join's computed value {at}; only a stored column can \
                     appear there, and a join's computed value does not exist until the \
                     pair it is computed from has been formed"
                )));
            }
            _ => {}
        }
        self.resolve(reference, what)
    }

    fn check_input(asked: usize, expected: usize, what: &str) -> Result<(), Status> {
        if asked == expected {
            return Ok(());
        }
        Err(bad(format!(
            "{what} names input {asked}, and is evaluated over input {expected}"
        )))
    }

    /// The wire form of an ordinal in this space.
    ///
    /// Total, like every outbound conversion. An ordinal this space has no slot
    /// for comes out as a plain column reference, which the inbound direction
    /// then refuses — that keeps "a column past the end of the table" a refusal
    /// rather than a panic on the way out.
    #[must_use]
    pub fn unresolve(&self, ordinal: Ordinal) -> pb::ColumnRef {
        match &self.shape {
            Shape::Local { input, index } => {
                let width = input.width();
                if ordinal.0 >= width && ordinal.0 < width + input.computed {
                    return computed_ref(*index, ordinal.0 - width);
                }
                column_ref(*index, ordinal.0)
            }
            Shape::Joined {
                inputs, computed, ..
            } => {
                let mut at = 0;
                for (index, input) in inputs.iter().enumerate() {
                    if ordinal.0 >= at && ordinal.0 < at + input.width() {
                        return column_ref(index, ordinal.0 - at);
                    }
                    at += input.width();
                }
                // Past every input's columns is the join's own computed space.
                // Without this the ordinal fell through to `column_ref(0, n)` —
                // input 0's column *n*, which on the way back in is a different
                // column or an out-of-range refusal, so a join carrying a
                // computed value could not survive a round trip.
                if ordinal.0 < at + *computed {
                    return joined_computed_ref(ordinal.0 - at);
                }
                column_ref(0, ordinal.0)
            }
            Shape::Groups { keys, .. } => {
                if ordinal.0 < *keys {
                    return reference(pb::column_ref::Of::GroupKey(ordinal.0 as u32));
                }
                reference(pb::column_ref::Of::Aggregate((ordinal.0 - keys) as u32))
            }
        }
    }
}

fn reference(of: pb::column_ref::Of) -> pb::ColumnRef {
    pb::ColumnRef {
        input: 0,
        of: Some(of),
    }
}

/// A reference to a stored column of one input.
#[must_use]
pub fn column_ref(input: usize, ordinal: usize) -> pb::ColumnRef {
    pb::ColumnRef {
        input: input as u32,
        of: Some(pb::column_ref::Of::Column(ordinal as u32)),
    }
}

/// A reference to the `at`th value one input computes.
#[must_use]
pub fn computed_ref(input: usize, at: usize) -> pb::ColumnRef {
    pb::ColumnRef {
        input: input as u32,
        of: Some(pb::column_ref::Of::Computed(at as u32)),
    }
}

/// A reference to the `at`th value the **join itself** computes.
///
/// No input, deliberately: the value belongs to the request rather than to one
/// of its tables. See `ColumnRef.joined_computed` in `records.proto`.
#[must_use]
pub fn joined_computed_ref(at: usize) -> pb::ColumnRef {
    reference(pb::column_ref::Of::JoinedComputed(at as u32))
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

/// A predicate in its wire form, addressed against `space`.
#[must_use]
pub fn expr_to_proto(space: &Space<'_>, expr: &Expr) -> pb::Expr {
    use pb::expr::Node;
    let node = match expr {
        Expr::True => Node::Literal(true),
        Expr::False => Node::Literal(false),
        Expr::Compare { column, op, value } => Node::Compare(pb::Compare {
            column: Some(space.unresolve(*column)),
            op: op_to_proto(*op) as i32,
            value: Some(value_to_proto(value)),
        }),
        Expr::CompareColumns { left, op, right } => Node::CompareColumns(pb::CompareColumns {
            left: Some(space.unresolve(*left)),
            op: op_to_proto(*op) as i32,
            right: Some(space.unresolve(*right)),
        }),
        Expr::IsNull { column, negated } => Node::IsNull(pb::IsNull {
            column: Some(space.unresolve(*column)),
            negated: *negated,
        }),
        Expr::Like {
            column,
            pattern,
            negated,
            insensitive,
        } => Node::Like(pb::Like {
            column: Some(space.unresolve(*column)),
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
            column: Some(space.unresolve(*column)),
            pattern: pattern.clone(),
            negated: *negated,
            insensitive: *insensitive,
        }),
        Expr::In { column, values } => Node::InList(pb::InList {
            column: Some(space.unresolve(*column)),
            values: values.iter().map(value_to_proto).collect(),
        }),
        Expr::And(parts) => Node::Conjunction(pb::ExprList {
            exprs: parts.iter().map(|e| expr_to_proto(space, e)).collect(),
        }),
        Expr::Or(parts) => Node::Disjunction(pb::ExprList {
            exprs: parts.iter().map(|e| expr_to_proto(space, e)).collect(),
        }),
        Expr::Not(inner) => Node::Negation(Box::new(expr_to_proto(space, inner))),
        // `Expr` is `#[non_exhaustive]`. A predicate this server cannot
        // represent must not become `True`, which would widen the result — and
        // on a table under a policy, widening is the failure that matters. It
        // becomes `False`, which is the safe direction, and the caller sees an
        // empty result rather than someone else's rows.
        _ => Node::Literal(false),
    };
    pb::Expr { node: Some(node) }
}

/// A wire predicate as the kernel's, resolved against `space`.
pub fn expr_from_proto(space: &Space<'_>, expr: &pb::Expr) -> Result<Expr, Status> {
    expr_named(space, expr, "the predicate")
}

/// [`expr_from_proto`], with a name for the part of the request it came from.
pub fn expr_named(space: &Space<'_>, expr: &pb::Expr, what: &str) -> Result<Expr, Status> {
    use pb::expr::Node;
    let Some(node) = &expr.node else {
        return Err(bad(format!("{what} has an expression with no node set")));
    };
    Ok(match node {
        Node::Literal(true) => Expr::True,
        Node::Literal(false) => Expr::False,
        Node::Compare(compare) => Expr::Compare {
            column: space.resolve(compare.column.as_ref(), what)?,
            op: op_from_proto(compare.op)?,
            value: match &compare.value {
                Some(value) => value_from_proto(value)?,
                None => return Err(bad(format!("{what} has a comparison with no value"))),
            },
        },
        Node::CompareColumns(compare) => Expr::CompareColumns {
            left: space.resolve(compare.left.as_ref(), what)?,
            op: op_from_proto(compare.op)?,
            right: space.resolve(compare.right.as_ref(), what)?,
        },
        Node::IsNull(is_null) => Expr::IsNull {
            column: space.resolve(is_null.column.as_ref(), what)?,
            negated: is_null.negated,
        },
        Node::Like(like) => Expr::Like {
            column: space.resolve(like.column.as_ref(), what)?,
            pattern: like.pattern.clone(),
            negated: like.negated,
            insensitive: like.insensitive,
        },
        Node::Matches(matches) => Expr::Matches {
            column: space.resolve(matches.column.as_ref(), what)?,
            pattern: matches.pattern.clone(),
            negated: matches.negated,
            insensitive: matches.insensitive,
        },
        Node::InList(in_list) => Expr::In {
            column: space.resolve(in_list.column.as_ref(), what)?,
            values: in_list
                .values
                .iter()
                .map(value_from_proto)
                .collect::<Result<_, _>>()?,
        },
        Node::Conjunction(list) => Expr::And(
            list.exprs
                .iter()
                .map(|e| expr_named(space, e, what))
                .collect::<Result<_, _>>()?,
        ),
        Node::Disjunction(list) => Expr::Or(
            list.exprs
                .iter()
                .map(|e| expr_named(space, e, what))
                .collect::<Result<_, _>>()?,
        ),
        Node::Negation(inner) => Expr::Not(Box::new(expr_named(space, inner, what)?)),
    })
}

// --- computed values ------------------------------------------------------

const fn unit_to_proto(unit: TimeUnit) -> pb::TimeUnit {
    match unit {
        TimeUnit::Second => pb::TimeUnit::Second,
        TimeUnit::Minute => pb::TimeUnit::Minute,
        TimeUnit::Hour => pb::TimeUnit::Hour,
        TimeUnit::Day => pb::TimeUnit::Day,
    }
}

fn unit_from_proto(unit: i32) -> Result<TimeUnit, Status> {
    match pb::TimeUnit::try_from(unit) {
        Ok(pb::TimeUnit::Second) => Ok(TimeUnit::Second),
        Ok(pb::TimeUnit::Minute) => Ok(TimeUnit::Minute),
        Ok(pb::TimeUnit::Hour) => Ok(TimeUnit::Hour),
        Ok(pb::TimeUnit::Day) => Ok(TimeUnit::Day),
        // There is no unit that is a safe guess: truncating to the wrong one
        // returns a plausible timestamp for a different question.
        Ok(pb::TimeUnit::Unspecified) | Err(_) => Err(bad(format!(
            "time unit {unit} is not one this server knows"
        ))),
    }
}

const fn part_to_proto(part: CalendarPart) -> pb::CalendarPart {
    match part {
        CalendarPart::Year => pb::CalendarPart::Year,
        CalendarPart::Month => pb::CalendarPart::Month,
        CalendarPart::DayOfMonth => pb::CalendarPart::DayOfMonth,
        CalendarPart::DayOfWeek => pb::CalendarPart::DayOfWeek,
    }
}

fn part_from_proto(part: i32) -> Result<CalendarPart, Status> {
    match pb::CalendarPart::try_from(part) {
        Ok(pb::CalendarPart::Year) => Ok(CalendarPart::Year),
        Ok(pb::CalendarPart::Month) => Ok(CalendarPart::Month),
        Ok(pb::CalendarPart::DayOfMonth) => Ok(CalendarPart::DayOfMonth),
        Ok(pb::CalendarPart::DayOfWeek) => Ok(CalendarPart::DayOfWeek),
        // No field is a safe guess, for the reason no time unit is: every one
        // of them returns a plausible small integer for a different question.
        Ok(pb::CalendarPart::Unspecified) | Err(_) => Err(bad(format!(
            "calendar part {part} is not one this server knows"
        ))),
    }
}

const fn calendar_unit_to_proto(unit: CalendarUnit) -> pb::CalendarUnit {
    match unit {
        CalendarUnit::Month => pb::CalendarUnit::Month,
        CalendarUnit::Year => pb::CalendarUnit::Year,
    }
}

/// A zone name the kernel's table has, or a refusal that names the ones it does.
///
/// Refused at the boundary rather than evaluated, because `Scalar::ZoneShift`
/// answers null for a zone it does not know and null is the wrong answer to
/// give a caller who typed `america/new_york`: it looks like the column was
/// empty. The kernel has nowhere to put an error — the edge does.
///
/// The list comes from `zones::listing()` rather than being written out here,
/// so this message and the SQL front end's cannot drift apart. They did once:
/// two of three edges went on saying "there is no timezone database here" after
/// one of them had grown an offset.
fn zone_from_proto(zone: &str, what: &str) -> Result<String, Status> {
    if zone.is_empty() {
        return Err(bad(format!(
            "{what} has a zone shift with no zone name; the zones this knows \
             are {}",
            slate_kernel::zones::listing()
        )));
    }
    if !slate_kernel::zones::has(zone) {
        return Err(bad(format!(
            "{what} names the timezone {zone:?}, which this does not have. \
             IANA names are case-sensitive, and the zones this knows are {}",
            slate_kernel::zones::listing()
        )));
    }
    Ok(zone.to_owned())
}

fn calendar_unit_from_proto(unit: i32) -> Result<CalendarUnit, Status> {
    match pb::CalendarUnit::try_from(unit) {
        Ok(pb::CalendarUnit::Month) => Ok(CalendarUnit::Month),
        Ok(pb::CalendarUnit::Year) => Ok(CalendarUnit::Year),
        // Neither is a safe guess: truncating to a year where a month was
        // meant returns a timestamp, in the right column, twelve times too
        // coarse.
        Ok(pb::CalendarUnit::Unspecified) | Err(_) => Err(bad(format!(
            "calendar unit {unit} is not one this server knows"
        ))),
    }
}

const fn metric_to_proto(metric: Metric) -> pb::Metric {
    match metric {
        Metric::L2 => pb::Metric::L2,
        Metric::L2Squared => pb::Metric::L2Squared,
        Metric::Cosine => pb::Metric::Cosine,
        Metric::NegativeInnerProduct => pb::Metric::NegativeInnerProduct,
    }
}

fn metric_from_proto(metric: i32) -> Result<Metric, Status> {
    match pb::Metric::try_from(metric) {
        Ok(pb::Metric::L2) => Ok(Metric::L2),
        Ok(pb::Metric::L2Squared) => Ok(Metric::L2Squared),
        Ok(pb::Metric::Cosine) => Ok(Metric::Cosine),
        Ok(pb::Metric::NegativeInnerProduct) => Ok(Metric::NegativeInnerProduct),
        // Cosine and L2 rank differently, so a default would silently answer a
        // different nearest-neighbour question.
        Ok(pb::Metric::Unspecified) | Err(_) => Err(bad(format!(
            "distance metric {metric} is not one this server knows"
        ))),
    }
}

/// A computed value in its wire form.
///
/// The match is exhaustive on purpose. [`Scalar`] is not `#[non_exhaustive]`,
/// so a new kernel variant fails to compile here rather than silently going
/// unrepresentable on the wire — the same reason the kernel's own aggregate
/// oracle spells out every `Aggregate`.
#[must_use]
pub fn scalar_to_proto(space: &Space<'_>, scalar: &Scalar) -> pb::Scalar {
    use pb::scalar::Node;
    let pair = |left: &Scalar, right: &Scalar| {
        Box::new(pb::ScalarPair {
            left: Some(Box::new(scalar_to_proto(space, left))),
            right: Some(Box::new(scalar_to_proto(space, right))),
        })
    };
    let node = match scalar {
        Scalar::Column(ordinal) => Node::Column(space.unresolve(*ordinal)),
        Scalar::Literal(value) => Node::Literal(value_to_proto(value)),
        Scalar::Add(a, b) => Node::Add(pair(a, b)),
        Scalar::Sub(a, b) => Node::Sub(pair(a, b)),
        Scalar::Mul(a, b) => Node::Mul(pair(a, b)),
        Scalar::Div(a, b) => Node::Div(pair(a, b)),
        Scalar::Length(inner) => Node::Length(Box::new(scalar_to_proto(space, inner))),
        Scalar::Concat(parts) => Node::Concat(pb::ScalarList {
            scalars: parts.iter().map(|s| scalar_to_proto(space, s)).collect(),
        }),
        Scalar::Lower(inner) => Node::Lower(Box::new(scalar_to_proto(space, inner))),
        Scalar::Upper(inner) => Node::Upper(Box::new(scalar_to_proto(space, inner))),
        Scalar::Extract { unit, value } => Node::Extract(Box::new(pb::TimePart {
            unit: unit_to_proto(*unit) as i32,
            value: Some(Box::new(scalar_to_proto(space, value))),
        })),
        Scalar::DateTrunc { unit, value } => Node::DateTrunc(Box::new(pb::TimePart {
            unit: unit_to_proto(*unit) as i32,
            value: Some(Box::new(scalar_to_proto(space, value))),
        })),
        Scalar::CalendarPart { part, value } => Node::CalendarPart(Box::new(pb::CalendarField {
            part: part_to_proto(*part) as i32,
            value: Some(Box::new(scalar_to_proto(space, value))),
        })),
        Scalar::CalendarTrunc { unit, value } => Node::CalendarTrunc(Box::new(pb::CalendarTrunc {
            unit: calendar_unit_to_proto(*unit) as i32,
            value: Some(Box::new(scalar_to_proto(space, value))),
        })),
        Scalar::ZoneShift { zone, value } => Node::ZoneShift(Box::new(pb::ZoneShift {
            zone: zone.clone(),
            value: Some(Box::new(scalar_to_proto(space, value))),
        })),
        Scalar::Round(inner) => Node::Round(Box::new(scalar_to_proto(space, inner))),
        Scalar::Case {
            branches,
            otherwise,
        } => Node::Case(Box::new(pb::Case {
            branches: branches
                .iter()
                .map(|(when, then)| pb::CaseBranch {
                    when: Some(expr_to_proto(space, when)),
                    then: Some(scalar_to_proto(space, then)),
                })
                .collect(),
            otherwise: Some(Box::new(scalar_to_proto(space, otherwise))),
        })),
        Scalar::Coalesce(parts) => Node::Coalesce(pb::ScalarList {
            scalars: parts.iter().map(|s| scalar_to_proto(space, s)).collect(),
        }),
        Scalar::Distance {
            left,
            right,
            metric,
        } => Node::Distance(Box::new(pb::Distance {
            left: Some(Box::new(scalar_to_proto(space, left))),
            right: Some(Box::new(scalar_to_proto(space, right))),
            metric: metric_to_proto(*metric) as i32,
        })),
        Scalar::RegexpReplace {
            value,
            pattern,
            replacement,
        } => Node::RegexpReplace(Box::new(pb::RegexpReplace {
            value: Some(Box::new(scalar_to_proto(space, value))),
            pattern: pattern.clone(),
            replacement: replacement.clone(),
        })),
    };
    pb::Scalar { node: Some(node) }
}

/// A wire computed value as the kernel's, resolved against `space`.
pub fn scalar_from_proto(space: &Space<'_>, scalar: &pb::Scalar) -> Result<Scalar, Status> {
    scalar_named(space, scalar, "a computed value")
}

pub(crate) fn scalar_named(
    space: &Space<'_>,
    scalar: &pb::Scalar,
    what: &str,
) -> Result<Scalar, Status> {
    use pb::scalar::Node;
    let Some(node) = &scalar.node else {
        return Err(bad(format!("{what} arrived with no node set")));
    };
    let one = |inner: &Option<Box<pb::Scalar>>| -> Result<Box<Scalar>, Status> {
        match inner {
            Some(inner) => Ok(Box::new(scalar_named(space, inner, what)?)),
            None => Err(bad(format!("{what} is missing an operand"))),
        }
    };
    let pair = |p: &pb::ScalarPair| -> Result<(Box<Scalar>, Box<Scalar>), Status> {
        Ok((one(&p.left)?, one(&p.right)?))
    };
    Ok(match node {
        Node::Column(reference) => Scalar::Column(space.resolve(Some(reference), what)?),
        Node::Literal(value) => Scalar::Literal(value_from_proto(value)?),
        Node::Add(p) => {
            let (a, b) = pair(p)?;
            Scalar::Add(a, b)
        }
        Node::Sub(p) => {
            let (a, b) = pair(p)?;
            Scalar::Sub(a, b)
        }
        Node::Mul(p) => {
            let (a, b) = pair(p)?;
            Scalar::Mul(a, b)
        }
        Node::Div(p) => {
            let (a, b) = pair(p)?;
            Scalar::Div(a, b)
        }
        Node::Length(inner) => Scalar::Length(Box::new(scalar_named(space, inner, what)?)),
        Node::Concat(list) => Scalar::Concat(
            list.scalars
                .iter()
                .map(|s| scalar_named(space, s, what))
                .collect::<Result<_, _>>()?,
        ),
        Node::Lower(inner) => Scalar::Lower(Box::new(scalar_named(space, inner, what)?)),
        Node::Upper(inner) => Scalar::Upper(Box::new(scalar_named(space, inner, what)?)),
        Node::Extract(part) => Scalar::Extract {
            unit: unit_from_proto(part.unit)?,
            value: one(&part.value)?,
        },
        Node::DateTrunc(part) => Scalar::DateTrunc {
            unit: unit_from_proto(part.unit)?,
            value: one(&part.value)?,
        },
        Node::CalendarPart(field) => Scalar::CalendarPart {
            part: part_from_proto(field.part)?,
            value: one(&field.value)?,
        },
        Node::CalendarTrunc(trunc) => Scalar::CalendarTrunc {
            unit: calendar_unit_from_proto(trunc.unit)?,
            value: one(&trunc.value)?,
        },
        Node::ZoneShift(shift) => Scalar::ZoneShift {
            zone: zone_from_proto(&shift.zone, what)?,
            value: one(&shift.value)?,
        },
        Node::Round(inner) => Scalar::Round(Box::new(scalar_named(space, inner, what)?)),
        Node::Case(case) => {
            let mut branches = Vec::with_capacity(case.branches.len());
            for branch in &case.branches {
                let when = match &branch.when {
                    Some(when) => expr_named(space, when, what)?,
                    None => return Err(bad(format!("{what} has a CASE branch with no condition"))),
                };
                let then = match &branch.then {
                    Some(then) => scalar_named(space, then, what)?,
                    None => return Err(bad(format!("{what} has a CASE branch with no result"))),
                };
                branches.push((when, then));
            }
            Scalar::Case {
                branches,
                // Required rather than defaulted to null: `CASE` with no
                // `ELSE` is written by sending an explicit null literal, so an
                // absent field is a client that forgot rather than one that
                // meant null.
                otherwise: one(&case.otherwise)?,
            }
        }
        Node::Coalesce(list) => Scalar::Coalesce(
            list.scalars
                .iter()
                .map(|s| scalar_named(space, s, what))
                .collect::<Result<_, _>>()?,
        ),
        Node::Distance(distance) => Scalar::Distance {
            left: one(&distance.left)?,
            right: one(&distance.right)?,
            metric: metric_from_proto(distance.metric)?,
        },
        Node::RegexpReplace(replace) => Scalar::RegexpReplace {
            value: one(&replace.value)?,
            pattern: replace.pattern.clone(),
            replacement: replace.replacement.clone(),
        },
    })
}

// --- aggregates -----------------------------------------------------------

/// An aggregate in its wire form.
///
/// Exhaustive for the same reason [`scalar_to_proto`] is: a new `Aggregate`
/// variant should fail to compile here rather than reach the wire as something
/// else.
#[must_use]
pub fn aggregate_to_proto(space: &Space<'_>, aggregate: Aggregate) -> pb::Aggregate {
    use pb::AggregateFunction as F;
    let (function, column) = match aggregate {
        Aggregate::Count => (F::Count, None),
        Aggregate::CountColumn(c) => (F::CountColumn, Some(c)),
        Aggregate::Min(c) => (F::Min, Some(c)),
        Aggregate::Max(c) => (F::Max, Some(c)),
        Aggregate::Sum(c) => (F::Sum, Some(c)),
        Aggregate::Avg(c) => (F::Avg, Some(c)),
        Aggregate::CountDistinct(c) => (F::CountDistinct, Some(c)),
    };
    pb::Aggregate {
        function: function as i32,
        column: column.map(|c| space.unresolve(c)),
    }
}

/// A wire aggregate as the kernel's.
pub fn aggregate_from_proto(
    space: &Space<'_>,
    aggregate: &pb::Aggregate,
) -> Result<Aggregate, Status> {
    use pb::AggregateFunction as F;
    let what = "an aggregate";
    let function = pb::AggregateFunction::try_from(aggregate.function).map_err(|_| {
        bad(format!(
            "aggregate function {} is not one this server knows",
            aggregate.function
        ))
    })?;
    // Resolved once, and only where a column is wanted: `COUNT(*)` with a
    // column set is a client that meant `COUNT(column)`, which is a different
    // number on a nullable column, so it is refused rather than ignored.
    let column = |required: bool| -> Result<Option<Ordinal>, Status> {
        match (&aggregate.column, required) {
            (Some(reference), true) => Ok(Some(space.resolve(Some(reference), what)?)),
            (None, true) => Err(bad(
                "an aggregate other than COUNT(*) needs the column it reads",
            )),
            (Some(_), false) => Err(bad(
                "COUNT(*) reads no column; use COUNT_COLUMN to count non-null values of one",
            )),
            (None, false) => Ok(None),
        }
    };
    Ok(match function {
        F::Count => {
            column(false)?;
            Aggregate::Count
        }
        F::CountColumn => Aggregate::CountColumn(required(column(true)?)?),
        F::Min => Aggregate::Min(required(column(true)?)?),
        F::Max => Aggregate::Max(required(column(true)?)?),
        F::Sum => Aggregate::Sum(required(column(true)?)?),
        F::Avg => Aggregate::Avg(required(column(true)?)?),
        F::CountDistinct => Aggregate::CountDistinct(required(column(true)?)?),
        // Defaulting would return a plausible number for a question nobody
        // asked, which is the worst shape an aggregate bug can take.
        F::Unspecified => {
            return Err(bad("an aggregate arrived with no function set"));
        }
    })
}

fn required(column: Option<Ordinal>) -> Result<Ordinal, Status> {
    column.ok_or_else(|| bad("an aggregate is missing the column it reads"))
}

/// A group in its wire form.
#[must_use]
pub fn group_to_proto(group: &Group) -> pb::Group {
    pb::Group {
        key: group.key.iter().map(value_to_proto).collect(),
        values: group.values.iter().map(value_to_proto).collect(),
    }
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
    query_to_proto_at(table, query, 0)
}

/// [`query_to_proto`] for the `index`th input of a multi-table read.
#[must_use]
pub fn query_to_proto_at(table: &TableDef, query: &Query, index: usize) -> pb::Query {
    // Each computed value is written in the space that exists where it is
    // evaluated: the ones before it, and no more. That is the same rule the
    // inbound direction enforces, stated once on each side.
    let compute = query
        .compute
        .iter()
        .enumerate()
        .map(|(at, scalar)| scalar_to_proto(&Space::input(table, at, index), scalar))
        .collect();
    let space = Space::input(table, query.compute.len(), index);
    pb::Query {
        table: table.name().to_owned(),
        filter: Some(expr_to_proto(&space, &query.filter)),
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
                columns: columns.iter().map(|c| space.unresolve(*c)).collect(),
            },
        }),
        sort: query
            .sort
            .iter()
            .map(|key| pb::SortKey {
                column: Some(space.unresolve(key.column)),
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
                AccessHint::TableScan => pb::access_hint::Path::TableScan(pb::Unit::Unit as i32),
                AccessHint::Index(id) => pb::access_hint::Path::Index(
                    table
                        .index(id)
                        .map_or_else(|| format!("#{}", id.0), |index| index.name().to_owned()),
                ),
            }),
        }),
        compute,
        // Sent, not left absent: this is the reference client for every other
        // one, and a reference that skips the check teaches the check is
        // optional in practice. It also means the oracle in `tests/multi.rs`
        // puts a fingerprint through the server on every query it runs.
        schema: Some(fingerprint::claim(table)),
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
    query_from_proto_at(query, table, 0)
}

/// [`query_from_proto`] for the `index`th input of a multi-table read, whose
/// column references carry that index.
pub fn query_from_proto_at(
    query: &pb::Query,
    table: &TableDef,
    index: usize,
) -> Result<(Query, Vec<String>), Status> {
    // Before anything is resolved. Every ordinal below is only meaningful
    // relative to a declaration, so checking the declaration first is the only
    // order in which the refusal means anything: a reference that resolves
    // against the wrong schema resolves perfectly well.
    fingerprint::check(table, query.schema.as_ref())?;

    let mut warnings = Vec::new();

    // Computed values first, and one at a time: the `i`th is converted in a
    // space holding the `i` before it, so it can read an earlier one and
    // cannot read itself or a later one. A single space over all of them would
    // let a client write a cycle the executor would evaluate as null.
    let mut compute = Vec::with_capacity(query.compute.len());
    for (at, scalar) in query.compute.iter().enumerate() {
        let space = Space::input(table, at, index);
        compute.push(scalar_named(
            &space,
            scalar,
            &format!("computed value {at}"),
        )?);
    }
    let space = Space::input(table, compute.len(), index);

    let filter = match &query.filter {
        Some(filter) => expr_named(&space, filter, "the filter")?,
        None => Expr::True,
    };

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
                columns.push(space.resolve_stored_column(Some(column), "the projection")?);
            }
            Projection::Columns(columns)
        }
    };

    let sort = sort_from_proto(&space, &query.sort, "the sort")?;

    let hint = match query.hint.as_ref().and_then(|hint| hint.path.as_ref()) {
        None => None,
        Some(pb::access_hint::Path::TableScan(value)) => {
            unit(*value, "an access hint's `table_scan`")?;
            Some(AccessHint::TableScan)
        }
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
            compute,
            // The wire carries no cursor yet, so a remote caller pages by
            // offset. Hard-coded rather than plumbed through a field that does
            // not exist: `None` here is the honest translation of a request
            // that could not have asked for one, and the day the proto grows
            // the field this line is where it lands.
            after: None,
        },
        warnings,
    ))
}

/// Refuse the parts of a `Query` that have no meaning where it is being used.
///
/// The kernel documents a join side's `sort`, `limit` and `offset` as ignored,
/// and an aggregate's input narrows the projection to exactly the columns the
/// aggregates read. Ignoring a field a client set is how a request comes to
/// mean something other than what was written, so each is refused and the
/// message says where the setting does belong.
fn refuse_unused(query: &pb::Query, what: &str, instead: &str) -> Result<(), Status> {
    if !query.sort.is_empty() {
        return Err(bad(format!(
            "{what} has a sort, which would not order the result; {instead}"
        )));
    }
    if query.limit.is_some() {
        return Err(bad(format!(
            "{what} has a limit, which would change the answer rather than shorten it; {instead}"
        )));
    }
    if query.offset != 0 {
        return Err(bad(format!(
            "{what} has an offset, which would change the answer rather than skip rows; {instead}"
        )));
    }
    Ok(())
}

// --- joins and chains -----------------------------------------------------

/// A multi-table read, as the kernel takes it.
///
/// Two tables become a [`Join`] and more become a [`Chain`], which is a
/// dispatch rather than a difference in the request: the wire has one shape for
/// both because a chain step and a two-table join are the same idea. The split
/// exists because the kernel's two-table join can choose *which* side to hold
/// in memory, and a chain step cannot — its earlier rows are already there.
#[derive(Debug, Clone)]
pub enum MultiRead {
    /// Exactly two tables.
    Join(Box<Join>),
    /// Three or more.
    Chain(Box<Chain>),
}

/// A multi-table read as the kernel's, with the tables it names.
///
/// The tables come back as ids rather than definitions so the caller can look
/// them up in whichever catalog is going to serve the read — the pool's for a
/// replica read, the writer store's for one inside a transaction. Handing back
/// borrowed definitions would tie the request to the catalog that parsed it.
pub fn join_from_proto(
    wire: &pb::JoinQuery,
    catalog: &Catalog,
) -> Result<(Vec<TableId>, MultiRead, Vec<String>), Status> {
    if wire.inputs.len() < 2 {
        return Err(bad(format!(
            "a join needs at least two inputs, and this one has {}; \
             a single-table read is a Query",
            wire.inputs.len()
        )));
    }

    // Resolve every table first: the ordinal spaces below are built from their
    // widths, so a name that does not resolve has to fail before anything is
    // converted against a space that is missing an input.
    let mut tables = Vec::with_capacity(wire.inputs.len());
    for (index, input) in wire.inputs.iter().enumerate() {
        let query = input
            .query
            .as_ref()
            .ok_or_else(|| bad(format!("input {index} has no query")))?;
        let table = catalog.table_by_name(&query.table).ok_or_else(|| {
            // `NOT_FOUND` is what a single-table read gives for an unknown
            // table, and the same reasoning applies: the request is
            // well-formed, this catalog simply has no such table.
            Status::not_found(format!("no table named `{}`", query.table))
        })?;
        tables.push(table);
    }
    let shapes: Vec<Input<'_>> = wire
        .inputs
        .iter()
        .zip(&tables)
        .map(|(input, table)| {
            Input::new(table, input.query.as_ref().map_or(0, |q| q.compute.len()))
        })
        .collect();

    let mut warnings = Vec::new();
    let mut queries = Vec::with_capacity(wire.inputs.len());
    let mut steps = Vec::with_capacity(wire.inputs.len() - 1);

    for (index, (input, table)) in wire.inputs.iter().zip(&tables).enumerate() {
        let table = *table;
        let wire_query = input
            .query
            .as_ref()
            .ok_or_else(|| bad(format!("input {index} has no query")))?;
        refuse_unused(
            wire_query,
            &format!("input {index} (`{}`)", table.name()),
            "put them on the join itself",
        )?;
        let (query, mut hints) = query_from_proto_at(wire_query, table, index)?;
        warnings.append(&mut hints);

        if index == 0 {
            if !input.on.is_empty() {
                return Err(bad(
                    "input 0 has a join condition, and there is nothing before it to join to",
                ));
            }
            queries.push(query);
            continue;
        }

        // The earlier side is named in the space of everything read before
        // this input; the near side in this input's own ordinals. Both are the
        // same `ColumnRef`, resolved against different spaces — which is what
        // makes "join back to any earlier input" free rather than a feature.
        let earlier_space = Space::joined(shapes.clone(), index);
        let own_space = Space::input(table, 0, index);
        let mut on = Vec::with_capacity(input.on.len());
        for key in &input.on {
            let left = earlier_space
                .resolve_stored_column(key.earlier.as_ref(), "a join equality's earlier side")?;
            let right =
                own_space.resolve_stored_column(key.own.as_ref(), "a join equality's own side")?;
            on.push(JoinKey::new(left, right));
        }

        // `having` may read this input as well as the earlier ones: it is
        // checked on a formed pair, so the new row exists by then.
        let having_space = Space::joined(shapes.clone(), index + 1);
        let having = match &input.having {
            Some(having) => expr_named(&having_space, having, "the join condition")?,
            None => Expr::True,
        };

        steps.push(Step {
            query,
            on,
            join_type: join_type_from_proto(input.join_type)?,
            having,
            force: algorithm_from_proto(input.force.as_ref())?,
        });
    }

    // The join's own computed values, over the whole joined row. Each is
    // converted in a space that knows about the ones *before* it and not about
    // itself, which is what turns "compute[0] reads compute[1]" into a refusal
    // rather than a value that is null on every row. The kernel checks the same
    // rule again on its own ordinals; this one exists so the message names the
    // wire's kinds rather than an ordinal the client never wrote.
    let mut compute = Vec::with_capacity(wire.compute.len());
    for (index, scalar) in wire.compute.iter().enumerate() {
        let space = Space::joined_computing(shapes.clone(), shapes.len(), index);
        compute.push(scalar_named(
            &space,
            scalar,
            &format!("computed value {index} of the join"),
        )?);
    }

    let limit = wire.limit.map(|limit| limit as usize);
    let offset = wire.offset as usize;
    let build_limit = build_limit_from_proto(wire.build_limit, &mut warnings);

    let read = if shapes.len() == 2 {
        let step = steps.remove(0);
        let mut join = Join::on(step.on)
            .left(queries.remove(0))
            .right(step.query)
            .having(step.having)
            .build_limit(build_limit);
        join.join_type = step.join_type;
        join.limit = limit;
        join.offset = offset;
        join.force = step.force;
        join.compute = compute;
        MultiRead::Join(Box::new(join))
    } else {
        let mut chain = Chain::from(queries.remove(0))
            .offset(offset)
            .build_limit(build_limit);
        chain.limit = limit;
        chain.compute = compute;
        for step in steps {
            let mut next = JoinStep::on(step.on).query(step.query).having(step.having);
            next.join_type = step.join_type;
            next.force = step.force;
            chain = chain.join(next);
        }
        MultiRead::Chain(Box::new(chain))
    };

    Ok((tables.iter().map(|t| t.id()).collect(), read, warnings))
}

/// The first two tables of a multi-table read.
///
/// [`join_from_proto`] refuses fewer than two inputs, so this cannot fail in
/// practice. It is written as a refusal rather than an index so the invariant
/// is enforced where it is used rather than remembered — a head node should not
/// be able to panic on a request shape.
pub fn two_tables<'t>(
    tables: &[&'t TableDef],
) -> Result<(&'t TableDef, &'t TableDef), slate_kernel::KernelError> {
    match tables {
        [left, right, ..] => Ok((*left, *right)),
        _ => Err(slate_kernel::KernelError::JoinNotSupported {
            reason: format!(
                "a join needs two tables and this one names {}",
                tables.len()
            ),
        }),
    }
}

/// One converted input past the first, before it is known whether the read is
/// a two-table join or a chain.
struct Step {
    query: Query,
    on: Vec<JoinKey>,
    join_type: JoinType,
    having: Expr,
    force: Option<JoinAlgorithm>,
}

/// A build limit a client may lower and not raise.
///
/// The server's own limit is what stops a mistyped join key turning into an
/// out-of-memory kill, and a limit the client sets is not one. Clamping rather
/// than refusing, with a warning, because a client asking for *more* memory is
/// making a request the server is entitled to decline quietly — where refusing
/// would fail a query that will run perfectly well within the real limit.
fn build_limit_from_proto(asked: Option<u64>, warnings: &mut Vec<String>) -> usize {
    let Some(asked) = asked else {
        return DEFAULT_BUILD_LIMIT;
    };
    let asked = usize::try_from(asked).unwrap_or(usize::MAX);
    if asked > DEFAULT_BUILD_LIMIT {
        warnings.push(format!(
            "build limit lowered from {asked} to this node's limit of {DEFAULT_BUILD_LIMIT}"
        ));
        return DEFAULT_BUILD_LIMIT;
    }
    asked
}

fn join_type_from_proto(join_type: i32) -> Result<JoinType, Status> {
    match pb::JoinType::try_from(join_type) {
        // Inner is a genuine default rather than a refusal: SQL spells an
        // inner join as plain `JOIN`, and it is what `JoinType::default()` is.
        Ok(pb::JoinType::Inner) => Ok(JoinType::Inner),
        Ok(pb::JoinType::Left) => Ok(JoinType::Left),
        Ok(pb::JoinType::Right) => Ok(JoinType::Right),
        Ok(pb::JoinType::Full) => Ok(JoinType::Full),
        Err(_) => Err(bad(format!(
            "join type {join_type} is not one this server knows"
        ))),
    }
}

const fn join_type_to_proto(join_type: JoinType) -> pb::JoinType {
    match join_type {
        JoinType::Inner => pb::JoinType::Inner,
        JoinType::Left => pb::JoinType::Left,
        JoinType::Right => pb::JoinType::Right,
        JoinType::Full => pb::JoinType::Full,
    }
}

fn algorithm_from_proto(
    algorithm: Option<&pb::JoinAlgorithm>,
) -> Result<Option<JoinAlgorithm>, Status> {
    let Some(algorithm) = algorithm.and_then(|a| a.algorithm.as_ref()) else {
        return Ok(None);
    };
    Ok(match algorithm {
        pb::join_algorithm::Algorithm::HashBuild(side) => match pb::Side::try_from(*side) {
            Ok(pb::Side::Left) => Some(JoinAlgorithm::Hash { build: Side::Left }),
            Ok(pb::Side::Right) => Some(JoinAlgorithm::Hash { build: Side::Right }),
            Err(_) => {
                return Err(bad(format!("side {side} is not one this server knows")));
            }
        },
        pb::join_algorithm::Algorithm::NestedLoop(value) => {
            // No `false` arm to interpret any more: `nested_loop` is a `Unit`,
            // so a client that zeroed the message leaves `algorithm` unset and
            // is handled above. That used to be a `bool` whose false spelling
            // had to be read as "no algorithm forced" — a field that meant
            // something other than what it said.
            unit(*value, "a forced algorithm's `nested_loop`")?;
            Some(JoinAlgorithm::NestedLoop)
        }
    })
}

#[must_use]
fn algorithm_to_proto(algorithm: JoinAlgorithm) -> pb::JoinAlgorithm {
    let algorithm = match algorithm {
        JoinAlgorithm::Hash { build } => pb::join_algorithm::Algorithm::HashBuild(match build {
            Side::Left => pb::Side::Left as i32,
            Side::Right => pb::Side::Right as i32,
        }),
        JoinAlgorithm::NestedLoop => {
            pb::join_algorithm::Algorithm::NestedLoop(pb::Unit::Unit as i32)
        }
    };
    pb::JoinAlgorithm {
        algorithm: Some(algorithm),
    }
}

/// A two-table join in its wire form.
#[must_use]
pub fn join_to_proto(left: &TableDef, right: &TableDef, join: &Join) -> pb::JoinQuery {
    let shapes = vec![
        Input::new(left, join.left.compute.len()),
        Input::new(right, join.right.compute.len()),
    ];
    // A two-table join's equalities are each in their own table's ordinals,
    // and the left table's offset in the joined space is zero — so the same
    // `unresolve` serves both this and the chain case below.
    let earlier = Space::joined(shapes.clone(), 1);
    let own = Space::input(right, 0, 1);
    // The join's own computed values, each unresolved in a space that knows
    // about the ones before it — the mirror of the conversion inwards, so a
    // round trip through both is the identity.
    let compute = join
        .compute
        .iter()
        .enumerate()
        .map(|(index, scalar)| {
            scalar_to_proto(&Space::joined_computing(shapes.clone(), 2, index), scalar)
        })
        .collect();
    pb::JoinQuery {
        compute,
        inputs: vec![
            pb::JoinInput {
                query: Some(query_to_proto_at(left, &join.left, 0)),
                on: Vec::new(),
                join_type: pb::JoinType::Inner as i32,
                having: None,
                force: None,
            },
            pb::JoinInput {
                query: Some(query_to_proto_at(right, &join.right, 1)),
                on: join
                    .on
                    .iter()
                    .map(|key| pb::JoinOn {
                        earlier: Some(earlier.unresolve(key.left)),
                        own: Some(own.unresolve(key.right)),
                    })
                    .collect(),
                join_type: join_type_to_proto(join.join_type) as i32,
                having: Some(expr_to_proto(&Space::joined(shapes, 2), &join.having)),
                force: join.force.map(algorithm_to_proto),
            },
        ],
        limit: join.limit.map(|limit| limit as u64),
        offset: join.offset as u64,
        build_limit: Some(join.build_limit as u64),
    }
}

/// A chain in its wire form.
#[must_use]
pub fn chain_to_proto(tables: &[&TableDef], chain: &Chain) -> pb::JoinQuery {
    let shapes: Vec<Input<'_>> = tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let computed = match index.checked_sub(1) {
                None => chain.first.compute.len(),
                Some(step) => chain.steps.get(step).map_or(0, |s| s.query.compute.len()),
            };
            Input::new(table, computed)
        })
        .collect();

    let mut inputs = Vec::with_capacity(tables.len());
    if let Some(first) = tables.first() {
        inputs.push(pb::JoinInput {
            query: Some(query_to_proto_at(first, &chain.first, 0)),
            on: Vec::new(),
            join_type: pb::JoinType::Inner as i32,
            having: None,
            force: None,
        });
    }
    for (at, (step, table)) in chain.steps.iter().zip(tables.iter().skip(1)).enumerate() {
        let index = at + 1;
        let earlier = Space::joined(shapes.clone(), index);
        let own = Space::input(table, 0, index);
        inputs.push(pb::JoinInput {
            query: Some(query_to_proto_at(table, &step.query, index)),
            on: step
                .on
                .iter()
                .map(|key| pb::JoinOn {
                    earlier: Some(earlier.unresolve(key.left)),
                    own: Some(own.unresolve(key.right)),
                })
                .collect(),
            join_type: join_type_to_proto(step.join_type) as i32,
            having: Some(expr_to_proto(
                &Space::joined(shapes.clone(), index + 1),
                &step.having,
            )),
            force: step.force.map(algorithm_to_proto),
        });
    }

    let computing = shapes.len();
    pb::JoinQuery {
        inputs,
        limit: chain.limit.map(|limit| limit as u64),
        offset: chain.offset as u64,
        build_limit: Some(chain.build_limit as u64),
        compute: chain
            .compute
            .iter()
            .enumerate()
            .map(|(index, scalar)| {
                scalar_to_proto(
                    &Space::joined_computing(shapes.clone(), computing, index),
                    scalar,
                )
            })
            .collect(),
    }
}

/// A chain row flattened to one entry per input, padded where the kernel's
/// row is shorter.
///
/// The padding is defensive, and deliberately so. A `ChainRow` that has been
/// through every step is as long as the chain, and a right outer step that
/// preserves a row of a later table fills in the earlier ones — so today the
/// two lengths always agree. A client reads its inputs *positionally*, though,
/// and a shorter row would silently shift every input past the gap rather than
/// error, which is the worst shape this could fail in. So the width comes from
/// the request rather than from the row, and
/// `a_chain_row_is_padded_to_one_entry_per_input` pins it.
#[must_use]
pub fn chain_row_values(row: &ChainRow, inputs: usize) -> Vec<Option<Row>> {
    (0..inputs).map(|at| row.at(at).cloned()).collect()
}

/// One row of a multi-table read in its wire form.
///
/// Takes the row already flattened to one entry per input — `None` where an
/// outer join preserved something that matched nothing — because the two
/// kernel cursors spell that differently and the wire should not. See
/// [`crate::session::MultiCursor`], which does the flattening and the padding
/// a chain needs.
///
/// `stored` is one declared table width per input, so each input's computed
/// values are split off its *own* table rather than off whichever width came
/// to hand. An input with no width — which cannot happen, since the widths come
/// from the same request the row does — is sent whole rather than panicking.
#[must_use]
pub fn multi_row_to_proto(row: &MultiRow, stored: &[usize]) -> pb::JoinedRow {
    pb::JoinedRow {
        computed: row.computed.iter().map(value_to_proto).collect(),
        inputs: row
            .inputs
            .iter()
            .enumerate()
            .map(|(input, row)| pb::JoinedInput {
                row: row.as_ref().map(|row| {
                    row_to_proto_split(
                        row,
                        stored.get(input).copied().unwrap_or(row.values().len()),
                    )
                }),
            })
            .collect(),
    }
}

// --- aggregate queries ----------------------------------------------------

/// What an aggregate request asks for, as the kernel takes it.
#[derive(Debug)]
pub struct GroupedRead {
    /// Where the rows come from.
    pub source: GroupedSource,
    /// The grouping columns, empty for one group over everything.
    pub group: Vec<Ordinal>,
    /// The aggregates, in request order.
    pub aggregates: Vec<Aggregate>,
    /// Which groups survive, over the group's own ordinal space.
    pub having: Expr,
    /// How the groups are ordered, over the group's own ordinal space.
    pub sort: Vec<SortKey>,
    /// At most this many groups, after `having` and `sort`.
    pub limit: Option<usize>,
    /// Groups to discard first, after `having` and `sort`.
    pub offset: usize,
}

impl GroupedRead {
    /// The kernel's grouping for this read.
    ///
    /// Built rather than stored so that the wire shape and the kernel shape
    /// stay separately reviewable: this is the one place the two are lined up.
    #[must_use]
    pub fn grouping(&self) -> Grouping {
        let mut grouping = Grouping::by(self.group.iter().copied(), &self.aggregates)
            .having(self.having.clone())
            .sort_by(self.sort.iter().copied())
            .offset(self.offset);
        if let Some(limit) = self.limit {
            grouping = grouping.limit(limit);
        }
        grouping
    }
}

/// What a grouped read groups over.
///
/// A chain is absent deliberately: the kernel groups a two-table joined row
/// stream and does not yet group a chain, so a request naming three inputs is
/// refused with that reason rather than silently planned as something else.
#[derive(Debug, Clone)]
pub enum GroupedSource {
    /// One table.
    Table {
        /// The table it reads.
        table: TableId,
        /// Which rows.
        query: Query,
    },
    /// Three or more tables, chained.
    ///
    /// Separate from [`GroupedSource::Join`] rather than folded into it,
    /// because the kernel has two entry points — `group_by_join` narrows each
    /// side's projection and `group_by_chain` cannot — and collapsing them
    /// here would mean choosing at the call site anyway, one level further
    /// from the reason.
    Chain {
        /// The tables, in the order the chain reads them.
        tables: Vec<TableId>,
        /// The chain itself.
        chain: Box<Chain>,
    },
    /// Exactly two tables, joined.
    ///
    /// A pair rather than a `Vec`, because "exactly two" is checked once here
    /// and every consumer would otherwise index into it and have to be trusted
    /// not to be wrong.
    Join {
        /// The left table.
        left: TableId,
        /// The right table.
        right: TableId,
        /// The join itself.
        join: Box<Join>,
    },
}

/// Sort keys resolved against `space`.
///
/// Shared between a query's `ORDER BY` over rows and a grouped read's over
/// groups. The two differ only in which space the columns name — the input's
/// or the group's — which is exactly the distinction that makes ordering a
/// grouped result by a raw column a refusal rather than a silent null.
fn sort_from_proto(
    space: &Space<'_>,
    keys: &[pb::SortKey],
    place: &str,
) -> Result<Vec<SortKey>, Status> {
    let mut sort = Vec::with_capacity(keys.len());
    for key in keys {
        let column = space.resolve(key.column.as_ref(), place)?;
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
    Ok(sort)
}

/// An aggregate request as the kernel's.
pub fn aggregate_from_proto_query(
    wire: &pb::AggregateQuery,
    catalog: &Catalog,
) -> Result<(GroupedRead, Vec<String>), Status> {
    // Exactly one source. Both set is a client bug worth reporting rather than
    // a precedence rule worth inventing, and neither is the same bug.
    match (wire.input.as_ref(), wire.join.as_ref()) {
        (Some(_), Some(_)) => {
            return Err(bad(
                "an aggregate request sets both `input` and `join`; it aggregates over \
                 one or the other, so set exactly one",
            ));
        }
        (None, None) => return Err(bad("an aggregate request has no input query")),
        _ => {}
    }

    if wire.aggregates.is_empty() {
        return Err(bad(
            "an aggregate request with no aggregates is a query; use Query",
        ));
    }

    let (source, space, warnings) = match (wire.input.as_ref(), wire.join.as_ref()) {
        (Some(input), _) => {
            let table = catalog
                .table_by_name(&input.table)
                .ok_or_else(|| Status::not_found(format!("no table named `{}`", input.table)))?;
            refuse_unused(
                input,
                "an aggregate's input",
                "order and limit the groups with the aggregate's own `sort`, `limit` \
                 and `offset`, which are over groups rather than over input rows",
            )?;
            if input.projection.is_some() {
                // The kernel narrows the projection to exactly the columns the
                // aggregates read, which is what lets an index answer
                // `COUNT(*)` without touching a row. Honouring a client's
                // projection would silently undo that; ignoring it silently is
                // worse.
                return Err(bad(
                    "an aggregate's input must not set a projection: it is narrowed to the \
                     columns the aggregates and grouping read, which is what lets an index \
                     answer without reading a row",
                ));
            }
            let (query, warnings) = query_from_proto_at(input, table, 0)?;
            let space = Space::input(table, query.compute.len(), 0);
            (
                GroupedSource::Table {
                    table: table.id(),
                    query,
                },
                space,
                warnings,
            )
        }
        (_, Some(join_wire)) => {
            let (tables, read, warnings) = join_from_proto(join_wire, catalog)?;
            // The group and the aggregates are named in the joined schema, so
            // the space has to be the same one `JoinQuery.having` uses: every
            // input visible, each shifted past the width of everything before
            // it. Rebuilt from the wire rather than returned by
            // `join_from_proto`, because the compute counts are right here.
            let mut shapes = Vec::with_capacity(tables.len());
            for (id, input) in tables.iter().zip(&join_wire.inputs) {
                let table = catalog
                    .table(*id)
                    .ok_or_else(|| bad("a table resolved by the join is not in the catalog"))?;
                let computed = input.query.as_ref().map_or(0, |q| q.compute.len());
                shapes.push(Input::new(table, computed));
            }
            let visible = shapes.len();
            // Plus the join's own computed values, which is what lets
            // `group_by` and the aggregates name one. Grouping is the whole
            // reason `JoinQuery.compute` exists — a computed value that no
            // group key can name would be a column nobody can ask about — so
            // the count comes from the same request the shapes do.
            let space = Space::joined_computing(shapes, visible, join_wire.compute.len());

            let source = match read {
                MultiRead::Join(join) => {
                    let [left, right] = <[TableId; 2]>::try_from(tables).map_err(|_| {
                        bad("a two-table join resolved to a different number of tables")
                    })?;
                    GroupedSource::Join { left, right, join }
                }
                // Three or more inputs used to be refused here, saying the
                // kernel does not group a chain. It does now, so the refusal
                // is gone rather than left to become a lie about what the
                // kernel can do.
                MultiRead::Chain(chain) => GroupedSource::Chain { tables, chain },
            };
            (source, space, warnings)
        }
        (None, None) => unreachable!("checked above"),
    };

    let aggregates = wire
        .aggregates
        .iter()
        .map(|aggregate| aggregate_from_proto(&space, aggregate))
        .collect::<Result<Vec<_>, _>>()?;

    let mut group = Vec::with_capacity(wire.group_by.len());
    for column in &wire.group_by {
        group.push(space.resolve(Some(column), "a grouping column")?);
    }

    // `having` and `sort` read the group, not a row, so they resolve against
    // the group space — which is what turns "column must appear in the GROUP
    // BY clause" into a refusal instead of a silent null.
    let group_space = Space::groups(group.len(), aggregates.len());
    let having = match &wire.having {
        Some(having) => expr_named(&group_space, having, "the HAVING condition")?,
        None => Expr::True,
    };
    let sort = sort_from_proto(&group_space, &wire.sort, "the group ordering")?;

    Ok((
        GroupedRead {
            source,
            group,
            aggregates,
            having,
            sort,
            limit: wire.limit.map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
            offset: usize::try_from(wire.offset).unwrap_or(usize::MAX),
        },
        warnings,
    ))
}

/// An aggregate request in its wire form.
#[must_use]
pub fn aggregate_to_proto_query(
    table: &TableDef,
    query: &Query,
    group: &[Ordinal],
    aggregates: &[Aggregate],
    having: &Expr,
) -> pb::AggregateQuery {
    let space = Space::input(table, query.compute.len(), 0);
    let group_space = Space::groups(group.len(), aggregates.len());
    let mut input = query_to_proto_at(table, query, 0);
    // The projection is the kernel's to choose here, and the inbound direction
    // refuses one, so it must not be sent.
    input.projection = None;
    pb::AggregateQuery {
        input: Some(input),
        group_by: group.iter().map(|c| space.unresolve(*c)).collect(),
        aggregates: aggregates
            .iter()
            .map(|a| aggregate_to_proto(&space, *a))
            .collect(),
        having: Some(expr_to_proto(&group_space, having)),
        // The outbound direction is used by the typed client for a
        // single-table grouped read, which is why these are the empty case
        // rather than parameters: a caller wanting a grouped join or ordered
        // groups builds the message itself.
        join: None,
        sort: Vec::new(),
        limit: None,
        offset: 0,
    }
}

// --- freshness ------------------------------------------------------------

/// How fresh a read has to be. An absent message means [`Freshness::Any`],
/// which is the kernel's default and the cheapest read.
pub fn freshness_from_proto(freshness: Option<&pb::Freshness>) -> Result<Freshness, Status> {
    let Some(level) = freshness.and_then(|f| f.level.as_ref()) else {
        return Ok(Freshness::Any);
    };
    Ok(match level {
        pb::freshness::Level::Any(value) => {
            unit(*value, "a freshness's `any`")?;
            Freshness::Any
        }
        pb::freshness::Level::AtLeast(sequence) => Freshness::AtLeast(ReadToken::new(*sequence)),
        // There is no longer a `latest: false` to reinterpret. It used to be
        // read as `ANY` — with the right reason, since it is what a client
        // that zeroed the struct sent, and routing every such read to the
        // writer would point the whole fleet at the scarcest resource in the
        // deployment — but the outcome was a wire that said "the writer" and
        // meant "any replica". `Unit` removes the spelling instead of
        // reinterpreting it: a client with nothing to say leaves the message
        // absent, which has always meant `ANY`.
        pb::freshness::Level::Latest(value) => {
            unit(*value, "a freshness's `latest`")?;
            Freshness::Latest
        }
    })
}

/// A freshness in its wire form.
#[must_use]
pub const fn freshness_to_proto(freshness: Freshness) -> pb::Freshness {
    let level = match freshness {
        Freshness::Any => pb::freshness::Level::Any(pb::Unit::Unit as i32),
        Freshness::AtLeast(token) => pb::freshness::Level::AtLeast(token.sequence()),
        Freshness::Latest => pb::freshness::Level::Latest(pb::Unit::Unit as i32),
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
        decodes: explanation
            .decodes
            .iter()
            .map(|ordinal| ordinal.0 as u32)
            .collect(),
    }
}

/// A two-table join's plan in its wire form.
///
/// The second input's estimates are the join's own, because a two-table join
/// has one step and its accumulated rows *are* the result. A chain reports each
/// step separately; see [`chain_plan_to_proto`].
#[must_use]
pub fn join_explanation_to_proto(
    explanation: &JoinExplanation,
    warnings: Vec<String>,
    served_by: Option<pb::ServedBy>,
) -> pb::JoinExplainResponse {
    pb::JoinExplainResponse {
        inputs: vec![
            pb::JoinInputPlan {
                plan: Some(explanation_to_proto(&explanation.left, Vec::new(), None)),
                join_type: pb::JoinType::Inner as i32,
                algorithm: None,
                estimated_rows: explanation.left.estimated_rows,
                estimated_cost: explanation.left.estimated_cost,
            },
            pb::JoinInputPlan {
                plan: Some(explanation_to_proto(&explanation.right, Vec::new(), None)),
                join_type: join_type_to_proto(explanation.join_type) as i32,
                algorithm: Some(algorithm_to_proto(explanation.algorithm)),
                estimated_rows: explanation.estimated_rows,
                estimated_cost: explanation.estimated_cost,
            },
        ],
        estimated_rows: explanation.estimated_rows,
        estimated_cost: explanation.estimated_cost,
        display: explanation.to_string(),
        warnings,
        served_by,
    }
}

/// A chain's plan in its wire form.
#[must_use]
pub fn chain_plan_to_proto(
    plan: &ChainPlan,
    tables: &[&TableDef],
    chain: &Chain,
    warnings: Vec<String>,
    served_by: Option<pb::ServedBy>,
) -> pb::JoinExplainResponse {
    let Some(first_table) = tables.first() else {
        // Unreachable: a chain plan comes from a chain that named its tables.
        // Answered as an empty explanation rather than a panic, on the
        // principle that a head node should not be able to bring itself down
        // over a shape it can describe.
        return pb::JoinExplainResponse {
            inputs: Vec::new(),
            estimated_rows: plan.estimated_rows,
            estimated_cost: plan.estimated_cost,
            display: String::new(),
            warnings,
            served_by,
        };
    };
    let first = Explanation::of(first_table, &plan.first, &chain.first);
    let mut display = format!("Chain\n  -> {first}");
    let mut inputs = vec![pb::JoinInputPlan {
        plan: Some(explanation_to_proto(&first, Vec::new(), None)),
        join_type: pb::JoinType::Inner as i32,
        algorithm: None,
        estimated_rows: plan.first.estimated_rows,
        estimated_cost: plan.first.estimated_cost,
    }];
    for (at, step) in plan.steps.iter().enumerate() {
        let Some(table) = tables.get(at + 1) else {
            break;
        };
        let Some(request) = chain.steps.get(at) else {
            break;
        };
        let explanation = Explanation::of(table, &step.plan, &request.query);
        // Written out here rather than taken from a `Display` impl, because
        // `ChainPlan` has none — and a chain's shape is the one thing an
        // operator reads first, so it is worth a line per step.
        display.push_str(&format!(
            "\n  -> {} step {}: {explanation}",
            match step.algorithm {
                JoinAlgorithm::Hash { .. } => "Hash",
                JoinAlgorithm::NestedLoop => "Nested Loop",
            },
            at + 1
        ));
        inputs.push(pb::JoinInputPlan {
            plan: Some(explanation_to_proto(&explanation, Vec::new(), None)),
            join_type: join_type_to_proto(request.join_type) as i32,
            algorithm: Some(algorithm_to_proto(step.algorithm)),
            estimated_rows: step.estimated_rows,
            estimated_cost: step.estimated_cost,
        });
    }

    pb::JoinExplainResponse {
        inputs,
        estimated_rows: plan.estimated_rows,
        estimated_cost: plan.estimated_cost,
        display,
        warnings,
        served_by,
    }
}

/// A grouped read's plan in its wire form.
///
/// `display` is prefixed with the grouping rather than left to the underlying
/// explanation, because the projections alone do not say which of the columns
/// being read are group keys and which are being folded — and that is exactly
/// what someone reading the plan of a grouped read wants to know first.
#[must_use]
pub fn grouped_explanation_to_proto(
    explanation: &GroupedExplanation,
    tables: &[&TableDef],
    grouping: &Grouping,
    warnings: Vec<String>,
    served_by: pb::ServedBy,
) -> pb::AggregateExplainResponse {
    // Named, not numbered. The first version of this printed the raw joined
    // ordinals and the `Debug` form of each aggregate — `Group by [2]
    // computing [Count, Max(Ordinal(6))]` — which is legible with the schema
    // open beside it and meaningless without. The names are right here in
    // `tables`, and this string is the one thing about a grouped plan a person
    // reads first.
    let schema = JoinSchema::over(tables.iter().copied());
    let name_of = |joined: Ordinal| -> String {
        let Some((position, at)) = schema.locate(joined) else {
            return format!("#{}", joined.0);
        };
        let Some(table) = tables.get(position) else {
            return format!("#{}", joined.0);
        };
        match table.column(at) {
            // Qualified only when there is more than one input: `books.year`
            // reads as noise on a single-table grouping, where every column is
            // from the one table by construction.
            Some(column) if tables.len() > 1 => format!("{}.{}", table.name(), column.name()),
            Some(column) => column.name().to_owned(),
            None => format!("#{}", joined.0),
        }
    };
    let describe = |aggregate: &Aggregate| match aggregate {
        Aggregate::Count => "count(*)".to_owned(),
        Aggregate::CountColumn(c) => format!("count({})", name_of(*c)),
        Aggregate::CountDistinct(c) => format!("count(distinct {})", name_of(*c)),
        Aggregate::Min(c) => format!("min({})", name_of(*c)),
        Aggregate::Max(c) => format!("max({})", name_of(*c)),
        Aggregate::Sum(c) => format!("sum({})", name_of(*c)),
        Aggregate::Avg(c) => format!("avg({})", name_of(*c)),
    };

    let keys: Vec<String> = grouping.group.iter().copied().map(name_of).collect();
    let heading = format!(
        "Group by [{}] computing [{}]",
        keys.join(", "),
        grouping
            .aggregates
            .iter()
            .map(describe)
            .collect::<Vec<_>>()
            .join(", ")
    );

    let (input, join) = match explanation {
        GroupedExplanation::Table(plan) => {
            (Some(explanation_to_proto(plan, Vec::new(), None)), None)
        }
        GroupedExplanation::Join(plan) => (
            None,
            Some(join_explanation_to_proto(plan, Vec::new(), None)),
        ),
        GroupedExplanation::Chain(chain, plan) => (
            None,
            Some(chain_plan_to_proto(plan, tables, chain, Vec::new(), None)),
        ),
    };

    let body = input.as_ref().map_or_else(
        || {
            join.as_ref()
                .map_or_else(String::new, |j| j.display.clone())
        },
        |i| i.display.clone(),
    );

    pb::AggregateExplainResponse {
        input,
        join,
        display: format!("{heading}\n  {body}"),
        warnings,
        served_by: Some(served_by),
    }
}
