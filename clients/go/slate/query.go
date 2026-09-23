package slate

import (
	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
)

// Ordinal is a column's position in its table, counting from zero.
//
// Positions rather than names, because that is what the wire carries: a name
// would have to be resolved somewhere, and resolving it here would mean this
// client holding a second copy of the schema that could disagree with the
// server's.
type Ordinal uint32

// Expr is a predicate over one table's rows.
type Expr struct{ wire *pb.Expr }

// True admits every row. The zero value of a filter.
func True() Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_Literal{Literal: true}}}
}

// False admits none.
func False() Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_Literal{Literal: false}}}
}

func compare(col Ordinal, op pb.CmpOp, v Value) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_Compare{Compare: &pb.Compare{
		Column: columnRef(col),
		Op:     op,
		Value:  v.toProto(),
	}}}}
}

// Eq is `column = value`.
func Eq(col Ordinal, v Value) Expr { return compare(col, pb.CmpOp_CMP_OP_EQ, v) }

// Ne is `column <> value`.
func Ne(col Ordinal, v Value) Expr { return compare(col, pb.CmpOp_CMP_OP_NE, v) }

// Lt is `column < value`.
func Lt(col Ordinal, v Value) Expr { return compare(col, pb.CmpOp_CMP_OP_LT, v) }

// Le is `column <= value`.
func Le(col Ordinal, v Value) Expr { return compare(col, pb.CmpOp_CMP_OP_LE, v) }

// Gt is `column > value`.
func Gt(col Ordinal, v Value) Expr { return compare(col, pb.CmpOp_CMP_OP_GT, v) }

// Ge is `column >= value`.
func Ge(col Ordinal, v Value) Expr { return compare(col, pb.CmpOp_CMP_OP_GE, v) }

// IsNull is `column IS NULL`.
func IsNull(col Ordinal) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_IsNull{IsNull: &pb.IsNull{
		Column: columnRef(col), Negated: false,
	}}}}
}

// IsNotNull is `column IS NOT NULL`.
func IsNotNull(col Ordinal) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_IsNull{IsNull: &pb.IsNull{
		Column: columnRef(col), Negated: true,
	}}}}
}

// In is `column IN (values)`.
func In(col Ordinal, values ...Value) Expr {
	wire := make([]*pb.Value, 0, len(values))
	for _, v := range values {
		wire = append(wire, v.toProto())
	}
	return Expr{&pb.Expr{Node: &pb.Expr_InList{InList: &pb.InList{
		Column: columnRef(col), Values: wire,
	}}}}
}

// Like is `column LIKE pattern`, with `%` and `_` as the wildcards.
func Like(col Ordinal, pattern string) Expr {
	return like(col, pattern, false, false)
}

// ILike is `column ILIKE pattern`, matching without regard to case.
func ILike(col Ordinal, pattern string) Expr {
	return like(col, pattern, false, true)
}

// NotLike is `column NOT LIKE pattern`.
func NotLike(col Ordinal, pattern string) Expr {
	return like(col, pattern, true, false)
}

// Contains is `every term of text is a term of column`: full-text search.
//
// The search is sent as written and tokenized by the *server*, with the same
// function its write path tokenized the column with. This client deliberately
// does no splitting of its own: a client that split differently would find
// fewer rows than the table holds, with no error anywhere to say so.
//
// Conjunctive — every term must appear. For a disjunction, [Or] two of these.
// A phrase is not expressible: the index holds no positions.
//
// A text index makes this a lookup rather than a scan, but it does not have to
// exist. Without one the server evaluates the same predicate row by row and
// returns the same rows.
func Contains(col Ordinal, text string) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_Contains{Contains: &pb.Contains{
		Column: columnRef(col), Text: text,
	}}}}
}

func like(col Ordinal, pattern string, negated, insensitive bool) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_Like{Like: &pb.Like{
		Column:      columnRef(col),
		Pattern:     pattern,
		Negated:     negated,
		Insensitive: insensitive,
	}}}}
}

