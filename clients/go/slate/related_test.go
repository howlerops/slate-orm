package slate_test

import (
	"context"
	"strings"
	"testing"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// A parent and a child with a real foreign key.
//
// The join fixture's `authors`/`books` deliberately has no constraint between
// them — several tests there write a book with an author that does not exist,
// including one called `orphan` — so a relationship cannot be named through it.
// A relationship *is* a foreign key here, so this fixture declares one.
const relatedTables = `
[[tables]]
name = "libraries"
id = 20
columns = [
  { name = "id",   type = "u64" },
  { name = "name", type = "str" },
]
primary_key = ["id"]

[[tables]]
name = "shelves"
id = 21
columns = [
  { name = "id",         type = "u64" },
  { name = "library_id", type = "u64" },
  { name = "label",      type = "str" },
]
primary_key = ["id"]

[[tables.foreign_keys]]
name = "shelf_library"
parent = "libraries"
columns = ["library_id"]

[[security.grants]]
role = "app"
tables = ["libraries", "shelves"]
actions = ["everything"]
`

// shelved starts a node with those tables and seeds three libraries.
//
// Library 1 has two shelves, library 2 has one, library 3 has none — so a
// parent with nothing related is exercised, and the per-parent counts are not
// uniform, which is what makes the grouping assertions mean something.
func shelved(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, relatedTables).client(t).Session()
	ctx := testContext(t)
	for _, library := range [][]slate.Value{
		{slate.Uint(1), slate.String("main")},
		{slate.Uint(2), slate.String("annexe")},
		{slate.Uint(3), slate.String("empty")},
	} {
		if _, err := session.Insert(ctx, "libraries", library); err != nil {
			t.Fatalf("seeding libraries: %v", err)
		}
	}
	for _, shelf := range [][]slate.Value{
		{slate.Uint(100), slate.Uint(1), slate.String("history")},
		{slate.Uint(101), slate.Uint(1), slate.String("poetry")},
		{slate.Uint(102), slate.Uint(2), slate.String("maps")},
	} {
		if _, err := session.Insert(ctx, "shelves", shelf); err != nil {
			t.Fatalf("seeding shelves: %v", err)
		}
	}
	return session
}

func labelsOf(t *testing.T, groups [][][]slate.Value, at int) []string {
	t.Helper()
	out := make([]string, 0, len(groups[at]))
	for _, row := range groups[at] {
		text, ok := row[2].(slate.String)
		if !ok {
			t.Fatalf("label is not a string: %v", row[2])
		}
		out = append(out, string(text))
	}
	return out
}

func TestRelatedGroupsChildrenPerParent(t *testing.T) {
	session := shelved(t)
	got, err := session.Related(
		testContext(t),
		"shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		[]slate.Value{slate.Uint(1)},
		[]slate.Value{slate.Uint(2)},
		[]slate.Value{slate.Uint(3)},
	)
	if err != nil {
		t.Fatalf("related: %v", err)
	}
	if len(got) != 3 {
		t.Fatalf("want three groups, got %d", len(got))
	}
	if labels := labelsOf(t, got, 0); len(labels) != 2 {
		t.Errorf("library 1: want two shelves, got %v", labels)
	}
	if labels := labelsOf(t, got, 1); len(labels) != 1 || labels[0] != "maps" {
		t.Errorf("library 2: want [maps], got %v", labels)
	}
	// Empty, not missing: the caller indexes this by its own loop counter.
	if len(got[2]) != 0 {
		t.Errorf("library 3 has no shelves, got %v", got[2])
	}
}

func TestRelatedReadsParentsToo(t *testing.T) {
	session := shelved(t)
	got, err := session.Related(
		testContext(t),
		"libraries",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Parents},
		[]slate.Value{slate.Uint(1)},
		[]slate.Value{slate.Uint(2)},
	)
	if err != nil {
		t.Fatalf("related: %v", err)
	}
	for at, want := range []string{"main", "annexe"} {
		if len(got[at]) != 1 {
			t.Fatalf("parent %d: want one row, got %d", at, len(got[at]))
		}
		name, ok := got[at][0][1].(slate.String)
		if !ok || string(name) != want {
			t.Errorf("parent %d: want %q, got %v", at, want, got[at][0][1])
		}
	}
}

// Two parents with the same key both get the rows, from the one group the
// server sent — which is the saving, on the response as well as the read.
func TestRelatedRepeatedKeysShareOneGroup(t *testing.T) {
	session := shelved(t)
	got, err := session.Related(
		testContext(t),
		"shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		[]slate.Value{slate.Uint(1)},
		[]slate.Value{slate.Uint(1)},
	)
	if err != nil {
		t.Fatalf("related: %v", err)
	}
	if len(got) != 2 || len(got[0]) != 2 || len(got[1]) != 2 {
		t.Fatalf("both parents should get both shelves, got %v", got)
	}
}

