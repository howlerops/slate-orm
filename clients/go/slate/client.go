package slate

import (
	"context"
	"crypto/rand"
	"encoding/hex"
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

// RequestIDKey is the metadata key carrying a caller's label for one call.
//
// Exported because it is half of a contract with the server, spelled the same
// way in crates/slate-server/src/auth.rs. Not an identity key: the server
// decides nothing from it and never authenticates it — it exists so a caller
// holding a failure can find the line the daemon logged.
const RequestIDKey = "slate-request-id"

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
	// Rows are the rows the write touched, when Returning asked for them.
	//
	// Empty unless asked, and empty on a write that matched nothing. Only the
	// predicate writes can fill it: Insert and Update are given whole rows and
	// this server applies no DEFAULT to them, so the row written is the row
	// sent and returning it would hand back the request. A predicate write is
	// the other case — the caller named a condition, and which rows matched is
	// a fact it does not have.
	Rows [][]Value
}

// Client is a connection to a head node.
//
// Safe for concurrent use. A [Session] carved from it is not.
type Client struct {
	conn     *grpc.ClientConn
	rpc      pb.RecordsClient
	identity Identity
	schemas  Schemas
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

// Declaring attaches table declarations, so every request naming one of them
// carries a schema check.
//
// Optional and per-table: a table with no declaration sends no claim and is
// served as before. Worth doing for any table whose column *order* this client
// hard-codes, which is all of them — see [TableDef].
func (c *Client) Declaring(schemas Schemas) *Client {
	c.schemas = schemas
	return c
}

// Schema is this client's declaration of a table, if it has one.
func (c *Client) Schema(table string) (TableDef, bool) {
	def, ok := c.schemas[table]
	return def, ok
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

// requestIDKey is the context key under which ctx stashes the id it generated.
//
// An unexported type so nothing outside this package can collide with it,
// which is the standard rule for context keys and is not optional here: the
// value is read back by fromRPC to name a failure, and a collision would make
// it report somebody else's string.
type requestIDKey struct{}

// newRequestID is one call's label for the daemon's request log.
//
// A UUID's worth of randomness in hex. Per call rather than per session or per
// connection: the point is to name one line in the log, and a session-wide id
// names every line the session wrote, which is what a caller already has.
//
// crypto/rand rather than math/rand, not for secrecy — this value is a label
// and the server trusts nothing about it — but because math/rand's global
// source is seeded per process, and two processes started in the same instant
// would generate the same ids and collide in exactly the log somebody is
// reading to tell them apart.
func newRequestID() string {
	var raw [16]byte
	if _, err := rand.Read(raw[:]); err != nil {
		// Cannot happen on any supported platform, and if it somehow does,
		// losing a log label is not worth failing a request over. An empty id
		// is sent as no id at all, and the server logs `id=-`.
		return ""
	}
	return hex.EncodeToString(raw[:])
}

// ctx attaches this session's identity and a fresh request id.
//
// Every call site must assign the result back over its own ctx — `ctx =
// s.ctx(ctx)` — because fromRPC reads the id out of the context it is handed.
// A site that passes s.ctx(ctx) inline to the RPC and then the *outer* ctx to
// fromRPC would send an id and report none, which is the silent half of a
// correlation being useless. TestEveryCallKindNamesItsRequestID exists to make
// that a failure rather than a gap.
func (s *Session) ctx(ctx context.Context) context.Context {
	id := newRequestID()
	ctx = context.WithValue(ctx, requestIDKey{}, id)
	if id != "" {
		ctx = metadata.AppendToOutgoingContext(ctx, RequestIDKey, id)
	}
	return s.client.identity.apply(ctx)
}

// OutgoingForTest is the metadata one call would carry, without making it.
//
// Exported for the suite alone, and named so. Go's test files for this package
// are external (`package slate_test`), which is deliberate -- they exercise
// what a caller can reach -- and the outgoing metadata is not otherwise
// reachable from outside. The alternative was an internal test file duplicating
// the harness, or trusting that a header nobody asserts is being sent, which is
// exactly the thing this method exists to stop.
func (s *Session) OutgoingForTest(ctx context.Context) metadata.MD {
	out, _ := metadata.FromOutgoingContext(s.ctx(ctx))
	return out
}

// requestIDOf is the id ctx put in this context, or "".
func requestIDOf(ctx context.Context) string {
	id, _ := ctx.Value(requestIDKey{}).(string)
	return id
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

// rowFromProto decodes a row's stored columns, leaving its computed values
// behind.
//
// A computed value is not a column, and a caller that indexed past the end of
// the table's own columns would get one silently — so they travel in `Row.computed`
// on the wire and come back through `computedFromProto` here.
//
// This comment used to say the function *refused* such a row. It never did: it
// read `row.Values` and ignored `row.Computed`, which is the right behaviour
// and was described as a different one. Nothing depended on the wrong reading,
// because until now no Go request could produce a computed value at all.
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

// computedFromProto decodes a row's computed values, which are carried apart
// from its columns for the reason [rowFromProto] gives.
func computedFromProto(row *pb.Row) ([]Value, error) {
	if row == nil || len(row.Computed) == 0 {
		return nil, nil
	}
	out := make([]Value, 0, len(row.Computed))
	for i, v := range row.Computed {
		decoded, err := valueFromProto(v)
		if err != nil {
			return nil, fmt.Errorf("computed value %d: %w", i, err)
		}
		out = append(out, decoded)
	}
	return out, nil
}

// valuesFromProto decodes a bare list of values, for a join's own computed
// values — which belong to no input and so are not inside any `Row`.
func valuesFromProto(values []*pb.Value, what string) ([]Value, error) {
	if len(values) == 0 {
		return nil, nil
	}
	out := make([]Value, 0, len(values))
	for i, v := range values {
		decoded, err := valueFromProto(v)
		if err != nil {
			return nil, fmt.Errorf("%s %d: %w", what, i, err)
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
	ctx = s.ctx(ctx)
	response, err := call(ctx)
	if err != nil {
		return WriteResult{}, fromRPC(ctx, err)
	}
	out := WriteResult{Affected: response.Affected}
	for _, row := range response.Rows {
		values, err := rowFromProto(row)
		if err != nil {
			return WriteResult{}, err
		}
		out.Rows = append(out.Rows, values)
	}
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
			Schema: s.client.schemas.claimFor(table),
		})
	})
}

// Upsert adds rows, replacing any whose primary key is taken.
func (s *Session) Upsert(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.Insert(ctx, &pb.InsertRequest{
			Table: table, Rows: rowsToProto(rows), Upsert: true,
			Schema: s.client.schemas.claimFor(table),
		})
	})
}

