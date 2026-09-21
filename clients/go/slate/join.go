package slate

import (
	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
)

// JoinType is which rows survive a join.
type JoinType int

// The join types.
const (
	// Inner keeps rows that matched on both sides.
	Inner JoinType = iota
	// Left keeps every left row, padding an unmatched right side with nulls.
	Left
	// Right keeps every right row.
	Right
	// Full keeps every row from both.
	Full
)

func (j JoinType) wire() pb.JoinType {
	switch j {
	case Left:
		return pb.JoinType_JOIN_TYPE_LEFT
	case Right:
		return pb.JoinType_JOIN_TYPE_RIGHT
	case Full:
		return pb.JoinType_JOIN_TYPE_FULL
	default:
		return pb.JoinType_JOIN_TYPE_INNER
	}
}

// Algorithm forces a join strategy the planner would otherwise choose.
//
// A test aid and an escape hatch, not a tuning knob: the planner costs these
// and is usually right. Forcing one is how the oracle checks that every
// algorithm agrees, which is the reason it is reachable at all.
type Algorithm int

// The join algorithms.
const (
	// Chosen leaves it to the planner. The zero value.
	Chosen Algorithm = iota
	// HashBuildLeft builds the hash table from the left input.
	HashBuildLeft
	// HashBuildRight builds it from the right.
	HashBuildRight
	// NestedLoop probes an index on the right for each left row.
	NestedLoop
)

func (a Algorithm) wire() *pb.JoinAlgorithm {
	switch a {
	case HashBuildLeft:
		return &pb.JoinAlgorithm{Algorithm: &pb.JoinAlgorithm_HashBuild{HashBuild: pb.Side_SIDE_LEFT}}
	case HashBuildRight:
		return &pb.JoinAlgorithm{Algorithm: &pb.JoinAlgorithm_HashBuild{HashBuild: pb.Side_SIDE_RIGHT}}
	case NestedLoop:
		return &pb.JoinAlgorithm{Algorithm: &pb.JoinAlgorithm_NestedLoop{NestedLoop: pb.Unit_UNIT}}
	default:
		return nil
	}
}

// Column names one input's column: which input, and that input's own ordinal.
//
// Not a cumulative offset into a flattened joined row. The wire carries the
// input index alongside the ordinal, so a column of the second table is
// `At(1, 2)` — the third column of *that* table — rather than "first table's
// width plus two". Getting this wrong is the easiest mistake to make about a
// join, and it is the one this type exists to remove: there is no arithmetic
// to do and no table width to know, so a client needs no catalog.
type Column struct {
	// Input is the input's position, in the order they were added.
	Input uint32
	// Ordinal is the column's position in that input's own table — or, for a
	// computed value, which of them it is.
	Ordinal Ordinal
	// kind is which of the wire's reference kinds this is.
	//
	// Unexported, and zero means a stored column, so a `Column{Input: 1,
	// Ordinal: 2}` literal written before computed values existed still means
	// what it meant. The constructors below are the way to get the other
	// kinds, which is also what keeps the kinds from being mixed by arithmetic.
	kind refKind
}

// Which of `ColumnRef`'s kinds a [Column] names.
//
// The wire distinguishes them rather than carrying a flat ordinal, because
// they live in different spaces and an ordinal that is in range in the wrong
// one is a query about a different column. See `ColumnRef` in `records.proto`.
type refKind int

const (
	// refColumn is a stored column of an input's table. The zero value.
	refColumn refKind = iota
	// refComputed is the nth value that input's own query computes.
	refComputed
	// refJoinedComputed is the nth value the *join* computes, which belongs to
	// no input and sits past every input's columns.
	refJoinedComputed
	// refWindowed is the nth value the query's windows produce. Nameable from
	// a sort key and nowhere else — see [Windowed].
	refWindowed
)

// At names column `ordinal` of input `input`.
func At(input uint32, ordinal Ordinal) Column {
	return Column{Input: input, Ordinal: ordinal}
}

// ComputedAt names the `n`th value input `input` computes.
//
// Legal wherever that input's own rows are read — its filter, its sort, and a
// single-table grouping. Not across a join: an input's computed values are
// appended to that input's row and a joined row is packed by declared table
// width, so there is no slot for one and the server says so. [JoinComputed] is
// the kind that does have a slot.
func ComputedAt(input uint32, n uint32) Column {
	return Column{Input: input, Ordinal: Ordinal(n), kind: refComputed}
}

