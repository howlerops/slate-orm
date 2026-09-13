package slate

import (
	"context"
	"errors"
	"fmt"
	"io"
	"sync"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/metadata"

	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
)

// Identity is who a request runs as.
//
// The head node does not authenticate: it reads three headers a proxy is
// expected to have set. This type only fills them in. Its name is not
// `Credentials` for that reason — nothing here proves anything.
type Identity struct {
	// Principal is the caller's id, tagged: "str:alice", "u64:7", "uuid:…".
	// The tag is required by the server, because 7 as a string and 7 as a
	// number are different principals to it.
	Principal string
	// Tenant is the tenant the caller acts within, tagged the same way.
	Tenant string
	// Roles are the roles the caller holds.
	Roles []string
}

func (id Identity) apply(ctx context.Context) context.Context {
	pairs := make([]string, 0, 6)
	if id.Principal != "" {
		pairs = append(pairs, "slate-principal", id.Principal)
	}
	if id.Tenant != "" {
		pairs = append(pairs, "slate-tenant", id.Tenant)
	}
	if len(id.Roles) > 0 {
		roles := id.Roles[0]
		for _, r := range id.Roles[1:] {
			roles += "," + r
		}
		pairs = append(pairs, "slate-roles", roles)
	}
	if len(pairs) == 0 {
		return ctx
	}
	return metadata.AppendToOutgoingContext(ctx, pairs...)
}

// ReadToken is how far a read has to see.
//
// An opaque sequence from the writer. A caller passes one back to get a read
// that includes its own earlier write, which is the whole reason it exists.
type ReadToken uint64

// ServedBy says which store answered a read, and how far along it was.
type ServedBy struct {
	// Replica names the store, or is empty for the writer.
	Replica string
	// Sequence is how far that store had caught up.
	Sequence uint64
}

// WriteResult is what a write reports.
type WriteResult struct {
	// Sequence is the writer's position after this write, if it said.
	Sequence *ReadToken
	// Affected is how many rows the write touched.
	Affected uint64
}

// Client is a connection to a head node.
//
// Safe for concurrent use. A [Session] carved from it is not.
type Client struct {
	conn     *grpc.ClientConn
	rpc      pb.RecordsClient
	identity Identity
}

// Dial connects to a head node at `target`.
//
// Insecure by default and deliberately: this client is meant to sit behind the
// same proxy that sets its identity headers, and a TLS option here would
// suggest the identity was protected by something, which it is not. Pass
// grpc.WithTransportCredentials to override.
func Dial(target string, identity Identity, opts ...grpc.DialOption) (*Client, error) {
	if len(opts) == 0 {
		opts = []grpc.DialOption{grpc.WithTransportCredentials(insecure.NewCredentials())}
	}
	conn, err := grpc.NewClient(target, opts...)
	if err != nil {
		return nil, fmt.Errorf("slate: dialling %s: %w", target, err)
	}
	return &Client{conn: conn, rpc: pb.NewRecordsClient(conn), identity: identity}, nil
}

// Close releases the connection.
func (c *Client) Close() error { return c.conn.Close() }

// Session is a sequence of operations that keeps its own read position.
//
// Reads through one session never go backwards in time: a read after a write
// sees that write, and a read after a read sees at least what the earlier one
// did. That is what the session is *for* — without it a replica read can be
// served by a node that has not caught up, which is correct and surprising.
//
// Not safe for concurrent use. Take one per goroutine.
type Session struct {
	client *Client
	// watermark is the highest sequence this session has seen.
	watermark *ReadToken
	// monotonic turns the watermark into a freshness floor on every read.
	monotonic bool
	mu        sync.Mutex
}

// Session starts one. Monotonic reads are on, which is the safe default.
func (c *Client) Session() *Session {
	return &Session{client: c, monotonic: true}
}

// SessionWithoutMonotonicReads starts one that does not carry its watermark.
//
// For a caller that genuinely wants the cheapest read available and has
// decided that going backwards is acceptable — a dashboard, a cache warmer.
// Named at length so that choosing it is deliberate.
func (c *Client) SessionWithoutMonotonicReads() *Session {
	return &Session{client: c, monotonic: false}
}

// Watermark is the furthest this session has read or written.
func (s *Session) Watermark() *ReadToken {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.watermark
}

