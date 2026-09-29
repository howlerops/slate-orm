package slate_test

import (
	"strings"
	"testing"

	"google.golang.org/protobuf/proto"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// A batch is one round trip and twenty inserts are twenty, counted at the
// client's own interceptor.
//
// # Why this file exists when `related_test.go` already counts requests
//
// `TestRelatedIsOneRequestHoweverManyParents` has counted requests through
// `dialRecording` since relations landed, and it is the same instrument. What
// it counts is `/Related`. The *batch* claim — the one two ledger entries say
// is inherited from a Rust wire test rather than shown from a client — had no
// counterpart here, and neither did paging.
//
// Found by writing the Python equivalent first and then checking whether the
// sentence "none of the three clients has a benchmark" was still true. It was
// half true, which is the more expensive kind: Go had the mechanism and one
// use of it, so a reader who looked would have concluded the claim was covered.
//
// # Why counting and not timing
//
// A count has no spread. The inherited 9.4x was a ratio of durations on a
// loopback socket, which is mostly scheduling — and this session already
// published one timing claim that reversed between two machines. Twenty writes
// are twenty requests or they are one, on any machine.
//
// What a count cannot see is the per-request work the clients do that the Rust
// test did not: building a schema claim, encoding each value. Twenty rows cost
// twenty encodings inside a batch too. That half of the caveat stands and is
// recorded as the residual rather than implied away.

// batchRoundTripRows is far enough from one that a failure message is worth
// reading: `20 != 1` says what happened, where `2 != 1` could be anything.
const batchRoundTripRows = 20

func TestABatchIsOneRequestAndSinglesAreMany(t *testing.T) {
	server := start(t, batchTables)

	var inserts, batches int
	client := dialRecording(t, server, func(method string, _ proto.Message) {
		switch {
		case strings.HasSuffix(method, "/Insert"):
			inserts++
		case strings.HasSuffix(method, "/Batch"):
			batches++
		}
	})
	session := client.Session()
	ctx := testContext(t)

	// The control first. Without it, the batch case shows only that a batch
	// works — a client that sent one request per row would return the same
	// rows and pass everything about the answer.
	for id := uint64(1); id <= batchRoundTripRows; id++ {
		if _, err := session.Insert(ctx, "notes", note(id)); err != nil {
			t.Fatalf("insert %d: %v", id, err)
		}
	}
	if inserts != batchRoundTripRows {
		t.Errorf("%d rows one at a time took %d Insert calls, want %d",
			batchRoundTripRows, inserts, batchRoundTripRows)
	}

	b := slate.NewBatch(slate.Independent)
	for id := uint64(batchRoundTripRows + 1); id <= batchRoundTripRows*2; id++ {
		b = b.Insert("notes", note(id))
	}
	before := inserts
	if _, err := session.Batch(ctx, b); err != nil {
		t.Fatalf("batch: %v", err)
	}
	if batches != 1 {
		t.Errorf("%d rows in a batch took %d Batch calls, want 1", batchRoundTripRows, batches)
	}
	if inserts != before {
		t.Errorf("a batch also made %d Insert calls, want none", inserts-before)
	}
}

// batchRoundTripPages and batchRoundTripPageSize are separate constants from
// batchRoundTripRows rather than a division of it, so that resizing the write
// cases cannot silently resize the paging one.
const (
	batchRoundTripPages    = 4
	batchRoundTripPageSize = 5
)

// TestPagingByCursorIsOneRequestPerPage counts what a keyset page costs the
// caller, which is the other half of the same caveat.
//
// The README's "page 99 read 495 key-value pairs by offset and 5 by cursor" is
// a kernel measurement of what a page reads from the *store*. This is what a
// page costs on the wire, which is the number someone sizing a page against a
// network needs. Python counted it first; Go had the interceptor and never
// pointed it at paging.
func TestPagingByCursorIsOneRequestPerPage(t *testing.T) {
	server := start(t, batchTables)

	var queries int
	client := dialRecording(t, server, func(method string, _ proto.Message) {
		if strings.HasSuffix(method, "/Query") {
			queries++
		}
	})
	session := client.Session()
	ctx := testContext(t)

	rows := uint64(batchRoundTripPages * batchRoundTripPageSize)
	b := slate.NewBatch(slate.Independent)
	for id := uint64(1); id <= rows; id++ {
		b = b.Insert("notes", note(id))
	}
	if _, err := session.Batch(ctx, b); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	queries = 0

	// The node is this test's own and the memory backend seeds nothing, so
	// those rows are every row in `notes` and no filter is needed to isolate
	// them.
	var cursor []slate.Value
	seen := 0
	for p := 0; p < batchRoundTripPages; p++ {
		limit := uint64(batchRoundTripPageSize)
		page, err := session.Page(ctx, slate.Query{
			Table: "notes", Limit: &limit, After: cursor,
		})
		if err != nil {
			t.Fatalf("page %d: %v", p, err)
		}
		seen += len(page.Rows)
		cursor = page.Cursor
	}

	// Both sides of each comparison are constants, and that is the point: the
	// Python version of this first compared against a counter its own loop
	// incremented, so it held for any number of pages including one, and a
	// mutation shrinking the loop survived.
	//
	// `/Query`, not a `/Page` RPC: a keyset page is an ordinary query carrying
	// a cursor and a limit, which is the wire shape the interceptor sees.
	if queries != batchRoundTripPages {
		t.Errorf("%d pages took %d Query calls, want %d",
			batchRoundTripPages, queries, batchRoundTripPages)
	}
	if want := batchRoundTripPages * batchRoundTripPageSize; seen != want {
		t.Errorf("%d full pages of %d returned %d rows, want %d",
			batchRoundTripPages, batchRoundTripPageSize, seen, want)
	}
}

// TestBatchingSavesRoundTripsAndCostsBytes weighs what the counts could not.
//
// `ledger/2026-09-29-bytes-do-not-need-a-network.md` found in Python that a
// batch costs *more* bytes than the singles it replaces — four per statement
// for the tag and length prefix nesting adds, two for the envelope — and left
// the other two clients unweighed. A second measurement of the same shape is
// the only thing that can say whether two clients build the same request:
// three clients sending the same *number* of requests can still send
// different ones, and every answer-comparing case passes either way.
//
// Weighing a read needed more than weighing a write. grpc-go hands a stream
// interceptor the call and not the payload, so `dialRecording` reported a nil
// for every `Query` — enough to count one, never enough to weigh one. The
// `sendingStream` wrapper beside it is what closed that, and these two tests
// are why it exists.
func TestBatchingSavesRoundTripsAndCostsBytes(t *testing.T) {
	server := start(t, batchTables)

	var bytes int
	client := dialRecording(t, server, func(_ string, request proto.Message) {
		if request != nil {
			bytes += proto.Size(request)
		}
	})
	session := client.Session()
	ctx := testContext(t)

	for id := uint64(1); id <= batchRoundTripRows; id++ {
		if _, err := session.Insert(ctx, "notes", note(id)); err != nil {
			t.Fatalf("insert %d: %v", id, err)
		}
	}
	singly := bytes

	bytes = 0
	b := slate.NewBatch(slate.Independent)
	for id := uint64(batchRoundTripRows + 1); id <= batchRoundTripRows*2; id++ {
		b = b.Insert("notes", note(id))
	}
	if _, err := session.Batch(ctx, b); err != nil {
		t.Fatalf("batch: %v", err)
	}
	batched := bytes

	// The formula, not the literal. Measured here: 502 batched against 420
	// singly, a delta of 82 — and the Python client's own rows give 1382
	// against 1300, the *same* delta from totals three times the size. That
	// agreement is the point of measuring it twice: the payloads differ
	// because the fixtures differ, and the framing does not.
	//
	// The second half of these rows is a different key range from the first,
	// so a delta that depended on the payload would not survive this
	// comparison either.
	want := 4*batchRoundTripRows + 2
	if batched-singly != want {
		t.Errorf("a batch of %d weighed %d against %d sent singly, a delta of %d; "+
			"want %d — four bytes of framing per statement and two for the envelope",
			batchRoundTripRows, batched, singly, batched-singly, want)
	}
}

// TestACursorPageCostsMoreBytesThanAnOffsetPage is the paging half.
//
// The same correction to the same phrase: what keyset paging saves is *store*
// reads — the README's 495 key-value pairs by offset against 5 by cursor — and
// that saving is on the server. On the request a cursor carries the last row's
// key where an offset carries a small integer, so the cursor walk is heavier.
//
// Measured: 80 bytes against 66 over four pages, a delta of 14. Python's four
// pages read 215 against 198, a delta of 17. Both say the cursor costs more;
// the deltas differ because the keys do, which is why the assertion is a range
// and the numbers are narration.
func TestACursorPageCostsMoreBytesThanAnOffsetPage(t *testing.T) {
	server := start(t, batchTables)

	var bytes int
	client := dialRecording(t, server, func(_ string, request proto.Message) {
		if request != nil {
			bytes += proto.Size(request)
		}
	})
	session := client.Session()
	ctx := testContext(t)

	rows := uint64(batchRoundTripPages * batchRoundTripPageSize)
	b := slate.NewBatch(slate.Independent)
	for id := uint64(1); id <= rows; id++ {
		b = b.Insert("notes", note(id))
	}
	if _, err := session.Batch(ctx, b); err != nil {
		t.Fatalf("seeding: %v", err)
	}

	bytes = 0
	var cursor []slate.Value
	for p := 0; p < batchRoundTripPages; p++ {
		limit := uint64(batchRoundTripPageSize)
		page, err := session.Page(ctx, slate.Query{Table: "notes", Limit: &limit, After: cursor})
		if err != nil {
			t.Fatalf("cursor page %d: %v", p, err)
		}
		cursor = page.Cursor
		if cursor == nil {
			t.Fatalf("the fixture ran out of rows at page %d", p)
		}
	}
	byCursor := bytes

	bytes = 0
	for p := 0; p < batchRoundTripPages; p++ {
		limit := uint64(batchRoundTripPageSize)
		offset := uint64(p * batchRoundTripPageSize)
		read, err := session.Query(ctx, slate.Query{Table: "notes", Limit: &limit, Offset: offset})
		if err != nil {
			t.Fatalf("offset page %d: %v", p, err)
		}
		// Drained, because the request is sent when the stream starts and an
		// unstarted stream weighs nothing — and because leaving it open is
		// what the TypeScript twin did before node's runner objected, four
		// times, about a test whose assertions had all passed.
		if _, err := read.Collect(); err != nil {
			t.Fatalf("draining offset page %d: %v", p, err)
		}
	}
	byOffset := bytes

	// A range, not a literal: the difference is a key whose varint widens with
	// the value. What is pinned is that a cursor costs *more* and is not free,
	// which a cursor dropped from the request would break by making the two
	// equal.
	if delta := byCursor - byOffset; delta < 8 || delta > 48 {
		t.Errorf("four cursor pages weighed %d against %d by offset, a delta of %d; "+
			"want a cursor to cost measurably more and not absurdly more",
			byCursor, byOffset, delta)
	}
}