// Update replaces rows, refusing one whose primary key is not there.
func (s *Session) Update(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.Update(ctx, &pb.UpdateRequest{
			Table: table, Rows: rowsToProto(rows),
			Schema: s.client.schemas.claimFor(table),
		})
	})
}

// Delete removes rows by primary key.
func (s *Session) Delete(ctx context.Context, table string, keys ...[]Value) (WriteResult, error) {
	return s.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return s.client.rpc.Delete(ctx, &pb.DeleteRequest{
			Table: table, PrimaryKeys: rowsToProto(keys),
			Schema: s.client.schemas.claimFor(table),
		})
	})
}

// Get reads one row by primary key.
//
// The second return is whether it was found, which is separate from the error
// so that "no such row" is not an error path: a caller checking existence
// should not have to match on an error type to do it.
func (s *Session) Get(ctx context.Context, table string, key []Value) ([]Value, bool, error) {
	ctx = s.ctx(ctx)
	response, err := s.client.rpc.Get(ctx, &pb.GetRequest{
		Table:      table,
		PrimaryKey: rowToProto(key),
		Freshness:  s.freshness(),
		Schema:     s.client.schemas.claimFor(table),
	})
	if err != nil {
		return nil, false, fromRPC(ctx, err)
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
	stream grpc.ServerStreamingClient[pb.QueryResponse]
	// requestID is the id the call was opened with. A stream can fail
	// on its tenth message as readily as its first, and the id names the
	// same call either way -- the daemon logged one line for this stream,
	// at its head. Kept here because Next() has no context in scope.
	requestID string
	cancel    context.CancelFunc
	session   *Session
	batch     [][]Value
	computed  [][]Value
	at        int
	servedBy  *ServedBy
	warnings  []string
	done      bool
	err       error
}

// Query reads rows.
func (s *Session) Query(ctx context.Context, query Query) (*RowStream, error) {
	ctx, cancel := context.WithCancel(s.ctx(ctx))
	stream, err := s.client.rpc.Query(ctx, &pb.QueryRequest{
		Query:     query.toProto(s.client.schemas.claimFor(query.Table)),
		Freshness: s.freshness(),
	})
	if err != nil {
		cancel()
		return nil, fromRPC(ctx, err)
	}
	return &RowStream{stream: stream, requestID: requestIDOf(ctx), cancel: cancel, session: s}, nil
}

// Next reports whether another row is available, fetching a batch if the
// current one is spent, and false when there are none left or the stream
// failed. Check [RowStream.Err] afterwards.
//
// It does not advance: [RowStream.Row] does, which is what lets
// [RowStream.Computed] be read for the same row first. So `for s.Next() {}`
// with no Row() call in the body never terminates and spins at full CPU —
// this comment said "advances to the next row", and a test written from it
// did exactly that.
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
			r.err = withTrailers(fromRPCWithID(r.requestID, err), r.trailers())
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
		r.computed = r.computed[:0]
		r.at = 0
		for _, row := range message.Rows {
			decoded, err := rowFromProto(row)
			if err != nil {
				r.err = err
				r.done = true
				return false
			}
			extra, err := computedFromProto(row)
			if err != nil {
				r.err = err
				r.done = true
				return false
			}
			r.batch = append(r.batch, decoded)
			r.computed = append(r.computed, extra)
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
//
// Stored columns only. What [Query.Compute] produced is in [RowStream.Computed]
// — beside the row rather than as a tail of it, so an ordinal still means a
// column and a caller that indexes past the end gets nothing rather than
// silently getting a computed value.
//
// It advances the cursor, so [RowStream.Computed] must be read first.
func (r *RowStream) Row() []Value {
	if r.at >= len(r.batch) {
		return nil
	}
	row := r.batch[r.at]
	r.at++
	return row
}

// Computed is what [Query.Compute] produced for the row [RowStream.Row] is
// about to return, in declaration order. Empty when the query computes nothing.
//
// Read it *before* [RowStream.Row], which advances the cursor.
func (r *RowStream) Computed() []Value {
	if r.at >= len(r.computed) {
		return nil
	}
	return r.computed[r.at]
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
	ctx = s.ctx(ctx)
	response, err := s.client.rpc.Begin(ctx, &pb.BeginRequest{})
	if err != nil {
		return nil, fromRPC(ctx, err)
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
	ctx = t.session.ctx(ctx)
	response, err := t.session.client.rpc.Commit(
		ctx, &pb.CommitRequest{Transaction: t.id})
	if err != nil {
		return fromRPC(ctx, err)
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
	ctx = t.session.ctx(ctx)
	_, err := t.session.client.rpc.Rollback(
		ctx, &pb.RollbackRequest{Transaction: t.id})
	return fromRPC(ctx, err)
}

// Insert adds rows inside the transaction.
func (t *Transaction) Insert(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.Insert(ctx, &pb.InsertRequest{
			Transaction: t.id, Table: table, Rows: rowsToProto(rows),
			Schema: t.session.client.schemas.claimFor(table),
		})
	})
}

// Upsert adds or replaces rows inside the transaction.
func (t *Transaction) Upsert(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.Insert(ctx, &pb.InsertRequest{
			Transaction: t.id, Table: table, Rows: rowsToProto(rows), Upsert: true,
			Schema: t.session.client.schemas.claimFor(table),
		})
	})
}

