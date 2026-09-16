package slate_test

import (
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

const predicateTables = `
[[tables]]
name = "papers"
id = 40
columns = [
  { name = "id",   type = "u64" },
  { name = "kind", type = "str" },
  { name = "size", type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["papers"]
actions = ["everything"]
`

const (
	paperID    slate.Ordinal = 0
	paperKind  slate.Ordinal = 1
	paperSize  slate.Ordinal = 2
	paperCount               = 12
)

// seeded starts a node holding twelve papers: ids 0..11, size = id, and
// kind-0..kind-3 round-robin.
func seeded(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, predicateTables).client(t).Session()
	ctx := testContext(t)
	rows := make([][]slate.Value, 0, paperCount)
	for id := uint64(0); id < paperCount; id++ {
		rows = append(rows, []slate.Value{
			slate.Uint(id),
			slate.String("kind-" + string(rune('0'+id%4))),
			slate.Int(int64(id)),
		})
	}
	if _, err := session.Insert(ctx, "papers", rows...); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	return session
}

// remaining is every id still in the table.
func remaining(t *testing.T, session *slate.Session) []uint64 {
	t.Helper()
	stream, err := session.Query(testContext(t), slate.Query{Table: "papers"})
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	defer stream.Close()
	out := make([]uint64, 0, paperCount)
	// Row() advances the cursor; Next() alone only reports that one is there,
	// so a loop that never reads spins forever. The idiom is load-bearing.
	for stream.Next() {
		row := stream.Row()
		id, ok := row[0].(slate.Uint)
		if !ok {
			t.Fatalf("the first column was not a u64: %v", row[0])
		}
		out = append(out, uint64(id))
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("stream: %v", err)
	}
	return out
}

func returnedIDs(t *testing.T, result slate.WriteResult) []uint64 {
	t.Helper()
	out := make([]uint64, 0, len(result.Rows))
	for _, row := range result.Rows {
		id, ok := row[0].(slate.Uint)
		if !ok {
			t.Fatalf("the first column was not a u64: %v", row[0])
		}
		out = append(out, uint64(id))
	}
	return out
}

func TestDeleteWhereRemovesEveryMatchingRow(t *testing.T) {
	session := seeded(t)
	result, err := session.DeleteWhere(testContext(t), slate.DeleteWhere{
		Table: "papers", Filter: slate.Filter(slate.Ge(paperSize, slate.Int(9))),
	})
	if err != nil {
		t.Fatalf("deleting: %v", err)
	}
	if result.Affected != 3 {
		t.Fatalf("affected = %d, want 3", result.Affected)
	}
	if len(result.Rows) != 0 {
		t.Fatalf("no rows were asked for, got %d", len(result.Rows))
	}
	if got := len(remaining(t, session)); got != 9 {
		t.Fatalf("%d rows left, want 9", got)
	}
}

// TestDeleteWhereReturnsWhatItDestroyed is the one that could not be written
// any other way: after the delete the rows are gone, so this response is the
// only record of what they were.
func TestDeleteWhereReturnsWhatItDestroyed(t *testing.T) {
	session := seeded(t)
	result, err := session.DeleteWhere(testContext(t), slate.DeleteWhere{
		Table:     "papers",
		Filter:    slate.Filter(slate.Ge(paperSize, slate.Int(9))),
		Returning: true,
	})
	if err != nil {
		t.Fatalf("deleting: %v", err)
	}
	got := returnedIDs(t, result)
	if len(got) != 3 || got[0] != 9 || got[1] != 10 || got[2] != 11 {
		t.Fatalf("returned %v, want [9 10 11]", got)
	}
	// The sizes came back too, which a caller holding only a predicate could
	// not have reconstructed once the rows were gone.
	for at, row := range result.Rows {
		size, ok := row[2].(slate.Int)
		if !ok || int64(size) != int64(9+at) {
			t.Fatalf("row %d size = %v, want %d", at, row[2], 9+at)
		}
	}
}