// Computed0 names the `n`th value the only input computes, for a single-table
// query.
//
// `ComputedAt(0, n)` says the same thing; this reads better where there is no
// join, in the same way [Key0] does.
func Computed0(n uint32) Column { return ComputedAt(0, n) }

// JoinComputed names the `n`th value the **join itself** computes — the ones
// in [JoinQuery.Compute], not any one input's.
//
// It is evaluated over the whole joined row, so it may read every input, and
// it sits past every input's columns: the one place an ordinal can be added
// without moving one that already exists. That is what makes it addressable
// where an input's own computed value is not.
//
// No input index, because the value belongs to the request rather than to one
// of its tables.
func JoinComputed(n uint32) Column {
	return Column{Ordinal: Ordinal(n), kind: refJoinedComputed}
}

// Windowed names the `n`th value the query's windows produce.
//
// Usable in a sort key and nowhere else, which is SQL's own rule rather than a
// limitation here: a window is computed after the filter and before the sort,
// so a filter naming one would be asking for a value that does not exist yet.
// The server refuses that by name; this sentence saves the round trip.
//
// No input index, for the reason [JoinComputed] has none: the value belongs to
// the request rather than to one of its tables.
func Windowed(n uint32) Column {
	return Column{Ordinal: Ordinal(n), kind: refWindowed}
}

func (c Column) ref() *pb.ColumnRef {
	switch c.kind {
	case refComputed:
		return &pb.ColumnRef{
			Input: c.Input,
			Of:    &pb.ColumnRef_Computed{Computed: uint32(c.Ordinal)},
		}
	case refJoinedComputed:
		return &pb.ColumnRef{
			Of: &pb.ColumnRef_JoinedComputed{JoinedComputed: uint32(c.Ordinal)},
		}
	case refWindowed:
		return &pb.ColumnRef{
			Of: &pb.ColumnRef_Windowed{Windowed: uint32(c.Ordinal)},
		}
	default:
		return &pb.ColumnRef{
			Input: c.Input,
			Of:    &pb.ColumnRef_Column{Column: uint32(c.Ordinal)},
		}
	}
}

// On equates a column of an earlier input with one of this input.
//
// `Earlier` must name an input already read — the server refuses one that does
// not, by position rather than by guessing. `Own` is an ordinal of *this*
// input's table, so it needs no input index: which input it belongs to is the
// input it is written on.
type On struct {
	Earlier Column
	Own     Ordinal
}

// JoinInput is one table entering a join.
type JoinInput struct {
	// Table is the table's name.
	Table string
	// Filter admits rows of this input, before the join.
	Filter *Expr
	// On equates this input's columns with earlier ones. Empty for the first
	// input, which nothing precedes.
	On []On
	// Type is how unmatched rows are treated. Ignored on the first input.
	Type JoinType
	// Having is a condition evaluated after this input joins, and may name any
	// input read so far — which is what makes a chain more than nested pairs.
	Having *Expr
	// Force overrides the planner's algorithm choice.
	Force Algorithm
	// Columns is this input's projection. Empty means every column.
	Columns []Ordinal
	// Descending reads this input backwards where the access path allows it.
	Descending bool
	// Compute is values this input computes from its own rows, named with
	// [ComputedAt].
	//
	// Readable by this input's own filter, and returned beside its columns on
	// an ungrouped join. Not readable across the join and not groupable — see
	// [ComputedAt] — for which [JoinQuery.Compute] is the answer.
	Compute []Scalar
}