// And is the conjunction of every part. With no parts it is [True], which is
// the identity for `AND` and keeps a built-up filter from becoming `False` by
// accident when a loop adds nothing.
func And(parts ...Expr) Expr {
	if len(parts) == 0 {
		return True()
	}
	return Expr{&pb.Expr{Node: &pb.Expr_Conjunction{Conjunction: exprList(parts)}}}
}

// Or is the disjunction. With no parts it is [False], the identity for `OR`.
func Or(parts ...Expr) Expr {
	if len(parts) == 0 {
		return False()
	}
	return Expr{&pb.Expr{Node: &pb.Expr_Disjunction{Disjunction: exprList(parts)}}}
}

// Not inverts a predicate.
func Not(inner Expr) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_Negation{Negation: inner.wire}}}
}

func exprList(parts []Expr) *pb.ExprList {
	out := make([]*pb.Expr, 0, len(parts))
	for _, p := range parts {
		out = append(out, p.wire)
	}
	return &pb.ExprList{Exprs: out}
}

func columnRef(col Ordinal) *pb.ColumnRef {
	return &pb.ColumnRef{Of: &pb.ColumnRef_Column{Column: uint32(col)}}
}

// Operator is a comparison, for [Compare] and [CompareGroup].
//
// The [Eq] family covers the common case of a stored column of the query's own
// table and takes a bare [Ordinal]. This type exists so the same six
// comparisons can be written against any reference — a computed value, a
// column of another input, a value the join computed — without six more
// exported names each.
type Operator int

// The comparison operators.
const (
	// OpEq is `=`.
	OpEq Operator = iota
	// OpNe is `<>`.
	OpNe
	// OpLt is `<`.
	OpLt
	// OpLe is `<=`.
	OpLe
	// OpGt is `>`.
	OpGt
	// OpGe is `>=`.
	OpGe
)

func (o Operator) wire() pb.CmpOp {
	switch o {
	case OpNe:
		return pb.CmpOp_CMP_OP_NE
	case OpLt:
		return pb.CmpOp_CMP_OP_LT
	case OpLe:
		return pb.CmpOp_CMP_OP_LE
	case OpGt:
		return pb.CmpOp_CMP_OP_GT
	case OpGe:
		return pb.CmpOp_CMP_OP_GE
	default:
		return pb.CmpOp_CMP_OP_EQ
	}
}

// Compare is a comparison naming any reference: [At], [ComputedAt],
// [Computed0] or [JoinComputed].
//
// `Compare(Computed0(0), OpGt, I64(10))` filters on a query's first computed
// value; `Eq(2, ...)` remains the short way to say "column 2 of this table".
func Compare(c Column, op Operator, v Value) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_Compare{Compare: &pb.Compare{
		Column: c.ref(),
		Op:     op.wire(),
		Value:  v.toProto(),
	}}}}
}

// IsNullAt is `IS NULL` over any reference. See [Compare].
func IsNullAt(c Column) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_IsNull{IsNull: &pb.IsNull{
		Column: c.ref(), Negated: false,
	}}}}
}

// IsNotNullAt is `IS NOT NULL` over any reference.
func IsNotNullAt(c Column) Expr {
	return Expr{&pb.Expr{Node: &pb.Expr_IsNull{IsNull: &pb.IsNull{
		Column: c.ref(), Negated: true,
	}}}}
}

// Direction is which way a sort key orders.
type Direction int

// The sort directions.
const (
	// Asc orders smallest first.
	Asc Direction = iota
	// Desc orders largest first.
	Desc
)

// SortKey is one column of an ordering.
//
// `Column` is an ordinal of the query's own table. `Ref`, when set, names any
// reference instead — a computed value, say — and wins over `Column`. Two
// fields rather than changing the type of one, because a `SortKey{Column: 2}`
// literal written before computed values existed still means column 2.
type SortKey struct {
	Column    Ordinal
	Direction Direction
	// Ref overrides `Column` with a qualified reference. See [Computed0].
	Ref *Column
}

