package slate

import (
	"context"
	"fmt"

	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
)

// Atomicity says whether a batch's operations stand alone or land together.
//
// There is no zero value that means either, on purpose. The two guarantees
// differ only when something fails, so a caller who never chose finds out on
// the day a write in the middle is rejected — and discovers then whether the
// ones before it stayed. [Batch.Do] refuses an unset one, as does the server.
type Atomicity int32

const (
	// AtomicityUnset is the zero value and is refused. It exists so that a
	// zeroed [Batch] cannot silently mean one of the two.
	AtomicityUnset Atomicity = Atomicity(pb.Atomicity_ATOMICITY_UNSPECIFIED)
	// Independent applies each operation on its own, in order. One failing
	// neither undoes the ones before it nor stops the ones after, and each
	// gets its own result. A network optimisation and nothing else.
	Independent Atomicity = Atomicity(pb.Atomicity_ATOMICITY_INDEPENDENT)
	// AllOrNothing applies them in one transaction. Every operation lands or
	// none does, and the first failure fails the call.
	AllOrNothing Atomicity = Atomicity(pb.Atomicity_ATOMICITY_ALL_OR_NOTHING)
)

// A Batch is several writes in one round trip.
//
// Built by appending, because the operations are ordered:
//
//	b := slate.NewBatch(slate.Independent)
//	b.Insert("docs", row)
//	b.DeleteWhere(slate.DeleteWhere{Table: "docs", Filter: filter})
//	result, err := session.Batch(ctx, b)
type Batch struct {
	atomicity  Atomicity
	operations []*pb.BatchOperation
	// The table each operation named, in order, so a returned row decodes
	// against the right one. A batch may span tables, so one table on the
	// result would be wrong for all but the first.
	tables []string
	// Deferred so that a caller building a batch does not have to check an
	// error after every append; [Session.Batch] returns it.
	err error
}

// NewBatch starts one. `atomicity` is a parameter rather than a field to set
// afterwards, so the choice cannot be forgotten.
func NewBatch(atomicity Atomicity) *Batch {
	return &Batch{atomicity: atomicity}
}

// Len is how many operations it carries.
func (b *Batch) Len() int { return len(b.operations) }

func (b *Batch) add(table string, of *pb.BatchOperation) *Batch {
	b.operations = append(b.operations, of)
	b.tables = append(b.tables, table)
	return b
}

// Insert adds rows, refusing a primary key that is taken.
func (b *Batch) Insert(table string, rows ...[]Value) *Batch {
	return b.add(table, &pb.BatchOperation{
		Of: &pb.BatchOperation_Insert{Insert: &pb.InsertRequest{
			Table: table, Rows: rowsToProto(rows),
		}},
	})
}

// Upsert adds rows, replacing any whose primary key is taken.
func (b *Batch) Upsert(table string, rows ...[]Value) *Batch {
	return b.add(table, &pb.BatchOperation{
		Of: &pb.BatchOperation_Insert{Insert: &pb.InsertRequest{
			Table: table, Rows: rowsToProto(rows), Upsert: true,
		}},
	})
}

// Update replaces rows, refusing one whose primary key is not there.
func (b *Batch) Update(table string, rows ...[]Value) *Batch {
	return b.add(table, &pb.BatchOperation{
		Of: &pb.BatchOperation_Update{Update: &pb.UpdateRequest{
			Table: table, Rows: rowsToProto(rows),
		}},
	})
}

// Delete removes rows by primary key.
func (b *Batch) Delete(table string, keys ...[]Value) *Batch {
	return b.add(table, &pb.BatchOperation{
		Of: &pb.BatchOperation_Delete{Delete: &pb.DeleteRequest{
			Table: table, PrimaryKeys: rowsToProto(keys),
		}},
	})
}

// DeleteWhere deletes every row a predicate selects.
func (b *Batch) DeleteWhere(write DeleteWhere) *Batch {
	return b.add(write.Table, &pb.BatchOperation{
		Of: &pb.BatchOperation_DeleteWhere{DeleteWhere: &pb.DeleteWhereRequest{
			Table:     write.Table,
			Filter:    filterProto(write.Filter),
			Returning: write.Returning,
		}},
	})
}

// UpdateWhere assigns to columns of every row a predicate selects.
func (b *Batch) UpdateWhere(write UpdateWhere) *Batch {
	assignments := make([]*pb.Assignment, 0, len(write.Set))
	for _, a := range write.Set {
		assignments = append(assignments, a.toProto())
	}
	return b.add(write.Table, &pb.BatchOperation{
		Of: &pb.BatchOperation_UpdateWhere{UpdateWhere: &pb.UpdateWhereRequest{
			Table:       write.Table,
			Filter:      filterProto(write.Filter),
			Assignments: assignments,
			Returning:   write.Returning,
		}},
	})
}

// A BatchOutcome is one operation's result inside an independent batch.
//
// Exactly one of Written and Err is set. Two fields rather than a WriteResult
// with an error inside it, because a value that can carry both is one somebody
// reads the wrong half of.
type BatchOutcome struct {
	// What the operation did, zero if it failed.
	Written WriteResult
	// Why it failed, nil if it did not.
	//
	// The same *Error a lone call would have returned, rebuilt from the
	// message body rather than from trailers — the request succeeded, so
	// there are no trailers to read.
	Err error
}

