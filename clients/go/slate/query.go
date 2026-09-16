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
	}
	for _, value := range q.After {
		out.After = append(out.After, value.toProto())
	}
	if q.Filter != nil {
		out.Filter = q.Filter.wire
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
	for _, key := range q.Sort {
		direction := pb.SortDirection_SORT_DIRECTION_ASC
		if key.Direction == Desc {
			direction = pb.SortDirection_SORT_DIRECTION_DESC
		}
		out.Sort = append(out.Sort, &pb.SortKey{
			Column:    key.ref(),
			Direction: direction,
		})
	}
	return out
}
