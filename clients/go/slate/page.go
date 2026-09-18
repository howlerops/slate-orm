package slate

import (
	"context"
	"errors"
	"io"

	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
)

// Page is one page of a keyset-paged read, and where to resume.
//
// Cursor is nil when this page was short and there is provably nothing after
// it. A page that comes back *full* might be the last one, and the only way to
// know is to ask again: the server returns a cursor anyway rather than reading
// one row further to find out, because that extra read would be paid on every
// page to save one empty request at the end of a sequence most callers never
// finish. So a loop that runs until IsLast makes one final request returning
// nothing, which is the ordinary shape of every cursor API and is documented
// rather than optimised away.
type Page struct {
	// Rows, at most Query.Limit of them.
	Rows [][]Value
	// Computed is what Query.Compute produced, one entry per row.
	Computed [][]Value
	// Cursor for the next page, or nil when this one ended the sequence.
	Cursor []Value
	// ServedBy is which store answered.
	ServedBy *ServedBy
}

// IsLast reports whether there is provably nothing after this page.
func (p *Page) IsLast() bool { return p.Cursor == nil }

// Page reads one page of a keyset-paged read, with the cursor for the next.
//
// query.Limit must be set: a page with no size is the whole table, and the
// cursor it would return names its last row. Put the returned Cursor in
// [Query.After] for the page after this one, and stop when IsLast.
//
//	var cursor []slate.Value
//	for {
//		page, err := session.Page(ctx, slate.Query{
//			Table: "books", Limit: slate.Limit(100), After: cursor,
//		})
//		if err != nil {
//			return err
//		}
//		for _, row := range page.Rows {
//			...
//		}
//		if page.IsLast() {
//			break
//		}
//		cursor = page.Cursor
//	}
//
// The whole page is read before this returns, unlike [Session.Query], which
// streams. A page is bounded by its own limit, so there is nothing to stream
// *to* — and the cursor arrives at the end, so a caller would have to drain
// the stream before it could ask for the next page anyway.
func (s *Session) Page(ctx context.Context, query Query) (*Page, error) {
	query.Paged = true
	ctx, cancel := context.WithCancel(s.ctx(ctx))
	defer cancel()

	stream, err := s.client.rpc.Query(ctx, &pb.QueryRequest{
		Query:     query.toProto(s.client.schemas.claimFor(query.Table)),
		Freshness: s.freshness(),
	})
	if err != nil {
		return nil, fromRPC(err)
	}

	page := &Page{}
	for {
		message, err := stream.Recv()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return nil, fromRPC(err)
		}
		if message.ServedBy != nil && page.ServedBy == nil {
			page.ServedBy = &ServedBy{
				Replica:  message.ServedBy.Replica,
				Sequence: message.ServedBy.Sequence,
			}
			s.observeServedBy(message.ServedBy)
		}
		for _, row := range message.Rows {
			decoded, err := rowFromProto(row)
			if err != nil {
				return nil, err
			}
			extra, err := computedFromProto(row)
			if err != nil {
				return nil, err
			}
			page.Rows = append(page.Rows, decoded)
			page.Computed = append(page.Computed, extra)
		}
		// On whichever message carries it, not on the last one: a page whose
		// rows divide evenly into batches gets a trailing message with the
		// cursor and no rows, and one that does not gets it on the final
		// batch. A reader that only looked at the last message would work by
		// accident in the second case and not the first.
		if len(message.NextCursor) > 0 {
			cursor := make([]Value, 0, len(message.NextCursor))
			for _, value := range message.NextCursor {
				decoded, err := valueFromProto(value)
				if err != nil {
					return nil, err
				}
				cursor = append(cursor, decoded)
			}
			page.Cursor = cursor
		}
	}
	return page, nil
}

// JoinPage is one page of a keyset-paged join or chain, and where to resume.
//
// Cursor is a primary key of **input 0's** table, because a page of a join is
// a page of its driving table. Rows holds every joined row those input-0 rows
// produced, so it is usually longer than the page size: a page of 20 over a
// fan-out of 3 is about 60 rows, and 20 is how far the cursor moved.
//
// Cursor is nil when the page was short and there is provably nothing after
// it, where "short" is counted in input-0 rows for the same reason.
type JoinPage struct {
	// Rows, one slice of values per input, as [JoinStream] hands them out.
	Rows [][][]Value
	// Computed is what JoinQuery.Compute produced, one entry per row.
	Computed [][]Value
	// InputComputed is what each input's own Compute produced: one entry per
	// row, then one per input.
	InputComputed [][][]Value
	// Cursor for the next page, or nil when this one ended the sequence.
	Cursor []Value
	// ServedBy is which store answered.
	ServedBy *ServedBy
}

// IsLast reports whether there is provably nothing after this page.
func (p *JoinPage) IsLast() bool { return p.Cursor == nil }

// PageJoin reads one page of a keyset-paged join or chain.
//
// join.Limit must be set, and it counts **input-0 rows**: a page of a join is
// a page of its driving table, and every joined row those rows produce comes
// back with them. Put the returned Cursor in [JoinQuery.After] for the next
// page, and stop when IsLast.
//
//	var cursor []slate.Value
//	for {
//		q.Limit, q.After = slate.Limit(100), cursor
//		page, err := session.PageJoin(ctx, q)
//		if err != nil {
//			return err
//		}
//		for _, row := range page.Rows {
//			...
//		}
//		if page.IsLast() {
//			break
//		}
//		cursor = page.Cursor
//	}
//
// Every joined row derives from exactly one input-0 row, so this visits every
// joined row exactly once even while rows are inserted and deleted — which
// Offset does not, because it counts.
//
// The page is read whole before this returns, as [Session.Page] is and for the
// same reason: the cursor arrives at the end, so a caller would have to drain
// a stream before it could ask for the next page anyway. Note the page is
// bounded in input-0 rows, not in returned rows.
func (s *Session) PageJoin(ctx context.Context, join JoinQuery) (*JoinPage, error) {
	join.Paged = true
	ctx, cancel := context.WithCancel(s.ctx(ctx))
	defer cancel()

	stream, err := s.client.rpc.Join(ctx, &pb.JoinRequest{
		Join:      join.toProto(s.client.schemas),
		Freshness: s.freshness(),
	})
	if err != nil {
		return nil, fromRPC(err)
	}

	page := &JoinPage{}
	for {
		message, err := stream.Recv()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return nil, fromRPC(err)
		}
		if message.ServedBy != nil && page.ServedBy == nil {
			page.ServedBy = &ServedBy{
				Replica:  message.ServedBy.Replica,
				Sequence: message.ServedBy.Sequence,
			}
			s.observeServedBy(message.ServedBy)
		}
		for _, row := range message.Rows {
			inputs, computed, perInput, err := joinedRowFromProto(row)
			if err != nil {
				return nil, err
			}
			page.Rows = append(page.Rows, inputs)
			page.Computed = append(page.Computed, computed)
			page.InputComputed = append(page.InputComputed, perInput)
		}
		// On whichever message carries it, for the reason [Session.Page] gives.
		if len(message.NextCursor) > 0 {
			cursor := make([]Value, 0, len(message.NextCursor))
			for _, value := range message.NextCursor {
				decoded, err := valueFromProto(value)
				if err != nil {
					return nil, err
				}
				cursor = append(cursor, decoded)
			}
			page.Cursor = cursor
		}
	}
	return page, nil
}