// JoinQuery joins two or more tables.
//
// No `Sort` on an input: the server refuses it, so there is nowhere here to
// set one and the refusal is unreachable rather than a runtime surprise. The
// limit and offset that do apply are on the query.
//
// This used to say the kernel documents input-level sorting as ignored. It
// does not and did not — a side's sort is honoured where that side streams and
// silently dropped where it is hashed, which is worse than either and is the
// real reason it is refused. See docs/paging-a-join.md.
type JoinQuery struct {
	// Inputs in the order they are read. The first has no `On`.
	Inputs []JoinInput
	// Limit caps the joined rows returned.
	Limit *uint64
	// Offset discards joined rows before the limit applies.
	Offset uint64
	// BuildLimit caps rows held in a hash build side. The server clamps a
	// value above its own ceiling and says so in a warning.
	BuildLimit *uint64
	// After resumes after this row of **input 0's** table — keyset paging.
	//
	// A page of a join is a page of its driving table: the cursor is input 0's
	// primary key, Limit counts input-0 rows, and every joined row those rows
	// produce comes back with them. So a page of 20 over a fan-out of 3 is
	// about 60 rows, and 20 is how far the cursor moved.
	//
	// Every joined row derives from exactly one input-0 row, so paging this way
	// visits every joined row exactly once even while rows are inserted and
	// deleted — which Offset does not, because it counts. Use [Session.PageJoin],
	// and feed its Cursor back in here.
	//
	// Refused rather than served wrongly: a right or full outer join, an Offset
	// alongside it, and anything input 0's own cursor refuses.
	After []Value
	// Paged asks for a cursor on the response. [Session.PageJoin] sets it;
	// sending a cursor implies it, so only a first page needs it.
	//
	// Separate from After for the reason [Query.Paged] is: the first page has
	// no cursor to carry and must still be refused if it can never be resumed.
	Paged bool
	// Compute is values computed per *joined* row, appended after every
	// input's columns and named with [JoinComputed].
	//
	// The arrangement [Query.Compute] uses on one table, lifted one level. What
	// is new is that the expression is evaluated over the joined row, so it may
	// read both sides at once — which is the thing no input's own `Compute` can
	// express, and the reason this field is here rather than there.
	//
	// Each may read every input's columns and the values *before* it, so
	// `Compute[1]` may read `JoinComputed(0)` and not the other way round.
	//
	// An ungrouped join returns them on [JoinStream.Computed]; a grouped one
	// exposes them to `GroupBy` and the aggregates.
	Compute []Scalar
}

// JoinBuilder accumulates inputs and hands out their positions.
//
// An input's position is assigned in call order, which is the order the wire
// declares them in. The builder exists so that position is never written by
// hand: a literal `1` that becomes stale when an input is inserted above it is
// a join that silently answers a different question.
type JoinBuilder struct {
	inputs []JoinInput
}

// NewJoin starts a join.
func NewJoin() *JoinBuilder { return &JoinBuilder{} }

// Add appends an input and returns its position.
func (b *JoinBuilder) Add(input JoinInput) uint32 {
	b.inputs = append(b.inputs, input)
	return uint32(len(b.inputs) - 1)
}

// Query is the join built so far.
func (b *JoinBuilder) Query() JoinQuery { return JoinQuery{Inputs: b.inputs} }

// Inputs is how many inputs have been added.
func (b *JoinBuilder) Inputs() int { return len(b.inputs) }

// toProto renders the join. `schemas` supplies each input's declaration, so a
// join checks every table it reads rather than none of them.
func (q JoinQuery) toProto(schemas Schemas) *pb.JoinQuery {
	out := &pb.JoinQuery{
		Offset:     q.Offset,
		Limit:      q.Limit,
		BuildLimit: q.BuildLimit,
		Compute:    scalarsToProto(q.Compute),
		Paged:      q.Paged,
	}
	// Input 0's key, not a key in the joined space: a page of a join is a page
	// of its driving table, and that is the only table a cursor names.
	for _, value := range q.After {
		out.After = append(out.After, value.toProto())
	}
	for _, input := range q.Inputs {
		query := Query{
			Table:      input.Table,
			Columns:    input.Columns,
			Descending: input.Descending,
		}
		if input.Filter != nil {
			query.Filter = input.Filter
		}
		query.Compute = input.Compute
		wire := &pb.JoinInput{
			Query:    query.toProto(schemas.claimFor(input.Table)),
			JoinType: input.Type.wire(),
		}
		for _, on := range input.On {
			wire.On = append(wire.On, &pb.JoinOn{
				Earlier: on.Earlier.ref(),
				// `Own` carries this input's own position: the server checks
				// that the two sides name different inputs, and a bare
				// ordinal defaulting to input 0 is refused on every input but
				// the first — which is how this was caught.
				Own: At(uint32(len(out.Inputs)), on.Own).ref(),
			})
		}
		if input.Having != nil {
			wire.Having = input.Having.wire
		}
		if forced := input.Force.wire(); forced != nil {
			wire.Force = forced
		}
		out.Inputs = append(out.Inputs, wire)
	}
	return out
}