func (k SortKey) ref() *pb.ColumnRef {
	if k.Ref != nil {
		return k.Ref.ref()
	}
	return columnRef(k.Column)
}

// toProto renders one key. Extracted when windows arrived and needed the same
// conversion for their own ORDER BY: two copies of a direction mapping is two
// places for a direction to be written backwards.
func (k SortKey) toProto() *pb.SortKey {
	direction := pb.SortDirection_SORT_DIRECTION_ASC
	if k.Direction == Desc {
		direction = pb.SortDirection_SORT_DIRECTION_DESC
	}
	return &pb.SortKey{Column: k.ref(), Direction: direction}
}

// WindowFunction is what a [Window] computes for each row of its partition.
type WindowFunction int

// The window functions.
const (
	// RowNumber is the row's position in its partition, from 1.
	RowNumber WindowFunction = iota
	// Rank has peers share a rank and the next one skip the gap: 1, 1, 3.
	Rank
	// DenseRank is the same with no gaps: 1, 1, 2.
	DenseRank
	// Lag is the value Offset rows earlier in the partition's order.
	Lag
	// Lead is the value Offset rows later.
	Lead
	// AggregateOver is an ordinary [Aggregate] over the frame.
	AggregateOver
)

// Window is one value computed over a partition, one per input row.
//
// Not an [Aggregate], and the difference is the cardinality: a grouped read
// folds ten thousand rows into four, and a window answers "what is this row's
// rank among its peers", which has one answer per input row. So a query
// carrying one still returns rows, and the values arrive in
// [RowStream.Windowed] rather than as groups.
//
// # The frame
//
// SumOver(c).Over(...) with no Order is the partition's total, repeated on
// every row. Add an Order and it becomes a *running* total — SQL's own default
// frame changing, not a different spelling, and the server follows the
// standard. Rows tied on the order columns are peers and all see the value
// that includes all of them.
//
// # What it costs
//
// A window has to see every selected row before it can answer for any of them,
// so a query carrying one does not stream and is bounded by the server's
// max_window_rows rather than by its Limit. The limit cannot help: RowNumber
// numbers every row before anything knows which ten are first.
type Window struct {
	Function WindowFunction
	// Aggregate is what [AggregateOver] computes. Ignored by the rest, and
	// setting it on one of them is refused by the server rather than dropped.
	Aggregate Aggregate
	// Column is what [Lag] and [Lead] read. Ignored by the rest.
	Column Column
	// Offset is how far [Lag] and [Lead] step. Zero is refused: it is the
	// current row spelled obscurely.
	Offset uint64
	// Partition is PARTITION BY. Empty is one partition over the whole
	// result, which is what SQL means by omitting the clause — not one
	// partition per row.
	Partition []Column
	// Order is the window's own ORDER BY, which is not the query's. It decides
	// peer groups and turns an aggregate's frame into a running one. Required
	// by every function except [AggregateOver]; the server refuses an
	// unordered rank rather than answering 1 on every row.
	Order []SortKey
}

// RowNumberOver is a [Window] computing [RowNumber]. Add the clause with Over.
func RowNumberOver() Window { return Window{Function: RowNumber} }

// RankOver is a [Window] computing [Rank].
func RankOver() Window { return Window{Function: Rank} }

// DenseRankOver is a [Window] computing [DenseRank].
func DenseRankOver() Window { return Window{Function: DenseRank} }

// LagOver is a [Window] reading `c` from `offset` rows earlier.
//
// One is what LAG(x) means in SQL, and is what to pass unless you mean
// otherwise; zero is refused by the server.
func LagOver(c Column, offset uint64) Window {
	return Window{Function: Lag, Column: c, Offset: offset}
}

// LeadOver is a [Window] reading `c` from `offset` rows later.
func LeadOver(c Column, offset uint64) Window {
	return Window{Function: Lead, Column: c, Offset: offset}
}