// Update replaces rows inside the transaction.
func (t *Transaction) Update(ctx context.Context, table string, rows ...[]Value) (WriteResult, error) {
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.Update(ctx, &pb.UpdateRequest{
			Transaction: t.id, Table: table, Rows: rowsToProto(rows),
			Schema: t.session.client.schemas.claimFor(table),
		})
	})
}

// Delete removes rows by primary key inside the transaction.
func (t *Transaction) Delete(ctx context.Context, table string, keys ...[]Value) (WriteResult, error) {
	return t.session.write(ctx, func(ctx context.Context) (*pb.WriteResponse, error) {
		return t.session.client.rpc.Delete(ctx, &pb.DeleteRequest{
			Transaction: t.id, Table: table, PrimaryKeys: rowsToProto(keys),
			Schema: t.session.client.schemas.claimFor(table),
		})
	})
}

// Get reads one row inside the transaction, seeing its uncommitted writes.
func (t *Transaction) Get(ctx context.Context, table string, key []Value) ([]Value, bool, error) {
	ctx = t.session.ctx(ctx)
	response, err := t.session.client.rpc.Get(ctx, &pb.GetRequest{
		Transaction: t.id, Table: table, PrimaryKey: rowToProto(key),
		Schema: t.session.client.schemas.claimFor(table),
	})
	if err != nil {
		return nil, false, fromRPC(ctx, err)
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
		Transaction: t.id,
		Query:       query.toProto(t.session.client.schemas.claimFor(query.Table)),
	})
	if err != nil {
		cancel()
		return nil, fromRPC(ctx, err)
	}
	return &RowStream{stream: stream, requestID: requestIDOf(ctx), cancel: cancel, session: t.session}, nil
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
	// Decodes is the columns this plan decodes: the projection plus whatever
	// the residual reads. Often the only visible difference between a grouped
	// read's plan and the plan of the read it groups, since narrowing the
	// projection need not change the access path.
	Decodes  []uint32
	Display  string
	Warnings []string
}

