package slate_test

import (
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

const pagingTables = `
[[tables]]
name = "notes"
id = 30
columns = [
  { name = "id",   type = "u64" },
  { name = "body", type = "str" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["notes"]
actions = ["everything"]
`

const noteCount = 25

// noted starts a node holding twenty-five notes, ids 1..25.
func noted(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, pagingTables).client(t).Session()
	ctx := testContext(t)
	rows := make([][]slate.Value, 0, noteCount)
	for id := uint64(1); id <= noteCount; id++ {
		rows = append(rows, []slate.Value{slate.Uint(id), slate.String("note")})
	}
	if _, err := session.Insert(ctx, "notes", rows...); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	return session
}

func idsOf(t *testing.T, page *slate.Page) []uint64 {
	t.Helper()
	out := make([]uint64, 0, len(page.Rows))
	for _, row := range page.Rows {
		id, ok := row[0].(slate.Uint)
		if !ok {
			t.Fatalf("the first column is not a uint: %v", row[0])
		}
		out = append(out, uint64(id))
	}
	return out
}

func TestAFullPageCarriesACursor(t *testing.T) {
	session := noted(t)
	page, err := session.Page(testContext(t), slate.Query{
		Table: "notes", Limit: slate.Limit(10),
	})
	if err != nil {
		t.Fatalf("page: %v", err)
	}
	if got := len(page.Rows); got != 10 {
		t.Errorf("want ten rows, got %d", got)
	}
	if page.IsLast() {
		t.Error("a full page should not end the sequence")
	}
}

func TestAShortPageEndsTheSequence(t *testing.T) {
	session := noted(t)
	page, err := session.Page(testContext(t), slate.Query{
		Table: "notes", Limit: slate.Limit(1000),
	})
	if err != nil {
		t.Fatalf("page: %v", err)
	}
	if got := len(page.Rows); got != noteCount {
		t.Errorf("want every note, got %d", got)
	}
	if !page.IsLast() {
		t.Errorf("a short page proves there is nothing after it: %v", page.Cursor)
	}
}

func TestTheCursorResumesStrictlyAfterTheLastRow(t *testing.T) {
	session := noted(t)
	ctx := testContext(t)

	first, err := session.Page(ctx, slate.Query{Table: "notes", Limit: slate.Limit(10)})
	if err != nil {
		t.Fatalf("first page: %v", err)
	}
	second, err := session.Page(ctx, slate.Query{
		Table: "notes", Limit: slate.Limit(10), After: first.Cursor,
	})
	if err != nil {
		t.Fatalf("second page: %v", err)
	}

	want := []uint64{11, 12, 13, 14, 15, 16, 17, 18, 19, 20}
	if got := idsOf(t, second); !equalIDs(got, want) {
		t.Errorf("second page: got %v, want %v", got, want)
	}
	// Strictly after: nothing appears on both pages.
	seen := map[uint64]bool{}
	for _, id := range idsOf(t, first) {
		seen[id] = true
	}
	for _, id := range idsOf(t, second) {
		if seen[id] {
			t.Errorf("row %d appeared on both pages", id)
		}
	}
}