// Observe folds an external token into this session's watermark.
//
// For carrying a position between sessions, or between processes.
func (s *Session) Observe(token ReadToken) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.observeLocked(&token)
}

func (s *Session) observeLocked(token *ReadToken) {
	if token == nil {
		return
	}
	if s.watermark == nil || *token > *s.watermark {
		s.watermark = token
	}
}

// freshness is the floor this session's reads carry.
func (s *Session) freshness() *pb.Freshness {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.monotonic || s.watermark == nil {
		return nil
	}
	return &pb.Freshness{Level: &pb.Freshness_AtLeast{AtLeast: uint64(*s.watermark)}}
}

func (s *Session) observeServedBy(sb *pb.ServedBy) {
	if sb == nil {
		return
	}
	token := ReadToken(sb.Sequence)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.observeLocked(&token)
}

func (s *Session) ctx(ctx context.Context) context.Context {
	return s.client.identity.apply(ctx)
}

func rowToProto(values []Value) *pb.Row {
	out := make([]*pb.Value, 0, len(values))
	for _, v := range values {
		out = append(out, v.toProto())
	}
	return &pb.Row{Values: out}
}

func rowsToProto(rows [][]Value) []*pb.Row {
	out := make([]*pb.Row, 0, len(rows))
	for _, r := range rows {
		out = append(out, rowToProto(r))
	}
	return out
}

// rowFromProto decodes a row, refusing one that carries computed values.
//
// A computed value is not a column, and a caller that indexed past the end of
// the table's own columns would get one silently. `Query` puts them in
// `Computed`, and this is the read path for stored rows.
func rowFromProto(row *pb.Row) ([]Value, error) {
	if row == nil {
		return nil, nil
	}
	out := make([]Value, 0, len(row.Values))
	for i, v := range row.Values {
		decoded, err := valueFromProto(v)
		if err != nil {
			return nil, fmt.Errorf("column %d: %w", i, err)
		}
		out = append(out, decoded)
	}
	return out, nil
}

func (s *Session) write(
	ctx context.Context,
	call func(context.Context) (*pb.WriteResponse, error),
) (WriteResult, error) {
	var trailers metadata.MD
	_ = trailers
	response, err := call(s.ctx(ctx))
	if err != nil {
		return WriteResult{}, fromRPC(err)
	}
	out := WriteResult{Affected: response.Affected}
	if response.Sequence != nil {
		token := ReadToken(*response.Sequence)
		out.Sequence = &token
		s.mu.Lock()
		s.observeLocked(&token)
		s.mu.Unlock()
	}
	return out, nil
}

// Insert adds rows, refusing a primary key that is taken.
func (s *Session) Insert(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.Insert(ctx, &pb.InsertRequest{
			Table: table, Rows: rowsToProto(rows),
		})
	})
}

// Upsert adds rows, replacing any whose primary key is taken.
func (s *Session) Upsert(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.Insert(ctx, &pb.InsertRequest{
			Table: table, Rows: rowsToProto(rows), Upsert: true,
		})
	})
}

// Update replaces rows, refusing one whose primary key is not there.
func (s *Session) Update(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.Update(ctx, &pb.UpdateRequest{
			Table: table, Rows: rowsToProto(rows),
		})
	})
}

// Delete removes rows by primary key.
func (s *Session) Delete(ctx context.Context, table string, keys ...[]Value) (WriteResult, error) {
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.Delete(ctx, &pb.DeleteRequest{
			Table: table, PrimaryKeys: rowsToProto(keys),
		})
	})
}

// Get reads one row by primary key.
//
// The second return is whether it was found, which is separate from the error
// so that "no such row" is not an error path: a caller checking existence
// should not have to match on an error type to do it.
func (s *Session) Get(ctx context.Context, table string, key []Value) ([]Value, bool, error) {
	response, err := s.client.rpc.Get(s.ctx(ctx), &pb.GetRequest{
		Table:      table,
		PrimaryKey: rowToProto(key),
		Freshness:  s.freshness(),
	})
	if err != nil {
		return nil, false, fromRPC(err)
	}
	s.observeServedBy(response.ServedBy)
	if !response.Found {
		return nil, false, nil
	}
	row, err := rowFromProto(response.Row)
	if err != nil {
		return nil, false, err
	}
	return row, true, nil
}

