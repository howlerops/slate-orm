package slate_test

import (
	"fmt"
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// Authors and books with uneven fan-out, so a page of N authors is more than N
// rows and a test that confused the two would fail rather than pass by luck.
//
// "region" sits in front of "id" for the reason `related_path_test.go` gives:
// a fixture where the right answer is ordinal 0 cannot tell "read the key"
// from "assume the first column".
const pagedTables = `
[[tables]]
name = "writers"
id = 40
columns = [
  { name = "region", type = "str" },
  { name = "id",     type = "u64" },
  { name = "name",   type = "str" },
]
primary_key = ["region", "id"]

[[tables]]
name = "works"
id = 41
columns = [
  { name = "id",        type = "u64" },
  { name = "writer_id", type = "u64" },
  { name = "title",     type = "str" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["writers", "works"]
actions = ["everything"]
`

const writers = 6

// seedPaged writes writers 0..5 in one region, writer n having n works — so
// writer 0 has none, which is the input-0 row that matches nothing.
func seedPaged(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, pagedTables).client(t).Session()
	ctx := testContext(t)
	for id := 0; id < writers; id++ {
		if _, err := session.Insert(ctx, "writers",
			[]slate.Value{slate.String("north"), slate.Uint(uint64(id)), slate.String(fmt.Sprintf("w-%d", id))}); err != nil {
			t.Fatalf("seeding writers: %v", err)
		}
	}
	work := uint64(0)
	for id := 0; id < writers; id++ {
		for n := 0; n < id; n++ {
			work++
			if _, err := session.Insert(ctx, "works",
				[]slate.Value{slate.Uint(work), slate.Uint(uint64(id)), slate.String("t")}); err != nil {
				t.Fatalf("seeding works: %v", err)
			}
		}
	}
	return session
}

// pagedQuery joins writers to their works, resuming after `cursor`.
func pagedQuery(cursor []slate.Value, limit *uint64) slate.JoinQuery {
	join := slate.NewJoin()
	writersAt := join.Add(slate.JoinInput{Table: "writers"})
	join.Add(slate.JoinInput{
		Table: "works",
		On:    []slate.On{{Earlier: slate.At(writersAt, 1), Own: 1}},
	})
	query := join.Query()
	query.Limit = limit
	query.After = cursor
	return query
}

type pair struct{ writer, work uint64 }

func pairsOf(t *testing.T, rows [][][]slate.Value) []pair {
	t.Helper()
	out := make([]pair, 0, len(rows))
	for _, row := range rows {
		if row[0] == nil || row[1] == nil {
			t.Fatalf("an inner join returned an absent input: %v", row)
		}
		out = append(out, pair{writer: mustUint(t, row[0][1]), work: mustUint(t, row[1][0])})
	}
	return out
}

func mustUint(t *testing.T, value slate.Value) uint64 {
	t.Helper()
	n, ok := value.(slate.Uint)
	if !ok {
		t.Fatalf("expected a u64, got %T", value)
	}
	return uint64(n)
}

// Paging start to finish reproduces the whole join, and nothing twice.
//
// The property the item is about: every joined row derives from exactly one
// input-0 row, so visiting every input-0 row once visits every joined row once.
func TestPagingAJoinReproducesTheWholeJoin(t *testing.T) {
	session := seedPaged(t)
	ctx := testContext(t)

	stream, err := session.Join(ctx, pagedQuery(nil, nil))
	if err != nil {
		t.Fatalf("the unpaged join: %v", err)
	}
	all, err := stream.Collect()
	if err != nil {
		t.Fatalf("collecting the unpaged join: %v", err)
	}
	whole := pairsOf(t, all)

	for _, size := range []uint64{1, 2, 3} {
		seen := []pair{}
		var cursor []slate.Value
		for guard := 0; ; guard++ {
			if guard > 40 {
				t.Fatalf("pages of %d did not terminate: the cursor is stuck", size)
			}
			page, err := session.PageJoin(ctx, pagedQuery(cursor, &size))
			if err != nil {
				t.Fatalf("page: %v", err)
			}
			seen = append(seen, pairsOf(t, page.Rows)...)
			if page.IsLast() {
				break
			}
			cursor = page.Cursor
		}
		if len(seen) != len(whole) {
			t.Fatalf("pages of %d gave %d rows, the whole join has %d", size, len(seen), len(whole))
		}
		counts := map[pair]int{}
		for _, row := range seen {
			counts[row]++
		}
		for _, row := range whole {
			if counts[row] != 1 {
				t.Fatalf("pages of %d: %v seen %d times", size, row, counts[row])
			}
		}
	}
}

