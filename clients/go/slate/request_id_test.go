package slate_test

import (
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// idOf is the request id on err, or a failed test.
func idOf(t *testing.T, err error) string {
	t.Helper()
	if err == nil {
		t.Fatal("the call succeeded; this test needs it to fail")
	}
	var e *slate.Error
	if !errors.As(err, &e) {
		t.Fatalf("not a *slate.Error: %v", err)
	}
	return e.RequestID
}

// hex32 asserts the shape the client sends and the server's filter admits.
//
// The server keeps only [A-Za-z0-9._:-] and cuts at 64, so a client that
// started sending something punctuated or longer would be *altered* on the way
// into the log rather than refused — and the correlation would break quietly.
func hex32(t *testing.T, id string, what string) {
	t.Helper()
	if len(id) != 32 {
		t.Fatalf("%s: RequestID = %q, want 32 hex characters", what, id)
	}
	if strings.Trim(id, "0123456789abcdef") != "" {
		t.Fatalf("%s: RequestID = %q is not hex", what, id)
	}
}

// TestEveryCallKindNamesItsRequestID is the guard the design needs.
//
// The id travels in the context, so a call site that passes `s.ctx(ctx)` to
// the RPC but the *outer* ctx to fromRPC sends an id and reports none. Nothing
// in the compiler catches that: both are valid contexts. Every RPC family gets
// a row here so a missed site is a failing test rather than a correlation that
// silently stopped working for one method.
func TestEveryCallKindNamesItsRequestID(t *testing.T) {
	s := start(t, "")
	client := s.client(t)
	session := client.Session()
	ctx := testContext(t)

	// Each of these fails for its own reason -- a table nobody declared --
	// because what is being tested is the id on the error, and any error will
	// do so long as it came from the server rather than from this client.
	const absent = "no_such_table"

	_, _, err := session.Get(ctx, absent, []slate.Value{slate.Uint(1)})
	hex32(t, idOf(t, err), "Get")

	_, err = session.Insert(ctx, absent, []slate.Value{slate.Uint(1)})
	hex32(t, idOf(t, err), "Insert")

	// The three streaming calls return before they fail: a server stream is
	// opened without a round trip and the refusal arrives on the first
	// message. So these exercise the *other* path -- fromRPCWithID, reading
	// the id the stream kept -- which is the one a scan that dies on its tenth
	// message also takes.
	rows, err := session.Query(ctx, slate.Query{Table: absent})
	if err != nil {
		hex32(t, idOf(t, err), "Query")
	} else {
		for rows.Next() {
		}
		hex32(t, idOf(t, rows.Err()), "Query (stream)")
	}

	_, err = session.Explain(ctx, slate.Query{Table: absent})
	hex32(t, idOf(t, err), "Explain")

	groups, err := session.Aggregate(ctx, slate.Query{Table: absent}, slate.Grouping{
		Aggregates: []slate.Aggregate{slate.Count()},
	})
	if err != nil {
		hex32(t, idOf(t, err), "Aggregate")
	} else {
		for groups.Next() {
		}
		hex32(t, idOf(t, groups.Err()), "Aggregate (stream)")
	}

	join := slate.NewJoin()
	at := join.Add(slate.JoinInput{Table: absent})
	join.Add(slate.JoinInput{
		Table: "docs",
		On:    []slate.On{{Earlier: slate.At(at, 0), Own: 0}},
	})
	joined, err := session.Join(ctx, join.Query())
	if err != nil {
		hex32(t, idOf(t, err), "Join")
	} else {
		for joined.Next() {
		}
		hex32(t, idOf(t, joined.Err()), "Join (stream)")
	}

	_, err = session.Page(ctx, slate.Query{Table: absent, Limit: slate.Limit(10)})
	hex32(t, idOf(t, err), "Page")

	_, err = session.Related(ctx, absent,
		slate.Relation{On: absent, Way: slate.Children},
		[]slate.Value{slate.Uint(1)})
	hex32(t, idOf(t, err), "Related")

	b := slate.NewBatch(slate.AllOrNothing)
	b.Insert(absent, []slate.Value{slate.Uint(1)})
	_, err = session.Batch(ctx, b)
	hex32(t, idOf(t, err), "Batch")

	// The transaction path has its own three call sites, and they were the
	// ones that still passed `t.session.ctx(ctx)` inline after the rest were
	// converted -- so they are the ones most worth a row here.
	tx, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("beginning a transaction: %v", err)
	}
	_, _, err = tx.Get(ctx, absent, []slate.Value{slate.Uint(1)})
	hex32(t, idOf(t, err), "Transaction.Get")
	_ = tx.Rollback(ctx)
}

func TestTwoCallsGetTwoIDs(t *testing.T) {
	// Per call, not per session or per connection: an id shared by every call
	// a session made names every line it wrote, which is what a caller already
	// has.
	s := start(t, "")
	session := s.client(t).Session()
	ctx := testContext(t)

	_, _, first := session.Get(ctx, "no_such_table", []slate.Value{slate.Uint(1)})
	_, _, second := session.Get(ctx, "no_such_table", []slate.Value{slate.Uint(1)})
	if idOf(t, first) == idOf(t, second) {
		t.Fatalf("both calls reported %q", idOf(t, first))
	}
}

func TestTheKeyMatchesTheOneTheServerReads(t *testing.T) {
	// A mismatch here is silent: the server ignores metadata it does not know,
	// so a misspelled key means every call carries an id nobody ever sees and
	// every log line says `id=-`. Neither side's tests can catch that alone,
	// because each is internally consistent.
	root, err := repoRoot()
	if err != nil {
		t.Fatalf("finding the repository: %v", err)
	}
	source, err := os.ReadFile(filepath.Join(root, "crates", "slate-server", "src", "auth.rs"))
	if err != nil {
		// Read, not skipped. A skip would be green on a wrong path, and the
		// wrong path is the likelier bug: this suite already needs the repo
		// (the harness builds the daemon from it), so the file not being there
		// means this test is looking in the wrong place.
		t.Fatalf("reading the server's key: %v", err)
	}
	found := regexp.MustCompile(`pub const REQUEST_ID_KEY: &str = "([^"]+)";`).
		FindSubmatch(source)
	if found == nil {
		t.Fatal("no REQUEST_ID_KEY in crates/slate-server/src/auth.rs")
	}
	declared := string(found[1])
	if slate.RequestIDKey != declared {
		t.Fatalf("this client sends %q, the server reads %q", slate.RequestIDKey, declared)
	}
}

// TestTheHeaderIsActuallySent is the mutation this file was missing.
//
// Every other test here reads the id off an *error*, and the id on an error is
// generated client-side: deleting the line that puts it in the outgoing
// metadata left all of them passing. A client that mints an id, reports it on
// every failure and never sends it would look perfect from inside and be
// useless, because the server's log would say `id=-` for every call.
//
// This reads the outgoing metadata that `ctx` builds, which is the value that
// actually goes on the wire.
func TestTheHeaderIsActuallySent(t *testing.T) {
	s := start(t, "")
	session := s.client(t).Session()

	out := session.OutgoingForTest(testContext(t))
	sent := out.Get(slate.RequestIDKey)
	if len(sent) != 1 {
		t.Fatalf("%s appears %d times in the outgoing metadata, want 1", slate.RequestIDKey, len(sent))
	}
	hex32(t, sent[0], "the header")

	// And beside the identity, not instead of it.
	if got := out.Get("slate-principal"); len(got) != 1 || got[0] != "u64:1" {
		t.Fatalf("slate-principal = %v, want [u64:1]", got)
	}
}
