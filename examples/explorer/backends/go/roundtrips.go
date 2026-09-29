package main

import (
	"context"
	"encoding/json"
	"fmt"
	"sort"
	"sync"

	"google.golang.org/grpc"

	"github.com/howlerops/slate-orm/clients/go/slate"

	"github.com/howlerops/slate-orm/examples/explorer/backends/go/schema"
)

// roundTripFirst is the id range this handler owns, clear of every other one.
const roundTripFirst = 9400

// roundTripRows is how many rows the write workloads write.
//
// Four rather than two, so `4 != 1` says what happened where `2 != 1` could be
// an off-by-one anywhere. Not twenty, as the client suites use: this runs
// against a shared demo database on every conformance run, and what is being
// compared is the shape of the count, which four shows.
const roundTripRows = 4

// Three keyset pages of three, over the eleven books the fixture seeds.
//
// Nine of eleven, so the third page is full and the walk never meets the end.
// A short page would stop the loop early in a way that depends on how many
// rows the *other* endpoints happen to have left behind, and the three
// adapters run at different points in that sequence.
const (
	roundTripPages    = 3
	roundTripPageSize = 3
)

// roundTripParents are three seeded books that have sales.
var roundTripParents = []uint64{10, 11, 12}

// counting counts the calls a client makes, per RPC.
//
// Both interceptors, because a read is server-streaming and a write is unary.
// Registering only the unary one counts writes and reports zero for every
// query, which reads as "queries are free" rather than as a hole in the
// instrument — the bug `dialRecording` in the Go client's own suite shipped
// with for a day, recorded in
// `ledger/2026-09-28-the-third-client-counts-and-the-go-instrument-was-half-blind.md`.
type counting struct {
	mu    sync.Mutex
	calls map[string]int
}

func newCounting() *counting { return &counting{calls: map[string]int{}} }

func (c *counting) count(method string) {
	// `/slate.v1.Records/Insert` -> `Insert`. The RPC names are not the
	// client's method names: a single insert is `Insert` and a keyset page is
	// `Query`.
	name := method
	for i := len(method) - 1; i >= 0; i-- {
		if method[i] == '/' {
			name = method[i+1:]
			break
		}
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.calls[name]++
}

func (c *counting) clear() {
	c.mu.Lock()
	defer c.mu.Unlock()
	clear(c.calls)
}

func (c *counting) snapshot() map[string]int {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make(map[string]int, len(c.calls))
	for k, v := range c.calls {
		out[k] = v
	}
	return out
}

func (c *counting) unary(
	ctx context.Context, method string, req, reply any,
	conn *grpc.ClientConn, invoker grpc.UnaryInvoker, opts ...grpc.CallOption,
) error {
	c.count(method)
	return invoker(ctx, method, req, reply, conn, opts...)
}

func (c *counting) stream(
	ctx context.Context, desc *grpc.StreamDesc, conn *grpc.ClientConn,
	method string, streamer grpc.Streamer, opts ...grpc.CallOption,
) (grpc.ClientStream, error) {
	c.count(method)
	return streamer(ctx, desc, conn, method, opts...)
}

// rpcCall is one line of the answer. Sorted by name in the answer, and zeroes
// left out: a map's iteration order is the one thing in this contract three
// languages would spell three ways for free — and Go's is deliberately random.
type rpcCall struct {
	RPC   string `json:"rpc"`
	Count int    `json:"count"`
}

// roundTrips runs a fixed workload and reports how many requests the client
// sent.
//
// The one thing three SDKs can differ about that no comparison of *answers*
// can see. A client that looped where the other two batched returns identical
// rows and costs N times as much, so `/api/batch` and `/api/related` agreeing
// says nothing about it.
//
// Each client's own suite already counts its own round trips, against its own
// expectation. Three independent assertions are weaker than one comparison:
// they cannot catch two clients that are wrong the same way, which is the
// failure this whole runner exists for.
//
// The persona is ignored on purpose — this always runs as `app`, on the
// adapter's own counting connection. Row-level security changes which rows a
// read returns and not how many requests fetching them takes, so letting the
// header through would offer a knob that cannot move the answer.
func (s *server) roundTrips(ctx context.Context, _ *slate.Session, body json.RawMessage) (any, error) {
	var spec struct {
		Workload string `json:"workload"`
	}
	if err := json.Unmarshal(body, &spec); err != nil {
		return nil, fmt.Errorf("decoding the workload: %w", err)
	}

	session := s.counted.Session()
	scrub := func() error {
		_, err := session.DeleteWhere(ctx, slate.DeleteWhere{
			Table:  "books",
			Filter: slate.Filter(slate.Ge(0, slate.Uint(roundTripFirst))),
		})
		return err
	}

	// One measurement at a time. The counter is shared state and `net/http`
	// serves concurrent requests, so two overlapping calls would each read the
	// other's calls. The conformance runner is sequential, so this never
	// contends; it is here because a count that is silently wrong under
	// concurrency is worse than one that waits.
	s.countingMu.Lock()
	defer s.countingMu.Unlock()

	// Before, not only after: a run that died partway through leaves rows
	// behind, and `singles` would then fail on an already-exists rather than
	// counting anything.
	if err := scrub(); err != nil {
		return nil, err
	}
	s.counter.clear()
	// Outside the count, and deferred so a workload that failed partway still
	// leaves the range empty for the next adapter the runner asks.
	defer func() {
		s.counter.clear()
		_ = scrub()
	}()

	if err := s.runRoundTrip(ctx, session, spec.Workload); err != nil {
		return nil, err
	}

	calls := s.counter.snapshot()
	names := make([]string, 0, len(calls))
	for name := range calls {
		names = append(names, name)
	}
	sort.Strings(names)
	out := make([]rpcCall, 0, len(names))
	for _, name := range names {
		out = append(out, rpcCall{RPC: name, Count: calls[name]})
	}
	return map[string]any{"calls": out}, nil
}

func (s *server) runRoundTrip(ctx context.Context, session *slate.Session, workload string) error {
	switch workload {
	case "singles":
		// The control. Without it the batch case shows only that a batch
		// works, and a client sending one request per row returns exactly the
		// same rows.
		for n := range roundTripRows {
			title := fmt.Sprintf("Round Trip %d", n)
			if _, err := session.Insert(ctx, "books", book(roundTripFirst+uint64(n), title)); err != nil {
				return err
			}
		}
		return nil
	case "batch":
		b := slate.NewBatch(slate.Independent)
		for n := range roundTripRows {
			b.Insert("books", book(roundTripFirst+uint64(n), fmt.Sprintf("Round Trip %d", n)))
		}
		_, err := session.Batch(ctx, b)
		return err
	case "paging":
		var cursor []slate.Value
		for range roundTripPages {
			page, err := session.Page(ctx, slate.Query{
				Table: "books",
				Limit: slate.Limit(roundTripPageSize),
				After: cursor,
			})
			if err != nil {
				return err
			}
			cursor = page.Cursor
		}
		return nil
	case "related":
		key := schema.SalesForeignKeys["sale_book"]
		keys := make([][]slate.Value, 0, len(roundTripParents))
		for _, id := range roundTripParents {
			keys = append(keys, []slate.Value{slate.Uint(id)})
		}
		_, err := session.Related(ctx, key.Answers(slate.Children), key.Children(), keys...)
		return err
	default:
		return fmt.Errorf("unknown workload %q", workload)
	}
}