// A page is bounded in input-0 rows, not in returned rows.
//
// Writers 1, 2 and 3 have one, two and three works, so a page of three writers
// is six rows. A test asserting len(rows) <= limit would pass on a fixture
// with no fan-out; this one has one on purpose.
func TestAPageIsBoundedInInputZeroRows(t *testing.T) {
	session := seedPaged(t)
	ctx := testContext(t)
	size := uint64(4)
	page, err := session.PageJoin(ctx, pagedQuery(nil, &size))
	if err != nil {
		t.Fatalf("page: %v", err)
	}
	seen := map[uint64]bool{}
	for _, row := range pairsOf(t, page.Rows) {
		seen[row.writer] = true
	}
	// Writer 0 has no works, so four *writers* yields writers 1, 2 and 3.
	if len(seen) != 3 {
		t.Fatalf("expected three writers with works in a page of four, got %v", seen)
	}
	if len(page.Rows) != 1+2+3 {
		t.Fatalf("expected six rows for writers 1, 2 and 3, got %d", len(page.Rows))
	}
}

// A page whose input-0 rows match nothing still advances.
//
// Writer 0 has no works. A cursor taken from the *returned* rows would not
// move past it, and the caller would ask for the same page forever.
func TestAPageThatMatchesNothingStillAdvances(t *testing.T) {
	session := seedPaged(t)
	ctx := testContext(t)
	size := uint64(1)
	page, err := session.PageJoin(ctx, pagedQuery(nil, &size))
	if err != nil {
		t.Fatalf("page: %v", err)
	}
	if len(page.Rows) != 0 {
		t.Fatalf("writer 0 has no works, so the first page of one is empty: %v", page.Rows)
	}
	if page.IsLast() {
		t.Fatal("a full page — one writer read — must carry a cursor even with no rows")
	}
	if len(page.Cursor) != 2 {
		t.Fatalf("the cursor is writers' whole two-column key, got %v", page.Cursor)
	}
}

// A right outer join is refused, by name, on its first page.
func TestARightOuterJoinRefusesACursor(t *testing.T) {
	session := seedPaged(t)
	ctx := testContext(t)
	join := slate.NewJoin()
	writersAt := join.Add(slate.JoinInput{Table: "writers"})
	join.Add(slate.JoinInput{
		Table: "works",
		Type:  slate.Right,
		On:    []slate.On{{Earlier: slate.At(writersAt, 1), Own: 1}},
	})
	query := join.Query()
	size := uint64(2)
	query.Limit = &size
	if _, err := session.PageJoin(ctx, query); err == nil {
		t.Fatal("a right outer join cannot be paged by the left key")
	} else if !strings.Contains(err.Error(), "belong to no page") {
		t.Fatalf("unhelpful refusal: %v", err)
	}
}

// An offset beside a cursor is refused: two ways to say where a page starts.
func TestAnOffsetBesideACursorIsRefused(t *testing.T) {
	session := seedPaged(t)
	ctx := testContext(t)
	size := uint64(2)
	query := pagedQuery(nil, &size)
	query.Offset = 3
	if _, err := session.PageJoin(ctx, query); err == nil {
		t.Fatal("an offset and a cursor together must be refused")
	} else if !strings.Contains(err.Error(), "Drop the offset") {
		t.Fatalf("unhelpful refusal: %v", err)
	}
}

// A page with no size is the whole join, and is refused.
func TestAPageWithNoSizeIsRefused(t *testing.T) {
	session := seedPaged(t)
	ctx := testContext(t)
	if _, err := session.PageJoin(ctx, pagedQuery(nil, nil)); err == nil {
		t.Fatal("a page needs a size")
	} else if !strings.Contains(err.Error(), "a page needs a size") {
		t.Fatalf("unhelpful refusal: %v", err)
	}
}
