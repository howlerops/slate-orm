package slate_test

import (
	"errors"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

func rows(t *testing.T, s *slate.RowStream) [][]slate.Value {
	t.Helper()
	out, err := s.Collect()
	if err != nil {
		t.Fatalf("draining the stream: %v", err)
	}
	return out
}

func TestInsertThenGet(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	result, err := session.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(1), slate.String("note"), slate.Int(10)})
	if err != nil {
		t.Fatalf("inserting: %v", err)
	}
	if result.Affected != 1 {
		t.Errorf("affected = %d, want 1", result.Affected)
	}
	// The writer's position after the write, which is what a caller carries to
	// another session to read its own write there.
	if result.Sequence == nil {
		t.Error("a write should report the sequence it landed at")
	}

	row, found, err := session.Get(ctx, "docs", []slate.Value{slate.Uint(1)})
	if err != nil {
		t.Fatalf("getting: %v", err)
	}
	if !found {
		t.Fatal("the row just inserted was not found")
	}
	if len(row) != 3 || row[1] != slate.String("note") || row[2] != slate.Int(10) {
		t.Errorf("row = %v, want [1 note 10]", row)
	}
}

// A missing row is `found == false`, not an error: checking existence should
// not mean matching on an error type.
func TestGetMissingIsNotAnError(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	row, found, err := session.Get(ctx, "docs", []slate.Value{slate.Uint(404)})
	if err != nil {
		t.Fatalf("getting a missing row should not error: %v", err)
	}
	if found {
		t.Errorf("found a row that was never written: %v", row)
	}
}

func TestInsertRefusesADuplicateKey(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	row := []slate.Value{slate.Uint(1), slate.String("a"), slate.Int(1)}
	if _, err := session.Insert(ctx, "docs", row); err != nil {
		t.Fatalf("first insert: %v", err)
	}
	_, err := session.Insert(ctx, "docs", row)
	if err == nil {
		t.Fatal("a duplicate primary key must be refused")
	}
	if !slate.IsKind(err, slate.KindAlreadyExists) {
		t.Errorf("err = %v, want KindAlreadyExists", err)
	}
	var e *slate.Error
	if errors.As(err, &e) && e.Retryable() {
		t.Error("a duplicate key is not retryable")
	}
}

// Upsert is the same call with a flag, so it is worth pinning that the flag
// actually reaches the server rather than the two behaving identically.
func TestUpsertReplacesWhereInsertRefuses(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	if _, err := session.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(1), slate.String("first"), slate.Int(1)}); err != nil {
		t.Fatalf("inserting: %v", err)
	}
	if _, err := session.Upsert(ctx, "docs",
		[]slate.Value{slate.Uint(1), slate.String("second"), slate.Int(2)}); err != nil {
		t.Fatalf("upserting: %v", err)
	}
	row, found, err := session.Get(ctx, "docs", []slate.Value{slate.Uint(1)})
	if err != nil || !found {
		t.Fatalf("get after upsert: %v found=%v", err, found)
	}
	if row[1] != slate.String("second") {
		t.Errorf("row = %v, want the upserted values", row)
	}
}

func TestQueryFiltersAndOrders(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	for i := uint64(1); i <= 5; i++ {
		if _, err := session.Insert(ctx, "docs", []slate.Value{
			slate.Uint(i), slate.String("k"), slate.Int(int64(i) * 10),
		}); err != nil {
			t.Fatalf("seeding %d: %v", i, err)
		}
	}

	stream, err := session.Query(ctx, slate.Query{
		Table:  "docs",
		Filter: slate.Filter(slate.Ge(2, slate.Int(30))),
		Sort:   []slate.SortKey{{Column: 2, Direction: slate.Desc}},
	})
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	got := rows(t, stream)
	if len(got) != 3 {
		t.Fatalf("got %d rows, want 3 (sizes 30, 40, 50)", len(got))
	}
	if got[0][2] != slate.Int(50) || got[2][2] != slate.Int(30) {
		t.Errorf("rows are not in descending size order: %v", got)
	}
}