// explanationFrom converts one plan.
//
// Factored out because there were two copies of these ten assignments, and a
// field added to one and not the other is the kind of omission that shows up
// as a zero value rather than as an error -- which is exactly what `Decodes`
// would have done.
func explanationFrom(wire *pb.ExplainResponse) Explanation {
	if wire == nil {
		return Explanation{}
	}
	return Explanation{
		Table:         wire.Table,
		Access:        wire.Access,
		Residual:      wire.Residual,
		Descending:    wire.Descending,
		EstimatedRows: wire.EstimatedRows,
		EstimatedCost: wire.EstimatedCost,
		Sorts:         wire.Sorts,
		IndexOnly:     wire.IndexOnly,
		Decodes:       wire.Decodes,
		Display:       wire.Display,
		Warnings:      wire.Warnings,
	}
}

// Explain asks for a plan without running it.
//
// Requires the `explain` action on every table involved, which a `read` grant
// does not carry: a plan is costed against statistics covering rows the
// caller's policy may hide.
func (s *Session) Explain(ctx context.Context, query Query) (*Explanation, error) {
	ctx = s.ctx(ctx)
	response, err := s.client.rpc.Explain(ctx, &pb.ExplainRequest{
		Query:     query.toProto(s.client.schemas.claimFor(query.Table)),
		Freshness: s.freshness(),
	})
	if err != nil {
		return nil, fromRPC(ctx, err)
	}
	s.observeServedBy(response.ServedBy)
	plan := explanationFrom(response)
	return &plan, nil
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
		return nil, fromRPC(ctx, err)
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
	stream grpc.ServerStreamingClient[pb.JoinResponse]
	// requestID is the id the call was opened with. A stream can fail
	// on its tenth message as readily as its first, and the id names the
	// same call either way -- the daemon logged one line for this stream,
	// at its head. Kept here because Next() has no context in scope.
	requestID string
	cancel    context.CancelFunc
	session   *Session
	batch     [][][]Value
	computed  [][]Value
	// One entry per row, then one per input: what that input's own
	// `JoinInput.Compute` produced. Kept apart from `batch` rather than
	// appended to each input's values, for the reason the wire keeps them
	// apart — a caller indexing past an input's columns would otherwise get a
	// computed value and read it as a column.
	inputComputed [][][]Value
	at            int
	servedBy      *ServedBy
	warnings      []string
	done          bool
	err           error
}