// RowStream is rows arriving in batches.
//
// Close it when finished, which cancels the call: a stream abandoned without
// closing keeps the server producing rows nobody will read.
type RowStream struct {
	stream   grpc.ServerStreamingClient[pb.QueryResponse]
	cancel   context.CancelFunc
	session  *Session
	batch    [][]Value
	at       int
	servedBy *ServedBy
	warnings []string
	done     bool
	err      error
}

// Query reads rows.
func (s *Session) Query(ctx context.Context, query Query) (*RowStream, error) {
	ctx, cancel := context.WithCancel(s.ctx(ctx))
	stream, err := s.client.rpc.Query(ctx, &pb.QueryRequest{
		Query: query.toProto(), Freshness: s.freshness(),
	})
	if err != nil {
		cancel()
		return nil, fromRPC(err)
	}
	return &RowStream{stream: stream, cancel: cancel, session: s}, nil
}

// Next advances to the next row, reporting false when there are none left or
// the stream failed. Check [RowStream.Err] afterwards.
func (r *RowStream) Next() bool {
	for r.at >= len(r.batch) {
		if r.done {
			return false
		}
		message, err := r.stream.Recv()
		if errors.Is(err, io.EOF) {
			r.done = true
			return false
		}
		if err != nil {
			r.err = withTrailers(fromRPC(err), r.trailers())
			r.done = true
			return false
		}
		if message.ServedBy != nil && r.servedBy == nil {
			r.servedBy = &ServedBy{
				Replica:  message.ServedBy.Replica,
				Sequence: message.ServedBy.Sequence,
			}
			r.session.observeServedBy(message.ServedBy)
		}
		if len(message.Warnings) > 0 {
			r.warnings = append(r.warnings, message.Warnings...)
		}
		r.batch = r.batch[:0]
		r.at = 0
		for _, row := range message.Rows {
			decoded, err := rowFromProto(row)
			if err != nil {
				r.err = err
				r.done = true
				return false
			}
			r.batch = append(r.batch, decoded)
		}
	}
	return true
}

func (r *RowStream) trailers() metadata.MD {
	if r.stream == nil {
		return nil
	}
	return r.stream.Trailer()
}

// Row is the row [RowStream.Next] advanced to.
func (r *RowStream) Row() []Value {
	if r.at >= len(r.batch) {
		return nil
	}
	row := r.batch[r.at]
	r.at++
	return row
}

// Err is why the stream stopped, or nil if it simply ended.
func (r *RowStream) Err() error { return r.err }

// ServedBy is which store answered, known once the first message has arrived.
func (r *RowStream) ServedBy() *ServedBy { return r.servedBy }

// Warnings are what the server said about the request it did serve — an
// unusable hint, a clamped limit. Carried on the first message only.
func (r *RowStream) Warnings() []string { return r.warnings }

// Close cancels the call and releases it.
func (r *RowStream) Close() { r.cancel() }

// Collect drains a stream into a slice, closing it.
//
// For the common case where the answer is known to be small. A caller reading
// a large table should iterate instead.
func (r *RowStream) Collect() ([][]Value, error) {
	defer r.Close()
	var out [][]Value
	for r.Next() {
		out = append(out, r.Row())
	}
	return out, r.Err()
}

// Transaction is a set of operations that commit or roll back together.
type Transaction struct {
	session *Session
	id      string
	done    bool
}

// Begin opens a transaction.
//
// Every operation on it goes to the writer, and its reads see its own
// uncommitted writes. Roll back or commit: an abandoned transaction holds a
// slot until the node's idle timeout collects it.
func (s *Session) Begin(ctx context.Context) (*Transaction, error) {
	response, err := s.client.rpc.Begin(s.ctx(ctx), &pb.BeginRequest{})
	if err != nil {
		return nil, fromRPC(err)
	}
	return &Transaction{session: s, id: response.Transaction}, nil
}

// ID is the server's handle for this transaction.
func (t *Transaction) ID() string { return t.id }