// The property `OFFSET` cannot offer.
//
// Between pages, a row *behind* the cursor is deleted. Under offset-based
// paging that shifts the window and the reader silently skips a row it has not
// seen. A key does not move when its neighbours change.
func TestPagingVisitsEveryRowExactlyOnceUnderConcurrentWrites(t *testing.T) {
	session := noted(t)
	ctx := testContext(t)

	var seen []uint64
	var cursor []slate.Value
	deleted := 0
	// Bounded, because a bug in the cursor is exactly a loop that never ends:
	// a server returning one for a short page would page forever and this test
	// would hang rather than fail. Twenty-five notes in pages of five is six
	// requests; twenty is room for a different fixture and not for a loop.
	for round := 0; round < 20; round++ {
		if round == 19 {
			t.Fatalf("paging did not terminate: %v", seen)
		}
		page, err := session.Page(ctx, slate.Query{
			Table: "notes", Limit: slate.Limit(5), After: cursor,
		})
		if err != nil {
			t.Fatalf("page: %v", err)
		}
		seen = append(seen, idsOf(t, page)...)
		if page.IsLast() {
			break
		}
		cursor = page.Cursor

		// A *different* row each round, all of them behind the cursor and
		// already in `seen`, so anything re-read shows up as a duplicate
		// below. Deleting the same row twice would refuse rather than churn.
		if _, err := session.Delete(ctx, "notes",
			[]slate.Value{slate.Uint(seen[deleted])}); err != nil {
			t.Fatalf("deleting behind the cursor: %v", err)
		}
		deleted++
	}
	if deleted == 0 {
		t.Fatal("the churn never happened, so this asserts nothing")
	}

	counts := map[uint64]int{}
	for _, id := range seen {
		counts[id]++
		if counts[id] > 1 {
			t.Fatalf("row %d was visited twice: %v", id, seen)
		}
	}
	want := make([]uint64, 0, noteCount)
	for id := uint64(1); id <= noteCount; id++ {
		want = append(want, id)
	}
	if !equalIDs(seen, want) {
		t.Errorf("paging visited %v, want every row in key order", seen)
	}
}

func TestAPageWithNoLimitIsRefused(t *testing.T) {
	session := noted(t)
	_, err := session.Page(testContext(t), slate.Query{Table: "notes"})
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Errorf("a page with no size: err = %v, want KindInvalidRequest", err)
	}
	if err != nil && !strings.Contains(err.Error(), "limit") {
		t.Errorf("the refusal should name the missing field: %v", err)
	}
}

// Refused rather than served without a cursor, which would be the silent
// version: the caller loops until the cursor is empty, gets none on the first
// page, and reads five rows of the table as the whole answer.
func TestAProjectionThatDropsTheKeyIsRefusedWhenPaged(t *testing.T) {
	session := noted(t)
	ctx := testContext(t)

	_, err := session.Page(ctx, slate.Query{
		Table: "notes", Limit: slate.Limit(5), Columns: []slate.Ordinal{1},
	})
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Fatalf("a paged read with no key column: err = %v", err)
	}
	if !strings.Contains(err.Error(), "id") {
		t.Errorf("the refusal should name the missing key column: %v", err)
	}

	// The same projection reads fine when nothing is paging.
	stream, err := session.Query(ctx, slate.Query{
		Table: "notes", Limit: slate.Limit(5), Columns: []slate.Ordinal{1},
	})
	if err != nil {
		t.Fatalf("an unpaged projection is unaffected: %v", err)
	}
	rows := 0
	for stream.Next() {
		stream.Row()
		rows++
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("stream: %v", err)
	}
	stream.Close()
	if rows != 5 {
		t.Errorf("want five rows, got %d", rows)
	}
}

// `After` without `Paged`, for a caller keeping its own key. `Query` is
// unchanged by any of this: it sends no `paged`, gets no cursor, and honours
// `After` exactly as the kernel does.
func TestACursorAlonePagesWithoutAskingForOneBack(t *testing.T) {
	session := noted(t)
	stream, err := session.Query(testContext(t), slate.Query{
		Table: "notes",
		Limit: slate.Limit(3),
		After: []slate.Value{slate.Uint(10)},
	})
	if err != nil {
		t.Fatalf("query: %v", err)
	}
	var ids []uint64
	for stream.Next() {
		row := stream.Row()
		id, ok := row[0].(slate.Uint)
		if !ok {
			t.Fatalf("the first column is not a uint: %v", row[0])
		}
		ids = append(ids, uint64(id))
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("stream: %v", err)
	}
	stream.Close()
	if want := []uint64{11, 12, 13}; !equalIDs(ids, want) {
		t.Errorf("got %v, want %v", ids, want)
	}
}

func equalIDs(a, b []uint64) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