// Join reads joined rows.
func (s *Session) Join(ctx context.Context, join JoinQuery) (*JoinStream, error) {
	ctx, cancel := context.WithCancel(s.ctx(ctx))
	stream, err := s.client.rpc.Join(ctx, &pb.JoinRequest{
		Join: join.toProto(s.client.schemas), Freshness: s.freshness(),
	})
	if err != nil {
		cancel()
		return nil, fromRPC(ctx, err)
	}
	return &JoinStream{stream: stream, requestID: requestIDOf(ctx), cancel: cancel, session: s}, nil
}

// Join reads joined rows inside the transaction.
func (t *Transaction) Join(ctx context.Context, join JoinQuery) (*JoinStream, error) {
	ctx, cancel := context.WithCancel(t.session.ctx(ctx))
	stream, err := t.session.client.rpc.Join(ctx, &pb.JoinRequest{
		Transaction: t.id, Join: join.toProto(t.session.client.schemas),
	})
	if err != nil {
		cancel()
		return nil, fromRPC(ctx, err)
	}
	return &JoinStream{stream: stream, requestID: requestIDOf(ctx), cancel: cancel, session: t.session}, nil
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
			j.err = withTrailers(fromRPCWithID(j.requestID, err), j.stream.Trailer())
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
		j.computed = j.computed[:0]
		j.inputComputed = j.inputComputed[:0]
		j.at = 0
		for _, joined := range message.Rows {
			inputs, extra, perInput, err := joinedRowFromProto(joined)
			if err != nil {
				j.err = err
				j.done = true
				return false
			}
			j.batch = append(j.batch, inputs)
			j.computed = append(j.computed, extra)
			j.inputComputed = append(j.inputComputed, perInput)
		}
	}
	return true
}

// joinedRowFromProto decodes one joined row: each input's values, the join's
// own computed values, and each input's own.
//
// Extracted rather than written twice. [Session.PageJoin] needs the same
// decode, and a second copy is how the two come to disagree about the one part
// that is easy to get wrong — a nil input meaning "unmatched" rather than "a
// row of nulls".
func joinedRowFromProto(joined *pb.JoinedRow) ([][]Value, []Value, [][]Value, error) {
	inputs := make([][]Value, 0, len(joined.Inputs))
	perInput := make([][]Value, 0, len(joined.Inputs))
	for _, input := range joined.Inputs {
		// A nil `Row` is an unmatched side of an outer join, and stays nil
		// here so a caller can tell it from a row of nulls. Its computed
		// values are nil for the same reason: the input produced no row, so it
		// computed nothing for this one.
		if input.Row == nil {
			inputs = append(inputs, nil)
			perInput = append(perInput, nil)
			continue
		}
		row, err := rowFromProto(input.Row)
		if err != nil {
			return nil, nil, nil, err
		}
		own, err := computedFromProto(input.Row)
		if err != nil {
			return nil, nil, nil, err
		}
		inputs = append(inputs, row)
		perInput = append(perInput, own)
	}
	extra, err := valuesFromProto(joined.Computed, "the join's computed value")
	if err != nil {
		return nil, nil, nil, err
	}
	return inputs, extra, perInput, nil
}