// Commit makes the transaction's writes visible.
func (t *Transaction) Commit(ctx context.Context) error {
	if t.done {
		return fmt.Errorf("slate: this transaction is already finished")
	}
	t.done = true
	response, err := t.session.client.rpc.Commit(
		t.session.ctx(ctx), &pb.CommitRequest{Transaction: t.id})
	if err != nil {
		return fromRPC(err)
	}
	if response.Sequence != nil {
		token := ReadToken(*response.Sequence)
		t.session.mu.Lock()
		t.session.observeLocked(&token)
		t.session.mu.Unlock()
	}
	return nil
}

// Rollback discards the transaction's writes.
//
// Safe to call after Commit, where it does nothing — which is what makes
// `defer tx.Rollback(ctx)` the right shape for every transaction.
func (t *Transaction) Rollback(ctx context.Context) error {
	if t.done {
		return nil
	}
	t.done = true
	_, err := t.session.client.rpc.Rollback(
		t.session.ctx(ctx), &pb.RollbackRequest{Transaction: t.id})
	return fromRPC(err)
}

// Insert adds rows inside the transaction.
func (t *Transaction) Insert(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.Insert(ctx, &pb.InsertRequest{
			Transaction: t.id, Table: table, Rows: rowsToProto(rows),
		})
	})
}

// Upsert adds or replaces rows inside the transaction.
func (t *Transaction) Upsert(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.Insert(ctx, &pb.InsertRequest{
			Transaction: t.id, Table: table, Rows: rowsToProto(rows), Upsert: true,
		})
	})
}

// Update replaces rows inside the transaction.
func (t *Transaction) Update(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.Update(ctx, &pb.UpdateRequest{
			Transaction: t.id, Table: table, Rows: rowsToProto(rows),
		})
	})
}

// Delete removes rows by primary key inside the transaction.
func (t *Transaction) Delete(ctx context.Context, table string, keys ...[]Value) (WriteResult, error) {
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.Delete(ctx, &pb.DeleteRequest{
			Transaction: t.id, Table: table, PrimaryKeys: rowsToProto(keys),
		})
	})
}

// Get reads one row inside the transaction, seeing its uncommitted writes.
func (t *Transaction) Get(ctx context.Context, table string, key []Value) ([]Value, bool, error) {
	response, err := t.session.client.rpc.Get(t.session.ctx(ctx), &pb.GetRequest{
		Transaction: t.id, Table: table, PrimaryKey: rowToProto(key),
	})
	if err != nil {
		return nil, false, fromRPC(err)
	}
	if !response.Found {
		return nil, false, nil
	}
	row, err := rowFromProto(response.Row)
	if err != nil {
		return nil, false, err
	}
	return row, true, nil
}

// Query reads rows inside the transaction.
func (t *Transaction) Query(ctx context.Context, query Query) (*RowStream, error) {
	ctx, cancel := context.WithCancel(t.session.ctx(ctx))
	stream, err := t.session.client.rpc.Query(ctx, &pb.QueryRequest{
		Transaction: t.id, Query: query.toProto(),
	})
	if err != nil {
		cancel()
		return nil, fromRPC(err)
	}
	return &RowStream{stream: stream, cancel: cancel, session: t.session}, nil
}

// Explanation is the plan a query would run under.
type Explanation struct {
	Table         string
	Access        string
	Residual      string
	Descending    bool
	EstimatedRows float64
	EstimatedCost float64
	Sorts         bool
	IndexOnly     bool
	Display       string
	Warnings      []string
}

// Explain asks for a plan without running it.
//
// Requires the `explain` action on every table involved, which a `read` grant
// does not carry: a plan is costed against statistics covering rows the
// caller's policy may hide.
func (s *Session) Explain(ctx context.Context, query Query) (*Explanation, error) {
	response, err := s.client.rpc.Explain(s.ctx(ctx), &pb.ExplainRequest{
		Query: query.toProto(), Freshness: s.freshness(),
	})
	if err != nil {
		return nil, fromRPC(err)
	}
	s.observeServedBy(response.ServedBy)
	return &Explanation{
		Table:         response.Table,
		Access:        response.Access,
		Residual:      response.Residual,
		Descending:    response.Descending,
		EstimatedRows: response.EstimatedRows,
		EstimatedCost: response.EstimatedCost,
		Sorts:         response.Sorts,
		IndexOnly:     response.IndexOnly,
		Display:       response.Display,
		Warnings:      response.Warnings,
	}, nil
}