// Over is any ordinary [Aggregate], over the frame.
//
// Takes an [Aggregate] rather than repeating its seven constructors, so that
// SumOf(c) means the same thing grouped or windowed and a new aggregate has
// one place to be added.
func Over(a Aggregate) Window {
	return Window{Function: AggregateOver, Aggregate: a}
}

// Over sets the OVER (...) clause and returns the window.
//
// A method rather than two more fields on every constructor: the clause is the
// same for all six functions, and a chain reads the way the SQL does.
func (w Window) Over(partition []Column, order []SortKey) Window {
	w.Partition = partition
	w.Order = order
	return w
}

func (w Window) toProto() *pb.Window {
	functions := map[WindowFunction]pb.WindowFunction{
		RowNumber:     pb.WindowFunction_WINDOW_FUNCTION_ROW_NUMBER,
		Rank:          pb.WindowFunction_WINDOW_FUNCTION_RANK,
		DenseRank:     pb.WindowFunction_WINDOW_FUNCTION_DENSE_RANK,
		Lag:           pb.WindowFunction_WINDOW_FUNCTION_LAG,
		Lead:          pb.WindowFunction_WINDOW_FUNCTION_LEAD,
		AggregateOver: pb.WindowFunction_WINDOW_FUNCTION_AGGREGATE,
	}
	out := &pb.Window{Function: functions[w.Function], Offset: w.Offset}
	// Only the field the function uses, because the server refuses a rank
	// carrying an aggregate rather than ignoring it — and a zero-valued
	// Aggregate is COUNT(*), which would be sent as a real one.
	switch w.Function {
	case AggregateOver:
		out.Aggregate = w.Aggregate.toProto()
	case Lag, Lead:
		out.Column = w.Column.ref()
	}
	for _, c := range w.Partition {
		out.PartitionBy = append(out.PartitionBy, c.ref())
	}
	for _, key := range w.Order {
		out.Order = append(out.Order, key.toProto())
	}
	return out
}

// Query selects rows from one table.
//
// A struct with exported fields rather than a chain of builder methods: Go
// composite literals already read as a builder, and a chain would need a
// second way to say "no limit" that is not the zero value.
type Query struct {
	// Table is the table's name, as the server's catalog spells it.
	Table string
	// Filter admits rows. The zero value means every row.
	Filter *Expr
	// Sort orders the answer. Empty is the access path's own order.
	Sort []SortKey
	// Limit caps the rows returned. Nil is no cap.
	Limit *uint64
	// Offset discards rows before the limit applies.
	Offset uint64
	// Columns is the projection. Empty means every column.
	//
	// Naming fewer is what lets an index answer without reading a row, so it
	// is worth naming them where a caller knows.
	Columns []Ordinal
	// Descending reads the table backwards where the access path allows it.
	Descending bool
	// Compute is values computed per row, appended after the table's own
	// columns and named with [Computed0].
	//
	// A filter, a sort key, a GROUP BY key or an aggregate names one the same
	// way it names a column, so none of them has to learn what an expression
	// is. The `n`th may read the `n` before it and not itself or a later one.
	//
	// They come back in [RowStream.Computed], beside the row rather than as a
	// tail of it, so an ordinal still means a column.
	Compute []Scalar
	// After resumes at the first row strictly after this primary key —
	// keyset pagination. Empty is the first page.
	//
	// Not Offset, which counts rows and is only correct while nothing
	// changes: delete a row ahead of the cursor between two pages and the
	// reader silently skips one, insert one and they see a row twice, and
	// nothing reports either. A key does not move when its neighbours change.
	// It is also cheaper — Offset n reads and discards n rows, where a key
	// lets the range start after the cursor, so every page costs what the
	// first one costs.
	//
	// Use [Session.Page], which sets Paged and hands back the next cursor.
	After []Value
	// Paged asks the server for the cursor to the next page.
	//
	// Separate from After, because the first page has no cursor to resume
	// from and still wants one back. Separate from Limit, because `LIMIT 10`
	// and "the first page of ten" are the same request and different
	// intentions — and the server refuses a page it cannot build a cursor for
	// (no limit, or a projection dropping a key column) rather than serving it
	// without one, which it can only do if it knows one was wanted.
	Paged bool
	// Window is values computed over a partition, one per row, returned in
	// [RowStream.Windowed] and named from a sort key with [Windowed].
	//
	// See [Window] for what a window costs, which the request does not show:
	// a query carrying one does not stream, and no Limit bounds it.
	Window []Window
	// IncludeDeleted also returns rows a soft delete has retired.
	//
	// Needs the `read_deleted` action on the table, which `read` does not
	// imply and the `all` shorthand does not include: a soft delete hides a
	// row from every ordinary read, so lifting it shows rows the application
	// decided were gone. Without the grant the server answers
	// PERMISSION_DENIED naming `read_deleted`, rather than quietly serving the
	// smaller set.
	//
	// On a table that does not soft-delete it does nothing and needs no grant
	// — there is nothing to reveal.
	IncludeDeleted bool
	// Hint asks for a particular access path. Nil leaves the choice to the
	// planner, which is right nearly always.
	//
	// Build it with [UsingIndex] or [UsingTableScan]. See either for why this
	// is advice rather than an instruction.
	Hint *AccessHint
}