// Computed is what [JoinQuery.Compute] produced for the row [JoinStream.Row]
// is about to return, in declaration order. Empty when the join computes
// nothing.
//
// Beside the inputs rather than inside one of them, because a value that may
// read every input belongs to none of them. An input's *own* computed values
// — what [JoinInput.Compute] declared — are not here; they are that input's,
// and [JoinStream.InputComputed] returns them. See [ComputedAt], which
// explains why an input's computed value cannot be *named* across a join
// either.
//
// Read it *before* [JoinStream.Row], which advances the cursor.
func (j *JoinStream) Computed() []Value {
	if j.at >= len(j.computed) {
		return nil
	}
	return j.computed[j.at]
}

// InputComputed is what input `input`'s own [JoinInput.Compute] produced for
// the row [JoinStream.Row] is about to return, in declaration order.
//
// The index is the handle [JoinBuilder.Add] returned, which is what every
// other join accessor takes, so a caller never converts one.
//
// Nil for an input that declared no computed values, for an input index this
// join does not have, and for the unmatched side of an outer join — which
// produced no row and so computed nothing. Nil rather than an error in all
// three cases, matching [JoinStream.Row]'s treatment of an unmatched input:
// this is a cursor, and it has nowhere to put one.
//
// This was missing while [JoinStream.Computed] existed, so a Go caller could
// declare an input-level computed value, have the server evaluate it, and have
// no way to read it back — the value arrived on the wire in that input's
// `Row.computed` and was dropped here.
//
// Read it *before* [JoinStream.Row], which advances the cursor.
func (j *JoinStream) InputComputed(input uint32) []Value {
	if j.at >= len(j.inputComputed) {
		return nil
	}
	row := j.inputComputed[j.at]
	if input >= uint32(len(row)) {
		return nil
	}
	return row[input]
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
	stream grpc.ServerStreamingClient[pb.AggregateResponse]
	// requestID is the id the call was opened with. A stream can fail
	// on its tenth message as readily as its first, and the id names the
	// same call either way -- the daemon logged one line for this stream,
	// at its head. Kept here because Next() has no context in scope.
	requestID string
	cancel    context.CancelFunc
	session   *Session
	batch     []Group
	at        int
	servedBy  *ServedBy
	warnings  []string
	done      bool
	err       error
}

// Aggregate groups one table.
func (s *Session) Aggregate(
	ctx context.Context,
	over Query,
	grouping Grouping,
) (*GroupStream, error) {
	wire := &pb.AggregateQuery{Input: over.toProto(s.client.schemas.claimFor(over.Table))}
	grouping.apply(wire)
	return s.aggregate(ctx, wire, "")
}

// AggregateJoin groups a join, or a chain of any length.
//
// It was two inputs exactly when the kernel grouped only a two-table join and
// the server refused a third; it groups a chain now, and the refusal went with
// it. Nothing counts inputs here, where the count could drift from the
// kernel's.
func (s *Session) AggregateJoin(
	ctx context.Context,
	over JoinQuery,
	grouping Grouping,
) (*GroupStream, error) {
	wire := &pb.AggregateQuery{Join: over.toProto(s.client.schemas)}
	grouping.apply(wire)
	return s.aggregate(ctx, wire, "")
}

// Aggregate groups one table inside the transaction.
func (t *Transaction) Aggregate(
	ctx context.Context,
	over Query,
	grouping Grouping,
) (*GroupStream, error) {
	wire := &pb.AggregateQuery{
		Input: over.toProto(t.session.client.schemas.claimFor(over.Table)),
	}
	grouping.apply(wire)
	return t.session.aggregate(ctx, wire, t.id)
}

// AggregateJoin groups a join inside the transaction.
func (t *Transaction) AggregateJoin(
	ctx context.Context,
	over JoinQuery,
	grouping Grouping,
) (*GroupStream, error) {
	wire := &pb.AggregateQuery{Join: over.toProto(t.session.client.schemas)}
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
		return nil, fromRPC(ctx, err)
	}
	return &GroupStream{stream: stream, requestID: requestIDOf(ctx), cancel: cancel, session: s}, nil
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
			g.err = withTrailers(fromRPCWithID(g.requestID, err), g.stream.Trailer())
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
	ctx = s.ctx(ctx)
	response, err := s.client.rpc.ExplainJoin(ctx, &pb.ExplainJoinRequest{
		Join: join.toProto(s.client.schemas), Freshness: s.freshness(),
	})
	if err != nil {
		return nil, fromRPC(ctx, err)
	}
	s.observeServedBy(response.ServedBy)
	return joinExplanationFrom(response), nil
}