func TestRelatedWithNoParentsAsksNothing(t *testing.T) {
	session := shelved(t)
	got, err := session.Related(
		testContext(t),
		"shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
	)
	if err != nil {
		t.Fatalf("related: %v", err)
	}
	if len(got) != 0 {
		t.Errorf("want nothing, got %v", got)
	}
}

func TestRelatedNamesTheForeignKeysThatExist(t *testing.T) {
	session := shelved(t)
	_, err := session.Related(
		testContext(t),
		"shelves",
		slate.Relation{On: "shelves", Through: "nosuch", Way: slate.Children},
		[]slate.Value{slate.Uint(1)},
	)
	if err == nil {
		t.Fatal("an unknown foreign key should be refused")
	}
	if !strings.Contains(err.Error(), "shelf_library") {
		t.Errorf("the refusal should name the key that does exist: %v", err)
	}
}

// The oracle: one relationship load equals a filtered read per parent.
//
// A second, independent way of getting the same answer, so a Related that
// dropped or duplicated a row disagrees with it rather than with a hand-written
// expectation that was copied from the seed data.
func TestRelatedAgreesWithFilteringPerParent(t *testing.T) {
	session := shelved(t)
	ctx := testContext(t)

	related, err := session.Related(
		ctx,
		"shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		[]slate.Value{slate.Uint(1)},
		[]slate.Value{slate.Uint(2)},
		[]slate.Value{slate.Uint(3)},
	)
	if err != nil {
		t.Fatalf("related: %v", err)
	}

	for at, library := range []uint64{1, 2, 3} {
		stream, err := session.Query(ctx, slate.Query{
			Table:  "shelves",
			Filter: slate.Filter(slate.Eq(1, slate.Uint(library))),
		})
		if err != nil {
			t.Fatalf("query: %v", err)
		}
		var byHand int
		// Row(), not Next() alone: Next() reports whether a row is available and
		// Row() is what advances the cursor, so a loop that never reads the row
		// never moves and spins at 100% CPU forever. Written that way first.
		for stream.Next() {
			stream.Row()
			byHand++
		}
		if err := stream.Err(); err != nil {
			t.Fatalf("stream: %v", err)
		}
		stream.Close()
		if len(related[at]) != byHand {
			t.Errorf("library %d: related gave %d, a filter gave %d",
				library, len(related[at]), byHand)
		}
	}
}

// A relationship read inside a transaction sees that transaction's writes.
//
// The session-level read is the same call with no transaction on it, so this
// also pins down that the two are not accidentally the same request: the shelf
// added here must be invisible to it until the commit.
func TestRelatedInsideATransactionSeesItsOwnWrites(t *testing.T) {
	session := shelved(t)
	ctx := testContext(t)

	tx, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	defer func() { _ = tx.Rollback(ctx) }()

	if _, err := tx.Insert(ctx, "shelves",
		[]slate.Value{slate.Uint(103), slate.Uint(3), slate.String("atlases")}); err != nil {
		t.Fatalf("insert: %v", err)
	}

	inside, err := tx.Related(ctx, "shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		[]slate.Value{slate.Uint(3)})
	if err != nil {
		t.Fatalf("related in transaction: %v", err)
	}
	if len(inside[0]) != 1 {
		t.Errorf("the transaction should see its own shelf, got %d rows", len(inside[0]))
	}

	outside, err := session.Related(ctx, "shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		[]slate.Value{slate.Uint(3)})
	if err != nil {
		t.Fatalf("related outside: %v", err)
	}
	if len(outside[0]) != 0 {
		t.Errorf("an uncommitted shelf should not be visible outside, got %v", outside[0])
	}
}

// A relating key is one value, and a caller sending two is told so here.
//
// Refused in the client rather than at the server: a relationship relates on a
// single column, so there is nothing a second value could mean, and the round
// trip would only bring back the same answer later.
func TestRelatedRefusesACompositeKey(t *testing.T) {
	session := shelved(t)
	_, err := session.Related(
		testContext(t),
		"shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		[]slate.Value{slate.Uint(1), slate.Uint(2)},
	)
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Errorf("a two-value key: err = %v, want KindInvalidRequest", err)
	}
}

// The schema claim rides on Related too.
//
// The claim is attached at every call site by hand, so a new one that forgets
// it is not checked at all and nothing else notices — which is exactly what a
// mutation dropping it proved, before this test existed.
func TestRelatedCarriesTheSchemaClaim(t *testing.T) {
	server := start(t, relatedTables)
	// `shelves`, declared with two of its columns the wrong way round.
	misdeclared := slate.TableDef{
		Name: "shelves",
		Columns: []slate.ColumnDef{
			{Name: "id", Type: slate.TypeUint},
			{Name: "label", Type: slate.TypeString},
			{Name: "library_id", Type: slate.TypeUint},
		},
		PrimaryKey: []string{"id"},
	}
	session := server.client(t).Declaring(slate.Schemas{"shelves": misdeclared}).Session()

	_, err := session.Related(
		testContext(t),
		"shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		[]slate.Value{slate.Uint(1)},
	)
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Errorf("a misdeclared table: err = %v, want KindInvalidRequest", err)
	}
}