// OK reports whether the operation succeeded.
func (o BatchOutcome) OK() bool { return o.Err == nil }

// A BatchResult is what a batch returned.
type BatchResult struct {
	// Where the writer got to, nil inside a transaction.
	Sequence *ReadToken
	// One per operation, in order — but only for an independent batch.
	//
	// Empty for AllOrNothing, which is not a missing feature: they all
	// happened, or the call returned an error and none did.
	Outcomes []BatchOutcome
}

// Failures are the outcomes that failed, which is usually the only half worth
// looking at.
func (r BatchResult) Failures() []BatchOutcome {
	var out []BatchOutcome
	for _, one := range r.Outcomes {
		if !one.OK() {
			out = append(out, one)
		}
	}
	return out
}

// Batch sends several writes in one round trip.
//
// With [Independent] the result carries one outcome per operation, and a
// failed operation is one of those outcomes rather than an error — the request
// succeeded. With [AllOrNothing] the result carries none, and a failure is
// returned as the error, because the operations before it did not happen.
func (s *Session) Batch(ctx context.Context, batch *Batch) (BatchResult, error) {
	return s.runBatch(ctx, batch, "")
}

// Batch runs an atomic batch inside the transaction.
//
// [Independent] is refused here: "independent operations, all of which roll
// back together" is two contradictory requests, and the caller means one.
func (t *Transaction) Batch(ctx context.Context, batch *Batch) (BatchResult, error) {
	return t.session.runBatch(ctx, batch, t.id)
}

func (s *Session) runBatch(ctx context.Context, batch *Batch, transaction string) (BatchResult, error) {
	if batch == nil || batch.Len() == 0 {
		// Refused here rather than at the server, which would also refuse it.
		// A round trip to be told the list was empty is one the caller can be
		// spared.
		return BatchResult{}, fmt.Errorf("slate: a batch needs at least one operation")
	}
	if batch.err != nil {
		return BatchResult{}, batch.err
	}
	if batch.atomicity == AtomicityUnset {
		return BatchResult{}, fmt.Errorf(
			"slate: a batch must say its atomicity: Independent applies each operation on " +
				"its own and reports each separately, AllOrNothing applies them in one " +
				"transaction and fails the call if any of them does",
		)
	}

	// The schema claims are attached here rather than by each `Batch` method,
	// for the reason every other call site attaches them centrally: a method
	// that forgets one is a write that is silently unchecked.
	operations := make([]*pb.BatchOperation, 0, len(batch.operations))
	for at, operation := range batch.operations {
		operations = append(operations, s.claimed(operation, batch.tables[at]))
	}

	response, err := s.client.rpc.Batch(s.ctx(ctx), &pb.BatchRequest{
		Operations:  operations,
		Atomicity:   pb.Atomicity(batch.atomicity),
		Transaction: transaction,
	})
	if err != nil {
		return BatchResult{}, fromRPC(err)
	}

	out := BatchResult{}
	if response.Sequence != nil {
		token := ReadToken(*response.Sequence)
		out.Sequence = &token
		s.mu.Lock()
		s.observeLocked(&token)
		s.mu.Unlock()
	}
	for _, result := range response.Results {
		outcome, err := outcomeFromProto(result)
		if err != nil {
			return BatchResult{}, err
		}
		out.Outcomes = append(out.Outcomes, outcome)
	}
	return out, nil
}

// claimed attaches this client's declaration of `table` to an operation.
func (s *Session) claimed(operation *pb.BatchOperation, table string) *pb.BatchOperation {
	claim := s.client.schemas.claimFor(table)
	switch of := operation.Of.(type) {
	case *pb.BatchOperation_Insert:
		of.Insert.Schema = claim
	case *pb.BatchOperation_Update:
		of.Update.Schema = claim
	case *pb.BatchOperation_Delete:
		of.Delete.Schema = claim
	case *pb.BatchOperation_DeleteWhere:
		of.DeleteWhere.Schema = claim
	case *pb.BatchOperation_UpdateWhere:
		of.UpdateWhere.Schema = claim
	}
	return operation
}

func outcomeFromProto(result *pb.BatchResult) (BatchOutcome, error) {
	switch of := result.Of.(type) {
	case *pb.BatchResult_Error:
		return BatchOutcome{Err: fromBatchError(of.Error)}, nil
	case *pb.BatchResult_Ok:
		written := WriteResult{Affected: of.Ok.Affected}
		if of.Ok.Sequence != nil {
			token := ReadToken(*of.Ok.Sequence)
			written.Sequence = &token
		}
		for _, row := range of.Ok.Rows {
			values, err := rowFromProto(row)
			if err != nil {
				return BatchOutcome{}, err
			}
			written.Rows = append(written.Rows, values)
		}
		return BatchOutcome{Written: written}, nil
	default:
		return BatchOutcome{}, fmt.Errorf("slate: a batch result carried neither an outcome nor an error")
	}
}