// AggregateFunction is what an aggregate computes.
type AggregateFunction int

// The aggregate functions.
const (
	// CountRows is `COUNT(*)`: every row, nulls included.
	CountRows AggregateFunction = iota
	// CountColumn is `COUNT(column)`: rows where the column is not null.
	CountColumn
	// CountDistinct is `COUNT(DISTINCT column)`.
	CountDistinct
	// Min is the smallest value.
	Min
	// Max is the largest.
	Max
	// Sum totals them.
	Sum
	// Avg averages them.
	Avg
)

// Aggregate is one computed value over a group.
type Aggregate struct {
	Function AggregateFunction
	// Column is what it reads. Ignored by [CountRows], required by the rest.
	//
	// A [Column], not a bare [Ordinal]: aggregating a join has to be able to
	// name a column of an input other than the first. It could not, and the
	// server said so — "an aggregate names column 6 of table `authors`, which
	// has 3 columns" — because a bare ordinal is resolved against input 0.
	// Python's client took a qualified reference from the start; this one had
	// the wrong type and no test that reached across a join to notice.
	Column Column
}

// Count is `COUNT(*)`.
func Count() Aggregate { return Aggregate{Function: CountRows} }

// CountOf is `COUNT(column)`, which skips nulls.
//
// For a grouped table every column is on input 0, and [Key0] spells that:
// `CountOf(Key0(2))`. For a grouped join, [At] names the input.
func CountOf(c Column) Aggregate { return Aggregate{Function: CountColumn, Column: c} }

// CountDistinctOf is `COUNT(DISTINCT column)`.
func CountDistinctOf(c Column) Aggregate {
	return Aggregate{Function: CountDistinct, Column: c}
}

// MinOf is `MIN(column)`.
func MinOf(c Column) Aggregate { return Aggregate{Function: Min, Column: c} }

// MaxOf is `MAX(column)`.
func MaxOf(c Column) Aggregate { return Aggregate{Function: Max, Column: c} }

// SumOf is `SUM(column)`.
func SumOf(c Column) Aggregate { return Aggregate{Function: Sum, Column: c} }

// AvgOf is `AVG(column)`.
func AvgOf(c Column) Aggregate { return Aggregate{Function: Avg, Column: c} }

func (a Aggregate) toProto() *pb.Aggregate {
	functions := map[AggregateFunction]pb.AggregateFunction{
		CountRows:     pb.AggregateFunction_AGGREGATE_FUNCTION_COUNT,
		CountColumn:   pb.AggregateFunction_AGGREGATE_FUNCTION_COUNT_COLUMN,
		CountDistinct: pb.AggregateFunction_AGGREGATE_FUNCTION_COUNT_DISTINCT,
		Min:           pb.AggregateFunction_AGGREGATE_FUNCTION_MIN,
		Max:           pb.AggregateFunction_AGGREGATE_FUNCTION_MAX,
		Sum:           pb.AggregateFunction_AGGREGATE_FUNCTION_SUM,
		Avg:           pb.AggregateFunction_AGGREGATE_FUNCTION_AVG,
	}
	out := &pb.Aggregate{Function: functions[a.Function]}
	// `COUNT(*)` reads no column, and sending one would be a different
	// aggregate. Every other function requires it.
	if a.Function != CountRows {
		out.Column = a.Column.ref()
	}
	return out
}

// GroupRef names a column of the *grouped result* — a key or an aggregate —
// for use in `Having` and in the ordering over groups.
//
// A raw table column is a kind mismatch the server refuses by name: SQL's
// "column must appear in the GROUP BY clause", made decidable by the wire
// carrying the kind rather than inferring it.
type GroupRef struct{ ref *pb.ColumnRef }

// Key is the `index`th GROUP BY key.
func Key(index uint32) GroupRef {
	return GroupRef{&pb.ColumnRef{Of: &pb.ColumnRef_GroupKey{GroupKey: index}}}
}