func TestQueryLimitAndProjection(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	for i := uint64(1); i <= 4; i++ {
		if _, err := session.Insert(ctx, "docs", []slate.Value{
			slate.Uint(i), slate.String("k"), slate.Int(int64(i)),
		}); err != nil {
			t.Fatalf("seeding: %v", err)
		}
	}

	stream, err := session.Query(ctx, slate.Query{
		Table:   "docs",
		Limit:   slate.Limit(2),
		Columns: []slate.Ordinal{0},
	})
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	got := rows(t, stream)
	if len(got) != 2 {
		t.Fatalf("got %d rows, want 2", len(got))
	}
	// Columns outside the projection come back null rather than absent, so the
	// row keeps its shape and an ordinal still means what it meant.
	if _, isNull := got[0][1].(slate.Null); !isNull {
		t.Errorf("an unprojected column should be null, got %v", got[0][1])
	}
}

func TestTransactionRollbackDiscards(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	tx, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("beginning: %v", err)
	}
	if _, err := tx.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(7), slate.String("x"), slate.Int(1)}); err != nil {
		t.Fatalf("inserting in the transaction: %v", err)
	}
	// Visible inside, before the commit.
	if _, found, err := tx.Get(ctx, "docs", []slate.Value{slate.Uint(7)}); err != nil || !found {
		t.Fatalf("a transaction must see its own writes: %v found=%v", err, found)
	}
	if err := tx.Rollback(ctx); err != nil {
		t.Fatalf("rolling back: %v", err)
	}
	if _, found, err := session.Get(ctx, "docs", []slate.Value{slate.Uint(7)}); err != nil || found {
		t.Fatalf("a rolled-back write must not be visible: %v found=%v", err, found)
	}
}

func TestTransactionCommitLands(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	tx, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("beginning: %v", err)
	}
	defer func() { _ = tx.Rollback(ctx) }()

	if _, err := tx.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(8), slate.String("y"), slate.Int(2)}); err != nil {
		t.Fatalf("inserting: %v", err)
	}
	if err := tx.Commit(ctx); err != nil {
		t.Fatalf("committing: %v", err)
	}
	if _, found, err := session.Get(ctx, "docs", []slate.Value{slate.Uint(8)}); err != nil || !found {
		t.Fatalf("a committed write must be visible: %v found=%v", err, found)
	}
}

// `defer tx.Rollback(ctx)` is the shape every transaction should use, so
// rolling back after a commit has to be a no-op rather than an error.
func TestRollbackAfterCommitIsQuiet(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	tx, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("beginning: %v", err)
	}
	if err := tx.Commit(ctx); err != nil {
		t.Fatalf("committing: %v", err)
	}
	if err := tx.Rollback(ctx); err != nil {
		t.Errorf("rollback after commit should be quiet, got %v", err)
	}
}

func TestDelete(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	if _, err := session.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(3), slate.String("z"), slate.Int(1)}); err != nil {
		t.Fatalf("inserting: %v", err)
	}
	result, err := session.Delete(ctx, "docs", []slate.Value{slate.Uint(3)})
	if err != nil {
		t.Fatalf("deleting: %v", err)
	}
	if result.Affected != 1 {
		t.Errorf("affected = %d, want 1", result.Affected)
	}
	if _, found, _ := session.Get(ctx, "docs", []slate.Value{slate.Uint(3)}); found {
		t.Error("the deleted row is still there")
	}
}