// dialRecording dials the node with an interceptor over every call it makes,
// unary and streaming both.
//
// The freshness floor and the request count are properties of the *request*,
// not of the answer: a client that dropped the floor, or that looped over the
// parents one at a time, would return exactly the right rows. Nothing
// assertable about the result distinguishes them, so the request itself is
// what has to be looked at.
//
// Both interceptors, because a read is server-streaming and a write is unary.
// This took only the unary one until a paging count was written against it and
// reported zero `/Query` calls for four pages that plainly happened — which
// reads as "a read is free" rather than as a hole in the instrument. The same
// trap is written up in the Python client's `Counting`, which registers both
// grpc interfaces for the same reason; here it was met rather than avoided.
//
// A stream interceptor sees the call being opened and not the messages, which
// is exactly the round trip a caller pays for: one `Query` per page.
func dialRecording(
	t *testing.T,
	server *serving,
	seen func(method string, request proto.Message),
) *slate.Client {
	t.Helper()
	client, err := slate.Dial(server.addr, slate.Identity{
		Principal: "u64:1",
		Tenant:    "u64:1",
		Roles:     []string{"app"},
	},
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithChainUnaryInterceptor(func(
			ctx context.Context,
			method string,
			request, reply any,
			cc *grpc.ClientConn,
			invoke grpc.UnaryInvoker,
			opts ...grpc.CallOption,
		) error {
			if message, ok := request.(proto.Message); ok {
				seen(method, message)
			}
			return invoke(ctx, method, request, reply, cc, opts...)
		}),
		grpc.WithChainStreamInterceptor(func(
			ctx context.Context,
			desc *grpc.StreamDesc,
			cc *grpc.ClientConn,
			method string,
			streamer grpc.Streamer,
			opts ...grpc.CallOption,
		) (grpc.ClientStream, error) {
			// Nothing is reported here, and that is a change. This used to
			// call `seen(method, nil)` on the open, because grpc hands a
			// stream interceptor the call and not the payload — an honest nil
			// for a body it could not see. Reporting on the open *and* on the
			// send would count every read twice, so the report moved to the
			// send: a server-streaming RPC sends its one request immediately
			// after opening, so the count is the same and the message is no
			// longer nil. Go could count a read and not weigh one until this.
			stream, err := streamer(ctx, desc, cc, method, opts...)
			if err != nil {
				return stream, err
			}
			// And the body arrives later, through SendMsg, which is why the
			// wrapper exists. Without it Go could count a read and never weigh
			// one: the comment that used to stand here said "nothing needs it
			// yet", and `ledger/2026-09-29-bytes-do-not-need-a-network.md`
			// needed it the moment bytes were asked for in all three clients.
			return &sendingStream{ClientStream: stream, method: method, seen: seen}, nil
		}),
	)
	if err != nil {
		t.Fatalf("dialling %s: %v", server.addr, err)
	}
	t.Cleanup(func() { _ = client.Close() })
	return client
}

// sendingStream reports each message a client stream sends.
//
// grpc-go hands a stream interceptor the call and not the payload, so a
// `Query` — the shape every read takes — is invisible to the interceptor
// itself. Embedding `grpc.ClientStream` and overriding the one method keeps
// every other part of the stream exactly what grpc built, which matters:
// a hand-written forwarder would have to track the interface as it grows.
type sendingStream struct {
	grpc.ClientStream
	method string
	seen   func(method string, request proto.Message)
}

func (s *sendingStream) SendMsg(m any) error {
	if message, ok := m.(proto.Message); ok {
		s.seen(s.method, message)
	}
	return s.ClientStream.SendMsg(m)
}

// Whether a request message has a field set, by name.
//
// Read reflectively rather than by importing the generated package, so this
// says "the wire carries a freshness" rather than "this Go struct has a
// pointer", which is the thing actually under test.
func hasField(t *testing.T, message proto.Message, name string) bool {
	t.Helper()
	reflected := message.ProtoReflect()
	field := reflected.Descriptor().Fields().ByName(protoreflect.Name(name))
	if field == nil {
		t.Fatalf("%s has no field %q", reflected.Descriptor().FullName(), name)
	}
	return reflected.Has(field)
}

