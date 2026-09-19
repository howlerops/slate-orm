package slate

import (
	"context"

	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
)

// DeleteWhere deletes every row a predicate selects, in one statement.
//
// The alternative without it is to query the keys, carry them back and delete
// one per key: N+1 by construction, and not atomic with the query that found
// them — a row inserted in between is missed, and a row deleted in between is
// deleted twice.
//
// A struct rather than a builder, like [Query] beside it, so that the fields
// the server accepts are the fields there are.
type DeleteWhere struct {
	// Table is the table's name, as the server's catalog spells it.
	Table string
	// Filter admits rows. The zero value means every row the caller can see —
	// a `DELETE FROM t` with no `WHERE`, which is a real statement and is
	// allowed. Neither this nor the server can tell it from the mistake it
	// resembles.
	Filter *Expr
	// Returning asks for the rows back, as they were before removal — the only
	// moment they exist to be read.
	//
	// Off by default because the rows are the whole cost: a delete that
	// matched a million rows would send a million of them back.
	Returning bool
}

// UpdateWhere assigns to columns of every row a predicate selects.
type UpdateWhere struct {
	// Table is the table's name, as the server's catalog spells it.
	Table string
	// Filter admits rows. The zero value means every row the caller can see.
	Filter *Expr
	// Set is the assignments, in order. At least one is required; a request
	// with none is refused rather than reported as zero rows written, because
	// zero is what a predicate that matched nothing reports.
	Set []Assignment
	// Returning asks for the rows back, as written.
	Returning bool
}

// An Assignment stores a value into a column.
type Assignment struct {
	// Column is the column to write, by ordinal within the table.
	Column Ordinal
	// Value is evaluated over the row **as it was read**, so
	// `Assign(Views, Add(Col(Views), Lit(I64(1))))` is one write rather than a
	// read, a decision and a write — and two concurrent increments make two.
	//
	// Every assignment in one request reads the original row, so they apply
	// together: assigning a from b and b from a swaps them rather than making
	// both b. Left-to-right is the other reading and it is the one that
	// surprises people; SQL takes this one and so does this.
	Value Scalar
}

// Assign is an [Assignment], spelled for a call site.
func Assign(column Ordinal, value Scalar) Assignment {
	return Assignment{Column: column, Value: value}
}

func (a Assignment) toProto() *pb.Assignment {
	return &pb.Assignment{Column: columnRef(a.Column), Value: a.Value.wire}
}

// DeleteWhere deletes every row the predicate selects.
//
// With Returning set, the result's Rows are the rows as they were before
// removal. Without it they are empty, and the affected count is all that comes
// back.
func (s *Session) DeleteWhere(ctx context.Context, write DeleteWhere) (WriteResult, error) {
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.DeleteWhere(ctx, &pb.DeleteWhereRequest{
			Table:     write.Table,
			Filter:    filterProto(write.Filter),
			Returning: write.Returning,
			Schema:    s.client.schemas.claimFor(write.Table),
		})
	})
}

// PurgeDeleted erases, for good, every row a soft delete retired before an
// instant.
//
// The other half of soft delete. Stamping a column instead of removing a row
// means the row is still there, so a table that only ever soft-deletes grows
// without bound.
//
// `before` is seconds since the epoch and the comparison is strict: a row
// retired exactly then survives. An instant rather than a duration because how
// long retired rows are kept is a deployment's decision — a regulator's
// retention period, a product's undo window — so the caller subtracts.
//
// `atMost` refuses the whole call if more rows than that match, before the
// first is erased; zero means no ceiling. This is the one call here that
// destroys data nobody can get back, and a mistyped `before` is how that
// happens.
//
// Needs `delete` *and* `read_deleted` on the table: erasing a retired row
// means reading it first.
//
// The result's Affected is the count. Rows is always empty — they no longer
// exist to be returned.
func (s *Session) PurgeDeleted(
	ctx context.Context, table string, before int64, atMost uint64,
) (WriteResult, error) {
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.PurgeDeleted(ctx, &pb.PurgeDeletedRequest{
			Table:  table,
			Before: before,
			AtMost: atMost,
			Schema: s.client.schemas.claimFor(table),
		})
	})
}

// UpdateWhere assigns to columns of every row the predicate selects.
//
// With Returning set, the result's Rows are the rows as written.
func (s *Session) UpdateWhere(ctx context.Context, write UpdateWhere) (WriteResult, error) {
	assignments := make([]*pb.Assignment, 0, len(write.Set))
	for _, a := range write.Set {
		assignments = append(assignments, a.toProto())
	}
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.UpdateWhere(ctx, &pb.UpdateWhereRequest{
			Table:       write.Table,
			Filter:      filterProto(write.Filter),
			Assignments: assignments,
			Returning:   write.Returning,
			Schema:      s.client.schemas.claimFor(write.Table),
		})
	})
}

// filterProto is the nil-safe unwrap [Query.toProto] does inline.
//
// A nil filter is "every row", which the server reads as an absent field
// rather than as an empty expression — an empty `Expr` would be a client bug
// the server refuses, and is not the same thing as no filter at all.
func filterProto(filter *Expr) *pb.Expr {
	if filter == nil {
		return nil
	}
	return filter.wire
}

// DeleteWhere deletes every row the predicate selects, inside the transaction.
//
// The session-level method is the same call without a transaction id on it.
// Both exist because [Transaction] is its own type rather than a [Session]
// carrying a flag — so a method added to one is simply absent from the other,
// which is how these two came to be missing until a test tried to use them.
func (t *Transaction) DeleteWhere(ctx context.Context, write DeleteWhere) (WriteResult, error) {
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.DeleteWhere(ctx, &pb.DeleteWhereRequest{
			Transaction: t.id,
			Table:       write.Table,
			Filter:      filterProto(write.Filter),
			Returning:   write.Returning,
			Schema:      t.session.client.schemas.claimFor(write.Table),
		})
	})
}

// UpdateWhere assigns to columns of every row the predicate selects, inside
// the transaction.
func (t *Transaction) UpdateWhere(ctx context.Context, write UpdateWhere) (WriteResult, error) {
	assignments := make([]*pb.Assignment, 0, len(write.Set))
	for _, a := range write.Set {
		assignments = append(assignments, a.toProto())
	}
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.UpdateWhere(ctx, &pb.UpdateWhereRequest{
			Transaction: t.id,
			Table:       write.Table,
			Filter:      filterProto(write.Filter),
			Assignments: assignments,
			Returning:   write.Returning,
			Schema:      t.session.client.schemas.claimFor(write.Table),
		})
	})
}