// Agg is the `index`th aggregate.
func Agg(index uint32) GroupRef {
	return GroupRef{&pb.ColumnRef{Of: &pb.ColumnRef_Aggregate{Aggregate: index}}}
}

// GroupSortKey is one column of an ordering over groups.
type GroupSortKey struct {
	Column    GroupRef
	Direction Direction
}

// Grouping is a set of aggregates, optionally per group.
//
// Its `Sort`, `Limit` and `Offset` are over *groups*, not over the rows going
// into them — ordering the input rows would change nothing about the answer
// and cutting them would change it in a way nobody means.
type Grouping struct {
	// GroupBy names the key columns. Empty means one group over every row.
	//
	// A [Column], because a grouped join's key can be on either side: `At(1,
	// 1)` is the second input's second column. For a grouped table every key
	// is on input 0, and [Key0] spells that.
	GroupBy []Column
	// Aggregates is what to compute per group. Optional: keys with no
	// aggregates are the distinct combinations of those keys — SELECT
	// DISTINCT. What the server refuses is neither, which asks for one group
	// with nothing in it.
	Aggregates []Aggregate
	// Having keeps groups. Names keys and aggregates, not columns.
	Having *Expr
	// Sort orders the groups.
	Sort []GroupSortKey
	// Limit caps the groups returned.
	Limit *uint64
	// Offset discards groups before the limit applies.
	Offset uint64
}

func (g Grouping) apply(out *pb.AggregateQuery) {
	for _, c := range g.GroupBy {
		out.GroupBy = append(out.GroupBy, c.ref())
	}
	for _, a := range g.Aggregates {
		out.Aggregates = append(out.Aggregates, a.toProto())
	}
	for _, key := range g.Sort {
		direction := pb.SortDirection_SORT_DIRECTION_ASC
		if key.Direction == Desc {
			direction = pb.SortDirection_SORT_DIRECTION_DESC
		}
		out.Sort = append(out.Sort, &pb.SortKey{Column: key.Column.ref, Direction: direction})
	}
	if g.Having != nil {
		out.Having = g.Having.wire
	}
	out.Limit = g.Limit
	out.Offset = g.Offset
}

// Group is one row of a grouped answer.
type Group struct {
	// Key is the group's key values, in `GroupBy` order.
	Key []Value
	// Values is the aggregates, in `Aggregates` order.
	Values []Value
}

// Comparisons over a grouped result, for `Having`.
//
// Separate from the row-level [Eq] and friends because they take a [GroupRef]
// rather than an [Ordinal]: a `HAVING` names keys and aggregates, and a raw
// column there is a kind mismatch. Two families of function rather than one
// overloaded family means the wrong one does not compile, instead of being
// refused at runtime by a server the caller has already deployed against.
func groupCompare(ref GroupRef, op pb.CmpOp, v Value) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_Compare{Compare: &pb.Compare{
		Column: ref.ref,
		Op:     op,
		Value:  v.toProto(),
	}}}}
}

// GroupEq is `key-or-aggregate = value`.
func GroupEq(r GroupRef, v Value) Expr { return groupCompare(r, pb.CmpOp_CMP_OP_EQ, v) }

// GroupNe is `<> value`.
func GroupNe(r GroupRef, v Value) Expr { return groupCompare(r, pb.CmpOp_CMP_OP_NE, v) }

// GroupLt is `< value`.
func GroupLt(r GroupRef, v Value) Expr { return groupCompare(r, pb.CmpOp_CMP_OP_LT, v) }

// GroupLe is `<= value`.
func GroupLe(r GroupRef, v Value) Expr { return groupCompare(r, pb.CmpOp_CMP_OP_LE, v) }

// GroupGt is `> value`.
func GroupGt(r GroupRef, v Value) Expr { return groupCompare(r, pb.CmpOp_CMP_OP_GT, v) }

// GroupGe is `>= value`.
func GroupGe(r GroupRef, v Value) Expr { return groupCompare(r, pb.CmpOp_CMP_OP_GE, v) }

// Key0 names column `ordinal` of the only input, for grouping one table.
//
// `At(0, ordinal)` says the same thing; this reads better where there is no
// join and no second input to distinguish from.
func Key0(ordinal Ordinal) Column { return At(0, ordinal) }