// Every value kind must survive the round trip. A client that encodes a u64 as
// an i64 gets a different value back from this server, because its ordering is
// type-first — so this is not a formality.
func TestEveryValueKindRoundTrips(t *testing.T) {
	server := start(t, `
[[tables]]
name = "kinds"
id = 2
columns = [
  { name = "id",  type = "u64" },
  { name = "b",   type = "bool" },
  { name = "by",  type = "bytes" },
  { name = "s",   type = "str" },
  { name = "i",   type = "i64" },
  { name = "u",   type = "u64" },
  { name = "f",   type = "f64" },
  { name = "uu",  type = "uuid" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["kinds"]
actions = ["everything"]
`)
	session := server.client(t).Session()
	ctx := testContext(t)

	uu := slate.UUID{0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
		0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10}
	written := []slate.Value{
		slate.Uint(1), slate.Bool(true), slate.Bytes{0xde, 0xad},
		slate.String("hello"), slate.Int(-5), slate.Uint(5),
		slate.Float(1.5), uu,
	}
	if _, err := session.Insert(ctx, "kinds", written); err != nil {
		t.Fatalf("inserting: %v", err)
	}

	read, found, err := session.Get(ctx, "kinds", []slate.Value{slate.Uint(1)})
	if err != nil || !found {
		t.Fatalf("getting: %v found=%v", err, found)
	}
	if read[1] != slate.Bool(true) {
		t.Errorf("bool = %v", read[1])
	}
	if string(read[2].(slate.Bytes)) != "\xde\xad" {
		t.Errorf("bytes = %v", read[2])
	}
	if read[3] != slate.String("hello") {
		t.Errorf("string = %v", read[3])
	}
	if read[4] != slate.Int(-5) {
		t.Errorf("int = %v", read[4])
	}
	if read[5] != slate.Uint(5) {
		t.Errorf("uint = %v", read[5])
	}
	if read[6] != slate.Float(1.5) {
		t.Errorf("float = %v", read[6])
	}
	if read[7] != uu {
		t.Errorf("uuid = %v, want %v", read[7], uu)
	}
	// An i64 and a u64 of the same magnitude must not come back the same.
	if read[4] == read[5] {
		t.Error("i64 -5 and u64 5 must not decode to the same value")
	}
}

func TestExplainNeedsTheExplainGrant(t *testing.T) {
	// This node grants `all` — the four data actions — and not `explain`.
	server := start(t, `
[[tables]]
name = "plain"
id = 3
columns = [{ name = "id", type = "u64" }]
primary_key = ["id"]

[[security.grants]]
role = "reader"
tables = ["plain"]
actions = ["all"]
`)
	c, err := slate.Dial(server.addr, slate.Identity{
		Principal: "u64:2", Tenant: "u64:1", Roles: []string{"reader"},
	})
	if err != nil {
		t.Fatalf("dialling: %v", err)
	}
	defer func() { _ = c.Close() }()
	ctx := testContext(t)

	_, err = c.Session().Explain(ctx, slate.Query{Table: "plain"})
	if err == nil {
		t.Fatal("a `read` grant must not carry EXPLAIN")
	}
	if !slate.IsKind(err, slate.KindPermissionDenied) {
		t.Errorf("err = %v, want KindPermissionDenied", err)
	}
}

func TestExplainWithTheGrant(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	if _, err := session.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(1), slate.String("k"), slate.Int(1)}); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	plan, err := session.Explain(ctx, slate.Query{Table: "docs"})
	if err != nil {
		t.Fatalf("explaining: %v", err)
	}
	if plan.Table != "docs" {
		t.Errorf("plan is for %q, want docs", plan.Table)
	}
	if plan.Access == "" {
		t.Error("a plan should name its access path")
	}
}

func TestLeadership(t *testing.T) {
	server := start(t, "")
	ctx := testContext(t)

	status, err := server.client(t).Leadership(ctx)
	if err != nil {
		t.Fatalf("asking about leadership: %v", err)
	}
	if !status.Leader {
		t.Errorf("a lone node should hold the lease: %+v", status)
	}
}

// A session's watermark must advance past its own writes, which is what makes
// a later read see them.
func TestSessionWatermarkAdvancesOnWrite(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	if before := session.Watermark(); before != nil {
		t.Errorf("a fresh session has no watermark, got %v", *before)
	}
	if _, err := session.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(1), slate.String("k"), slate.Int(1)}); err != nil {
		t.Fatalf("inserting: %v", err)
	}
	after := session.Watermark()
	if after == nil {
		t.Fatal("a write must advance the session's watermark")
	}
	// A second write must move it further, not merely set it once.
	if _, err := session.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(2), slate.String("k"), slate.Int(2)}); err != nil {
		t.Fatalf("second insert: %v", err)
	}
	later := session.Watermark()
	if later == nil || *later <= *after {
		t.Errorf("the watermark should advance again: %v then %v", after, later)
	}
	if *after == 0 {
		t.Error("the watermark should be a real sequence")
	}
}

func TestUnknownTableIsNotFound(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Session()
	ctx := testContext(t)

	_, _, err := session.Get(ctx, "nosuchtable", []slate.Value{slate.Uint(1)})
	if err == nil {
		t.Fatal("an unknown table must be refused")
	}
	if !slate.IsKind(err, slate.KindNotFound) {
		t.Errorf("err = %v, want KindNotFound", err)
	}
}