// Leadership is what a node says about who holds the writer lease.
type Leadership struct {
	// Leader is whether this node holds it.
	Leader bool
	// Generation is the lease's term, if this node knows one.
	Generation *uint64
	// Holder names whoever holds it.
	Holder string
	// SteppedDownBecause is why this node gave it up, if it did.
	SteppedDownBecause string
}

// Leadership asks a node about the writer lease.
//
// Answered by any node, leader or not, which is the point: a follower's answer
// is how a caller finds the leader.
func (c *Client) Leadership(ctx context.Context) (*Leadership, error) {
	response, err := c.rpc.Leadership(c.identity.apply(ctx), &pb.LeadershipRequest{})
	if err != nil {
		return nil, fromRPC(err)
	}
	return &Leadership{
		Leader:             response.Standing == pb.LeadershipStatus_STANDING_LEADER,
		Generation:         response.Generation,
		Holder:             response.Holder,
		SteppedDownBecause: response.SteppedDownBecause,
	}, nil
}

// JoinStream is joined rows arriving in batches.
//
// A joined row is one slice of values per input, kept separate rather than
// concatenated: an outer join pads an unmatched side with nulls, and a flat
// row cannot tell "the right side had no match" from "the right side matched
// and its columns are null".
type JoinStream struct {
	stream   grpc.ServerStreamingClient[pb.JoinResponse]
	cancel   context.CancelFunc
	session  *Session
	batch    [][][]Value
	at       int
	servedBy *ServedBy
	warnings []string
	done     bool
	err      error
}

// Join reads joined rows.
func (s *Session) Join(ctx context.Context, join JoinQuery) (*JoinStream, error) {
	ctx, cancel := context.WithCancel(s.ctx(ctx))
	stream, err := s.client.rpc.Join(ctx, &pb.JoinRequest{
		Join: join.toProto(), Freshness: s.freshness(),
	})
	if err != nil {
		cancel()
		return nil, fromRPC(err)
	}
	return &JoinStream{stream: stream, cancel: cancel, session: s}, nil
}

// Join reads joined rows inside the transaction.
func (t *Transaction) Join(ctx context.Context, join JoinQuery) (*JoinStream, error) {
	ctx, cancel := context.WithCancel(t.session.ctx(ctx))
	stream, err := t.session.client.rpc.Join(ctx, &pb.JoinRequest{
		Transaction: t.id, Join: join.toProto(),
	})
	if err != nil {
		cancel()
		return nil, fromRPC(err)
	}
	return &JoinStream{stream: stream, cancel: cancel, session: t.session}, nil
}

// Next advances to the next joined row.
func (j *JoinStream) Next() bool {
	for j.at >= len(j.batch) {
		if j.done {
			return false
		}
		message, err := j.stream.Recv()
		if errors.Is(err, io.EOF) {
			j.done = true
			return false
		}
		if err != nil {
			j.err = withTrailers(fromRPC(err), j.stream.Trailer())
			j.done = true
			return false
		}
		if message.ServedBy != nil && j.servedBy == nil {
			j.servedBy = &ServedBy{
				Replica:  message.ServedBy.Replica,
				Sequence: message.ServedBy.Sequence,
			}
			j.session.observeServedBy(message.ServedBy)
		}
		j.warnings = append(j.warnings, message.Warnings...)
		j.batch = j.batch[:0]
		j.at = 0
		for _, joined := range message.Rows {
			inputs := make([][]Value, 0, len(joined.Inputs))
			for _, input := range joined.Inputs {
				// A nil `Row` is an unmatched side of an outer join, and stays
				// nil here so a caller can tell it from a row of nulls.
				if input.Row == nil {
					inputs = append(inputs, nil)
					continue
				}
				row, err := rowFromProto(input.Row)
				if err != nil {
					j.err = err
					j.done = true
					return false
				}
				inputs = append(inputs, row)
			}
			j.batch = append(j.batch, inputs)
		}
	}
	return true
}

// Row is the joined row [JoinStream.Next] advanced to: one slice per input,
// nil where an outer join found no match.
func (j *JoinStream) Row() [][]Value {
	if j.at >= len(j.batch) {
		return nil
	}
	row := j.batch[j.at]
	j.at++
	return row
}