// joinExplanationFrom converts a join or chain plan.
func joinExplanationFrom(response *pb.JoinExplainResponse) *JoinExplanation {
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
		plan.Plan = explanationFrom(input.Plan)
		out.Inputs = append(out.Inputs, plan)
	}
	return out
}

// AggregateExplanation is the plan a grouped read would run under.
//
// Exactly one of Input and Join is set, matching the request: Input for an
// aggregate over one table, Join for one over a join or a chain.
type AggregateExplanation struct {
	Input    *Explanation
	Join     *JoinExplanation
	Display  string
	Warnings []string
}

// ExplainAggregate asks for a grouped read's plan without running it.
//
// Not Explain or ExplainJoin on the underlying read: grouping narrows each
// input's projection to the group keys and the aggregates' columns, which is
// what lets an index answer a COUNT(*) without touching a row. Comparing
// Decodes between the two is how to see it where the access path is unchanged.
//
// Needs the `explain` action on every table involved, as every plan does.
func (s *Session) ExplainAggregate(
	ctx context.Context,
	over Query,
	grouping Grouping,
) (*AggregateExplanation, error) {
	wire := &pb.AggregateQuery{Input: over.toProto(s.client.schemas.claimFor(over.Table))}
	grouping.apply(wire)
	return s.explainAggregate(ctx, wire, "")
}

// ExplainAggregateJoin asks for a grouped join's or grouped chain's plan.
func (s *Session) ExplainAggregateJoin(
	ctx context.Context,
	over JoinQuery,
	grouping Grouping,
) (*AggregateExplanation, error) {
	wire := &pb.AggregateQuery{Join: over.toProto(s.client.schemas)}
	grouping.apply(wire)
	return s.explainAggregate(ctx, wire, "")
}

// ExplainAggregate asks for a grouped read's plan inside the transaction.
func (t *Transaction) ExplainAggregate(
	ctx context.Context,
	over Query,
	grouping Grouping,
) (*AggregateExplanation, error) {
	wire := &pb.AggregateQuery{
		Input: over.toProto(t.session.client.schemas.claimFor(over.Table)),
	}
	grouping.apply(wire)
	return t.session.explainAggregate(ctx, wire, t.id)
}

// ExplainAggregateJoin asks for a grouped join's plan inside the transaction.
func (t *Transaction) ExplainAggregateJoin(
	ctx context.Context,
	over JoinQuery,
	grouping Grouping,
) (*AggregateExplanation, error) {
	wire := &pb.AggregateQuery{Join: over.toProto(t.session.client.schemas)}
	grouping.apply(wire)
	return t.session.explainAggregate(ctx, wire, t.id)
}

func (s *Session) explainAggregate(
	ctx context.Context,
	wire *pb.AggregateQuery,
	transaction string,
) (*AggregateExplanation, error) {
	request := &pb.ExplainAggregateRequest{Aggregate: wire, Transaction: transaction}
	if transaction == "" {
		request.Freshness = s.freshness()
	}
	ctx = s.ctx(ctx)
	response, err := s.client.rpc.ExplainAggregate(ctx, request)
	if err != nil {
		return nil, fromRPC(ctx, err)
	}
	s.observeServedBy(response.ServedBy)
	out := &AggregateExplanation{Display: response.Display, Warnings: response.Warnings}
	if response.Input != nil {
		plan := explanationFrom(response.Input)
		out.Input = &plan
	}
	if response.Join != nil {
		out.Join = joinExplanationFrom(response.Join)
	}
	return out, nil
}