// A monotonic session carries its read watermark onto Related.
//
// Without it a relationship load can be served by a replica that has not yet
// caught up with the write this same session just made — the rows come back
// empty and nothing looks wrong. The harness runs one node, where a stale read
// cannot happen, so the floor's absence is invisible in the answer; this
// asserts it is on the request instead.
func TestRelatedCarriesTheFreshnessFloor(t *testing.T) {
	server := start(t, relatedTables)
	var related []proto.Message
	client := dialRecording(t, server, func(method string, request proto.Message) {
		if strings.HasSuffix(method, "/Related") {
			related = append(related, request)
		}
	})
	session := client.Session()
	ctx := testContext(t)

	// A write first: the floor is the watermark, and a session that has read
	// and written nothing has no watermark to send.
	if _, err := session.Insert(ctx, "libraries",
		[]slate.Value{slate.Uint(1), slate.String("main")}); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	if _, err := session.Related(ctx, "shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		[]slate.Value{slate.Uint(1)}); err != nil {
		t.Fatalf("related: %v", err)
	}

	if len(related) != 1 {
		t.Fatalf("want one Related request, got %d", len(related))
	}
	if !hasField(t, related[0], "freshness") {
		t.Errorf("the request carried no freshness floor: %v", related[0])
	}

	// A session that does not want monotonic reads still reads its own writes,
	// so it *also* carries a floor once it has written.
	//
	// This asserted the opposite until 2026-09-29, when comparing the three
	// clients found that Python did it this way and Go and TypeScript did not:
	// gating the floor on the flag dropped read-your-writes along with
	// monotonic reads, and a caller who wrote and read back could miss their
	// own write with no error anywhere. The flag is named for monotonic reads
	// and now decides only that. See
	// ledger/2026-09-29-read-your-writes-is-not-monotonic-reads.md.
	//
	// The write still comes first, and is still the point: without it the
	// session has no watermark and the assertion would hold for a reason that
	// has nothing to do with the flag. That hole was here for a fortnight.
	related = nil
	loose := client.SessionWithoutMonotonicReads()
	if _, err := loose.Insert(ctx, "libraries",
		[]slate.Value{slate.Uint(2), slate.String("annexe")}); err != nil {
		t.Fatalf("seeding the loose session: %v", err)
	}
	if _, err := loose.Related(ctx, "shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		[]slate.Value{slate.Uint(1)}); err != nil {
		t.Fatalf("related: %v", err)
	}
	if len(related) != 1 {
		t.Fatalf("want one Related request, got %d", len(related))
	}
	if !hasField(t, related[0], "freshness") {
		t.Errorf("a non-monotonic session that wrote still reads its own "+
			"writes, so it carries a floor: %v", related[0])
	}

	// And the half that gives the flag its meaning: a loose session that has
	// only *read* still sends no floor, because a read's `served_by` is not
	// folded into its watermark. Two reads, not one — with a single read there
	// is nothing for the first to have folded in, so the assertion would hold
	// against a client that ignored the flag entirely.
	//
	// Written because a mutation found it missing: removing the `!s.monotonic`
	// guard from `observeServedBy` survived the whole file.
	related = nil
	reader := client.SessionWithoutMonotonicReads()
	for range 2 {
		if _, err := reader.Related(ctx, "shelves",
			slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
			[]slate.Value{slate.Uint(1)}); err != nil {
			t.Fatalf("related: %v", err)
		}
	}
	if len(related) != 2 {
		t.Fatalf("want two Related requests, got %d", len(related))
	}
	if hasField(t, related[1], "freshness") {
		t.Errorf("a read must not advance a non-monotonic session's "+
			"watermark, so the second read carries no floor: %v", related[1])
	}
}

// One request, however many parents — the whole reason Related exists.
//
// A client that looped over the parents would return identical rows, so this
// counts the requests rather than checking them.
func TestRelatedIsOneRequestHoweverManyParents(t *testing.T) {
	server := start(t, relatedTables)
	var requests int
	client := dialRecording(t, server, func(method string, _ proto.Message) {
		if strings.HasSuffix(method, "/Related") {
			requests++
		}
	})
	session := client.Session()
	ctx := testContext(t)

	keys := make([][]slate.Value, 0, 50)
	for parent := uint64(1); parent <= 50; parent++ {
		keys = append(keys, []slate.Value{slate.Uint(parent)})
	}
	got, err := session.Related(ctx, "shelves",
		slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
		keys...)
	if err != nil {
		t.Fatalf("related: %v", err)
	}
	if len(got) != 50 {
		t.Errorf("want fifty entries, got %d", len(got))
	}
	if requests != 1 {
		t.Errorf("fifty parents took %d requests, want 1", requests)
	}
}