// Err is why the stream stopped, or nil if it simply ended.
func (j *JoinStream) Err() error { return j.err }

// ServedBy is which store answered.
func (j *JoinStream) ServedBy() *ServedBy { return j.servedBy }

// Warnings are what the server said about the request it served.
func (j *JoinStream) Warnings() []string { return j.warnings }

// Close cancels the call.
func (j *JoinStream) Close() { j.cancel() }

// Collect drains a stream into a slice, closing it.
func (j *JoinStream) Collect() ([][][]Value, error) {
	defer j.Close()
	var out [][][]Value
	for j.Next() {
		out = append(out, j.Row())
	}
	return out, j.Err()
}

// GroupStream is groups arriving in batches.
type GroupStream struct {
	stream   grpc.ServerStreamingClient[pb.AggregateResponse]
	cancel   context.CancelFunc
	session  *Session
	batch    []Group
	at       int
	servedBy *ServedBy
	warnings []string
	done     bool
	err      error
}

// Aggregate groups one table.
func (s *Session) Aggregate(
	ctx context.Context,
	over Query,
	grouping Grouping,
) (*GroupStream, error) {
	wire := &pb.AggregateQuery{Input: over.toProto()}
	grouping.apply(wire)
	return s.aggregate(ctx, wire, "")
}

// AggregateJoin groups a join.
//
// Two inputs exactly: the kernel groups a two-table join and does not group a
// chain. A third is refused by the server with that as the reason, rather than
// counted here where the count could drift from the kernel's.
func (s *Session) AggregateJoin(
	ctx context.Context,
	over JoinQuery,
	grouping Grouping,
) (*GroupStream, error) {
	wire := &pb.AggregateQuery{Join: over.toProto()}
	grouping.apply(wire)
	return s.aggregate(ctx, wire, "")
}

// Aggregate groups one table inside the transaction.
func (t *Transaction) Aggregate(
	ctx context.Context,
	over Query,
	grouping Grouping,
) (*GroupStream, error) {
	wire := &pb.AggregateQuery{Input: over.toProto()}
	grouping.apply(wire)
	return t.session.aggregate(ctx, wire, t.id)
}

// AggregateJoin groups a join inside the transaction.
func (t *Transaction) AggregateJoin(
	ctx context.Context,
	over JoinQuery,
	grouping Grouping,
) (*GroupStream, error) {
	wire := &pb.AggregateQuery{Join: over.toProto()}
	grouping.apply(wire)
	return t.session.aggregate(ctx, wire, t.id)
}

func (s *Session) aggregate(
	ctx context.Context,
	query *pb.AggregateQuery,
	transaction string,
) (*GroupStream, error) {
	ctx, cancel := context.WithCancel(s.ctx(ctx))
	request := &pb.AggregateRequest{Transaction: transaction, Aggregate: query}
	// A transaction's reads go to the writer and need no freshness floor.
	if transaction == "" {
		request.Freshness = s.freshness()
	}
	stream, err := s.client.rpc.Aggregate(ctx, request)
	if err != nil {
		cancel()
		return nil, fromRPC(err)
	}
	return &GroupStream{stream: stream, cancel: cancel, session: s}, nil
}

// Next advances to the next group.
func (g *GroupStream) Next() bool {
	for g.at >= len(g.batch) {
		if g.done {
			return false
		}
		message, err := g.stream.Recv()
		if errors.Is(err, io.EOF) {
			g.done = true
			return false
		}
		if err != nil {
			g.err = withTrailers(fromRPC(err), g.stream.Trailer())
			g.done = true
			return false
		}
		if message.ServedBy != nil && g.servedBy == nil {
			g.servedBy = &ServedBy{
				Replica:  message.ServedBy.Replica,
				Sequence: message.ServedBy.Sequence,
			}
			g.session.observeServedBy(message.ServedBy)
		}
		g.warnings = append(g.warnings, message.Warnings...)
		g.batch = g.batch[:0]
		g.at = 0
		for _, group := range message.Groups {
			key, err := decodeValues(group.Key)
			if err != nil {
				g.err = err
				g.done = true
				return false
			}
			values, err := decodeValues(group.Values)
			if err != nil {
				g.err = err
				g.done = true
				return false
			}
			g.batch = append(g.batch, Group{Key: key, Values: values})
		}
	}
	return true
}