// AccessHint is which access path a query asks for. Build one with
// [UsingIndex] or [UsingTableScan]; the zero value asks for nothing.
type AccessHint struct {
	wire *pb.AccessHint
}

// UsingIndex asks the planner to take this index rather than the cheapest
// path.
//
// Advice, not an instruction: an index the table does not have is ignored,
// matching the kernel, because a query that stops working because an index was
// renamed is worse than one that gets slower. The server records that it did
// so in [Explanation.Warnings] — and *only* there, so a plain read cannot tell
// you your hint did nothing.
func UsingIndex(name string) *AccessHint {
	return &AccessHint{&pb.AccessHint{Path: &pb.AccessHint_Index{Index: name}}}
}

// UsingTableScan asks the planner to read the table rather than any index.
//
// Chiefly for a test that wants to compare two access paths' answers: one of
// them has to be the path that cannot be wrong.
func UsingTableScan() *AccessHint {
	return &AccessHint{&pb.AccessHint{Path: &pb.AccessHint_TableScan{TableScan: pb.Unit_UNIT}}}
}

// Limit is a convenience for setting [Query.Limit].
func Limit(n uint64) *uint64 { return &n }

// Filter is a convenience for setting [Query.Filter].
func Filter(e Expr) *Expr { return &e }

// toProto renders the query. `claim` is the caller's declaration of the table,
// or nil where it has none — attached here rather than by each call site,
// because a read that forgets it is a read that is silently unchecked.
func (q Query) toProto(claim *pb.SchemaCheck) *pb.Query {
	out := &pb.Query{
		Table:   q.Table,
		Offset:  q.Offset,
		Schema:  claim,
		Compute: scalarsToProto(q.Compute),
		Paged:   q.Paged,

		IncludeDeleted: q.IncludeDeleted,
	}
	for _, value := range q.After {
		out.After = append(out.After, value.toProto())
	}
	if q.Filter != nil {
		out.Filter = q.Filter.wire
	}
	if q.Hint != nil {
		out.Hint = q.Hint.wire
	}
	if q.Limit != nil {
		out.Limit = q.Limit
	}
	if q.Descending {
		out.Order = pb.ScanOrder_SCAN_ORDER_DESCENDING
	}
	if len(q.Columns) > 0 {
		refs := make([]*pb.ColumnRef, 0, len(q.Columns))
		for _, c := range q.Columns {
			refs = append(refs, columnRef(c))
		}
		out.Projection = &pb.Projection{Columns: refs}
	}
	// Before the sort, here and in the message, because a sort key may name a
	// window and nothing a window names may be a window.
	for _, w := range q.Window {
		out.Window = append(out.Window, w.toProto())
	}
	for _, key := range q.Sort {
		out.Sort = append(out.Sort, key.toProto())
	}
	return out
}