func TestUpdateWhereReturnsRowsAsWritten(t *testing.T) {
	session := seeded(t)
	result, err := session.UpdateWhere(testContext(t), slate.UpdateWhere{
		Table:  "papers",
		Filter: slate.Filter(slate.Lt(paperSize, slate.Int(3))),
		// size = size + 100: one write, not a read, a decision and a write.
		Set:       []slate.Assignment{slate.Assign(paperSize, slate.Add(slate.Col(paperSize), slate.Lit(slate.Int(100))))},
		Returning: true,
	})
	if err != nil {
		t.Fatalf("updating: %v", err)
	}
	if result.Affected != 3 {
		t.Fatalf("affected = %d, want 3", result.Affected)
	}
	for at, row := range result.Rows {
		size, ok := row[2].(slate.Int)
		if !ok || int64(size) != int64(100+at) {
			t.Fatalf("row %d size = %v, want %d — the rows come back as written", at, row[2], 100+at)
		}
	}
}

// TestUpdateWhereAppliesAssignmentsTogether pins that every assignment reads
// the original row, so one cannot see another's result.
func TestUpdateWhereAppliesAssignmentsTogether(t *testing.T) {
	session := seeded(t)
	result, err := session.UpdateWhere(testContext(t), slate.UpdateWhere{
		Table:  "papers",
		Filter: slate.Filter(slate.Eq(paperID, slate.Uint(3))),
		Set: []slate.Assignment{
			slate.Assign(paperSize, slate.Add(slate.Col(paperSize), slate.Lit(slate.Int(1)))),
			slate.Assign(paperKind, slate.Col(paperKind)),
		},
		Returning: true,
	})
	if err != nil {
		t.Fatalf("updating: %v", err)
	}
	if size, ok := result.Rows[0][2].(slate.Int); !ok || int64(size) != 4 {
		t.Fatalf("size = %v, want 4", result.Rows[0][2])
	}
}

// TestDeleteWhereWithNoFilterIsEveryRow: a nil filter is `DELETE FROM docs`,
// which is a real statement and is allowed.
func TestDeleteWhereWithNoFilterIsEveryRow(t *testing.T) {
	session := seeded(t)
	result, err := session.DeleteWhere(testContext(t), slate.DeleteWhere{Table: "papers"})
	if err != nil {
		t.Fatalf("deleting: %v", err)
	}
	if result.Affected != paperCount {
		t.Fatalf("affected = %d, want %d", result.Affected, paperCount)
	}
	if got := remaining(t, session); len(got) != 0 {
		t.Fatalf("%d rows left, want none", len(got))
	}
}

func TestUpdateWhereWithNoAssignmentsIsRefused(t *testing.T) {
	session := seeded(t)
	_, err := session.UpdateWhere(testContext(t), slate.UpdateWhere{Table: "papers"})
	if err == nil {
		t.Fatal("an update with no assignments should be refused")
	}
	if !strings.Contains(err.Error(), "assignment") {
		t.Fatalf("the refusal should say what is missing: %v", err)
	}
	if got := len(remaining(t, session)); got != paperCount {
		t.Fatalf("%d rows left, want %d", got, paperCount)
	}
}

func TestUpdateWhereRefusesAColumnAssignedTwice(t *testing.T) {
	session := seeded(t)
	_, err := session.UpdateWhere(testContext(t), slate.UpdateWhere{
		Table: "papers",
		Set: []slate.Assignment{
			slate.Assign(paperSize, slate.Lit(slate.Int(1))),
			slate.Assign(paperSize, slate.Lit(slate.Int(2))),
		},
	})
	if err == nil {
		t.Fatal("a column assigned twice should be refused")
	}
}

func TestPredicateWriteMatchingNothingWritesNothing(t *testing.T) {
	session := seeded(t)
	result, err := session.DeleteWhere(testContext(t), slate.DeleteWhere{
		Table:     "papers",
		Filter:    slate.Filter(slate.Eq(paperKind, slate.String("no-such-kind"))),
		Returning: true,
	})
	if err != nil {
		t.Fatalf("deleting: %v", err)
	}
	if result.Affected != 0 || len(result.Rows) != 0 {
		t.Fatalf("affected = %d with %d rows, want 0 and none", result.Affected, len(result.Rows))
	}
	if got := len(remaining(t, session)); got != paperCount {
		t.Fatalf("%d rows left, want %d", got, paperCount)
	}
}
