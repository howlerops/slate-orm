package slate_test

import (
	"errors"
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

const batchTables = `
[[tables]]
name = "notes"
id = 42
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

func note(id uint64) []slate.Value {
	return []slate.Value{slate.Uint(id), slate.String("note")}
}

func batched(t *testing.T) *slate.Session {
	t.Helper()
	return start(t, batchTables).client(t).Session()
}

// present is every id in `notes`.
func present(t *testing.T, session *slate.Session) []uint64 {
	t.Helper()
	stream, err := session.Query(testContext(t), slate.Query{Table: "notes"})
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	defer stream.Close()
	out := []uint64{}
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

func TestBatchRefusesAnUnsetAtomicity(t *testing.T) {
	session := batched(t)
	// The zero value, which is what a caller who never chose would have.
	b := slate.NewBatch(slate.AtomicityUnset).Insert("notes", note(1))
	_, err := session.Batch(testContext(t), b)
	if err == nil {
		t.Fatal("a batch with no atomicity should be refused")
	}
	// The message must name both, because the caller's next move is to pick.
	for _, want := range []string{"Independent", "AllOrNothing"} {
		if !strings.Contains(err.Error(), want) {
			t.Fatalf("the refusal should name %q: %v", want, err)
		}
	}
	if got := present(t, session); len(got) != 0 {
		t.Fatalf("%d rows written on the way to being refused", len(got))
	}
}

func TestBatchRefusesAnEmptyList(t *testing.T) {
	session := batched(t)
	_, err := session.Batch(testContext(t), slate.NewBatch(slate.Independent))
	if err == nil {
		t.Fatal("an empty batch should be refused")
	}
}

func TestIndependentBatchAppliesEveryOperation(t *testing.T) {
	session := batched(t)
	b := slate.NewBatch(slate.Independent)
	for id := uint64(1); id <= 5; id++ {
		b.Insert("notes", note(id))
	}
	result, err := session.Batch(testContext(t), b)
	if err != nil {
		t.Fatalf("batching: %v", err)
	}
	if len(result.Outcomes) != 5 {
		t.Fatalf("%d outcomes, want one per operation", len(result.Outcomes))
	}
	for at, one := range result.Outcomes {
		if !one.OK() {
			t.Fatalf("operation %d failed: %v", at, one.Err)
		}
		if one.Written.Affected != 1 {
			t.Fatalf("operation %d affected %d, want 1", at, one.Written.Affected)
		}
	}
	if result.Sequence == nil {
		t.Fatal("the last commit's sequence should come back")
	}
	if got := len(present(t, session)); got != 5 {
		t.Fatalf("%d rows, want 5", got)
	}
}

// The difference, half one: a failure is a result, and the rest still land.
func TestIndependentBatchCarriesOnPastAFailure(t *testing.T) {
	session := batched(t)
	if _, err := session.Insert(testContext(t), "notes", note(2)); err != nil {
		t.Fatalf("seeding: %v", err)
	}

	b := slate.NewBatch(slate.Independent)
	b.Insert("notes", note(1))
	b.Insert("notes", note(2)) // already there
	b.Insert("notes", note(3))
	result, err := session.Batch(testContext(t), b)
	if err != nil {
		t.Fatalf("the batch itself should succeed: %v", err)
	}

	if result.Outcomes[0].OK() != true || result.Outcomes[2].OK() != true {
		t.Fatalf("the operations beside the failure should have landed: %+v", result.Outcomes)
	}
	failures := result.Failures()
	if len(failures) != 1 {
		t.Fatalf("%d failures, want 1", len(failures))
	}
	var failed *slate.Error
	if !errors.As(failures[0].Err, &failed) {
		t.Fatalf("a batch failure should be a *slate.Error: %v", failures[0].Err)
	}
	if failed.Kind != slate.KindAlreadyExists {
		t.Fatalf("kind = %v, want already-exists", failed.Kind)
	}
	// The stable token survives the trip through a message body, which is the
	// half a lone call gets from its trailers.
	if failed.Reason != "DUPLICATE_PRIMARY_KEY" {
		t.Fatalf("reason = %q, want DUPLICATE_PRIMARY_KEY", failed.Reason)
	}

	if got := len(present(t, session)); got != 3 {
		t.Fatalf("%d rows, want 3 — 1 and 3 land although 2 failed between them", got)
	}
}

// The difference, half two: the same three operations, and nothing lands.
func TestAtomicBatchUndoesEverythingBeforeTheFailure(t *testing.T) {
	session := batched(t)
	if _, err := session.Insert(testContext(t), "notes", note(2)); err != nil {
		t.Fatalf("seeding: %v", err)
	}

	b := slate.NewBatch(slate.AllOrNothing)
	b.Insert("notes", note(1))
	b.Insert("notes", note(2)) // already there
	b.Insert("notes", note(3))
	if _, err := session.Batch(testContext(t), b); err == nil {
		t.Fatal("an atomic batch should fail the call")
	}

	if got := present(t, session); len(got) != 1 || got[0] != 2 {
		t.Fatalf("rows = %v, want only the seeded 2", got)
	}
}

func TestAtomicBatchReportsNoPerOperationOutcomes(t *testing.T) {
	session := batched(t)
	b := slate.NewBatch(slate.AllOrNothing)
	for id := uint64(1); id <= 3; id++ {
		b.Insert("notes", note(id))
	}
	result, err := session.Batch(testContext(t), b)
	if err != nil {
		t.Fatalf("batching: %v", err)
	}
	if len(result.Outcomes) != 0 {
		t.Fatalf("%d outcomes; they all happened, so there is nothing to report", len(result.Outcomes))
	}
	if got := len(present(t, session)); got != 3 {
		t.Fatalf("%d rows, want 3", got)
	}
}

func TestBatchCarriesEveryKindOfWrite(t *testing.T) {
	session := batched(t)
	ctx := testContext(t)
	rows := make([][]slate.Value, 0, 6)
	for id := uint64(1); id <= 6; id++ {
		rows = append(rows, note(id))
	}
	if _, err := session.Insert(ctx, "notes", rows...); err != nil {
		t.Fatalf("seeding: %v", err)
	}

	b := slate.NewBatch(slate.Independent)
	b.Insert("notes", note(7))
	b.Upsert("notes", []slate.Value{slate.Uint(7), slate.String("replaced")})
	b.Delete("notes", []slate.Value{slate.Uint(1)})
	b.DeleteWhere(slate.DeleteWhere{
		Table:     "notes",
		Filter:    slate.Filter(slate.Ge(0, slate.Uint(6))),
		Returning: true,
	})
	b.UpdateWhere(slate.UpdateWhere{
		Table: "notes",
		Set:   []slate.Assignment{slate.Assign(1, slate.Lit(slate.String("touched")))},
	})
	result, err := session.Batch(ctx, b)
	if err != nil {
		t.Fatalf("batching: %v", err)
	}
	for at, one := range result.Outcomes {
		if !one.OK() {
			t.Fatalf("operation %d failed: %v", at, one.Err)
		}
	}
	// The delete_where asked for its rows: 6 and 7.
	if got := len(result.Outcomes[3].Written.Rows); got != 2 {
		t.Fatalf("%d returned rows, want 2 — returning should ride through a batch", got)
	}
	// The update_where did not ask, so it returns none.
	if got := len(result.Outcomes[4].Written.Rows); got != 0 {
		t.Fatalf("%d returned rows, want none", got)
	}
	if got := len(present(t, session)); got != 4 {
		t.Fatalf("%d rows, want 4", got)
	}
}

func TestAtomicBatchJoinsAnOpenTransaction(t *testing.T) {
	session := batched(t)
	ctx := testContext(t)
	tx, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	b := slate.NewBatch(slate.AllOrNothing)
	for id := uint64(1); id <= 3; id++ {
		b.Insert("notes", note(id))
	}
	result, err := tx.Batch(ctx, b)
	if err != nil {
		t.Fatalf("batching inside a transaction: %v", err)
	}
	// No sequence: the transaction has not committed, and the batch is not the
	// thing that commits it.
	if result.Sequence != nil {
		t.Fatalf("sequence = %v, want none until commit", *result.Sequence)
	}
	if err := tx.Rollback(ctx); err != nil {
		t.Fatalf("rollback: %v", err)
	}
	if got := present(t, session); len(got) != 0 {
		t.Fatalf("%d rows after the rollback, want none", len(got))
	}
}

func TestIndependentBatchMayNotRunInsideATransaction(t *testing.T) {
	session := batched(t)
	ctx := testContext(t)
	tx, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	defer func() { _ = tx.Rollback(ctx) }()

	b := slate.NewBatch(slate.Independent).Insert("notes", note(1))
	if _, err := tx.Batch(ctx, b); err == nil {
		t.Fatal(`"independent operations that roll back together" should be refused`)
	}
}

// TestTheSchemaClaimRidesOnABatchedWrite: a misdeclared table is refused, and
// nothing lands.
//
// Found by a surviving mutation — dropping the claim from every operation
// changed no answer — and the same mutation survived in all three clients,
// which makes it a blind spot rather than an oversight. It matters as much
// here as anywhere: a batched insert whose claim is dropped is a write the
// server cannot check the shape of.
func TestTheSchemaClaimRidesOnABatchedWrite(t *testing.T) {
	server := start(t, batchTables)
	// `body` misdeclared as `text`: the same width and types, a different
	// name, which is exactly the drift a width check would miss.
	swapped := slate.TableDef{
		Name: "notes",
		Columns: []slate.ColumnDef{
			{Name: "id", Type: slate.TypeUint},
			{Name: "text", Type: slate.TypeString},
		},
		PrimaryKey: []string{"id"},
	}
	session := server.client(t).Declaring(slate.Schemas{"notes": swapped}).Session()

	b := slate.NewBatch(slate.Independent).Insert("notes", note(1))
	if _, err := session.Batch(testContext(t), b); !slate.IsKind(
		err, slate.KindInvalidRequest,
	) {
		t.Fatalf("a batch against a misdeclared table: err = %v, want KindInvalidRequest", err)
	}

	// And nothing was written on the way to being refused. Read through a
	// session that declares nothing, so the check above is not what hides it.
	plain := server.client(t).Session()
	if got := present(t, plain); len(got) != 0 {
		t.Fatalf("%d rows written by a refused batch", len(got))
	}
}