func decodeValues(wire []*pb.Value) ([]Value, error) {
	out := make([]Value, 0, len(wire))
	for i, v := range wire {
		decoded, err := valueFromProto(v)
		if err != nil {
			return nil, fmt.Errorf("value %d: %w", i, err)
		}
		out = append(out, decoded)
	}
	return out, nil
}

// Group is the group [GroupStream.Next] advanced to.
func (g *GroupStream) Group() Group {
	if g.at >= len(g.batch) {
		return Group{}
	}
	group := g.batch[g.at]
	g.at++
	return group
}

// Err is why the stream stopped, or nil if it simply ended.
func (g *GroupStream) Err() error { return g.err }

// ServedBy is which store answered.
func (g *GroupStream) ServedBy() *ServedBy { return g.servedBy }

// Warnings are what the server said about the request it served.
func (g *GroupStream) Warnings() []string { return g.warnings }

// Close cancels the call.
func (g *GroupStream) Close() { g.cancel() }

// Collect drains a stream into a slice, closing it.
func (g *GroupStream) Collect() ([]Group, error) {
	defer g.Close()
	var out []Group
	for g.Next() {
		out = append(out, g.Group())
	}
	return out, g.Err()
}

// JoinInputPlan is how one input of a join is planned.
type JoinInputPlan struct {
	// Plan is the input's own access path.
	Plan Explanation
	// Type is how unmatched rows are treated.
	Type JoinType
	// Algorithm is what the planner chose, as the server spells it.
	Algorithm string
	// EstimatedRows and EstimatedCost are for the join up to and including
	// this input, not for the input alone.
	EstimatedRows float64
	EstimatedCost float64
}

// JoinExplanation is the plan a join would run under.
type JoinExplanation struct {
	Inputs        []JoinInputPlan
	EstimatedRows float64
	EstimatedCost float64
	Display       string
	Warnings      []string
}

// ExplainJoin asks for a join's plan without running it.
//
// Needs the `explain` action on every table involved, not just one.
func (s *Session) ExplainJoin(ctx context.Context, join JoinQuery) (*JoinExplanation, error) {
	response, err := s.client.rpc.ExplainJoin(s.ctx(ctx), &pb.ExplainJoinRequest{
		Join: join.toProto(), Freshness: s.freshness(),
	})
	if err != nil {
		return nil, fromRPC(err)
	}
	s.observeServedBy(response.ServedBy)
	out := &JoinExplanation{
		EstimatedRows: response.EstimatedRows,
		EstimatedCost: response.EstimatedCost,
		Display:       response.Display,
		Warnings:      response.Warnings,
	}
	for _, input := range response.Inputs {
		plan := JoinInputPlan{
			EstimatedRows: input.EstimatedRows,
			EstimatedCost: input.EstimatedCost,
		}
		switch input.JoinType {
		case pb.JoinType_JOIN_TYPE_LEFT:
			plan.Type = Left
		case pb.JoinType_JOIN_TYPE_RIGHT:
			plan.Type = Right
		case pb.JoinType_JOIN_TYPE_FULL:
			plan.Type = Full
		default:
			plan.Type = Inner
		}
		if input.Algorithm != nil {
			switch input.Algorithm.Algorithm.(type) {
			case *pb.JoinAlgorithm_NestedLoop:
				plan.Algorithm = "nested loop"
			case *pb.JoinAlgorithm_HashBuild:
				plan.Algorithm = "hash"
			}
		}
		if input.Plan != nil {
			plan.Plan = Explanation{
				Table:         input.Plan.Table,
				Access:        input.Plan.Access,
				Residual:      input.Plan.Residual,
				Descending:    input.Plan.Descending,
				EstimatedRows: input.Plan.EstimatedRows,
				EstimatedCost: input.Plan.EstimatedCost,
				Sorts:         input.Plan.Sorts,
				IndexOnly:     input.Plan.IndexOnly,
				Display:       input.Plan.Display,
				Warnings:      input.Plan.Warnings,
			}
		}
		out.Inputs = append(out.Inputs, plan)
	}
	return out, nil
}
