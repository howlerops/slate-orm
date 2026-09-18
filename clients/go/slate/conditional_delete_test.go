package slate_test

// A conditional delete.
//
// UpdateIfUnchanged shipped first because overwriting somebody else's edit is
// the loss everybody recognises. Deleting a row somebody else just edited is
// the same mistake: the caller read the row, decided *from what it said* that
// it should go, and by the time the delete lands it says something else.
//
// The one place this is not simply the update's twin: a plain delete reports
// an absent key in WriteResult.Affected, and a conditional one is refused with
// codes.NotFound. The caller said what it expected to find, so "it was already
// gone" is an answer it wants rather than a smaller count it will read as
// success.

import (
	"errors"
	"testing"

	"google.golang.org/grpc/codes"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

const conditionalTables = `
[[tables]]
name = "notes"
id = 60
columns = [
  { name = "id",   type = "u64" },
  { name = "body", type = "str" },
  { name = "size", type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["notes"]
actions = ["everything"]
`

func noteRow(id uint64, size int64) []slate.Value {
	return []slate.Value{slate.Uint(id), slate.String("note"), slate.Int(size)}
}

func noteKey(id uint64) []slate.Value {
	return []slate.Value{slate.Uint(id)}
}

// notesSeeded starts a node holding four notes, ids 0..3, size = id.
func notesSeeded(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, conditionalTables).client(t).Session()
	rows := make([][]slate.Value, 0, 4)
	for id := uint64(0); id < 4; id++ {
		rows = append(rows, noteRow(id, int64(id)))
	}
	if _, err := session.Insert(testContext(t), "notes", rows...); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	return session
}

func notePresent(t *testing.T, session *slate.Session, id uint64) bool {
	t.Helper()
	_, found, err := session.Get(testContext(t), "notes", noteKey(id))
	if err != nil {
		t.Fatalf("getting %d: %v", id, err)
	}
	return found
}

func TestADeleteNamingTheRowItReadIsApplied(t *testing.T) {
	session := notesSeeded(t)
	result, err := session.DeleteIfUnchanged(testContext(t), "notes", slate.RowDelete{
		Key: noteKey(2), Was: noteRow(2, 2),
	})
	if err != nil {
		t.Fatalf("the row is unchanged: %v", err)
	}
	if result.Affected != 1 {
		t.Fatalf("affected = %d, want 1", result.Affected)
	}
	if notePresent(t, session, 2) {
		t.Fatal("the row is still there")
	}
}

func TestADeleteNamingARowThatMovedIsRefused(t *testing.T) {
	session := notesSeeded(t)
	if _, err := session.Update(testContext(t), "notes", noteRow(2, 5000)); err != nil {
		t.Fatalf("the other writer's edit: %v", err)
	}

	_, err := session.DeleteIfUnchanged(testContext(t), "notes", slate.RowDelete{
		Key: noteKey(2), Was: noteRow(2, 2),
	})
	if err == nil {
		t.Fatal("a stale conditional delete was accepted")
	}
	if !notePresent(t, session, 2) {
		t.Fatal("a refused conditional delete removed the row anyway")
	}
}

func TestARowAlreadyGoneIsRefusedRatherThanCountedAsAbsent(t *testing.T) {
	// Both halves, because the refusal only means something beside the zero it
	// replaces.
	session := notesSeeded(t)
	ctx := testContext(t)
	if _, err := session.Delete(ctx, "notes", noteKey(3)); err != nil {
		t.Fatalf("the first delete: %v", err)
	}

	plain, err := session.Delete(ctx, "notes", noteKey(3))
	if err != nil {
		t.Fatalf("a plain delete of an absent key is not an error: %v", err)
	}
	if plain.Affected != 0 {
		t.Fatalf("affected = %d, want 0", plain.Affected)
	}

	_, err = session.DeleteIfUnchanged(ctx, "notes", slate.RowDelete{
		Key: noteKey(3), Was: noteRow(3, 3),
	})
	var refused *slate.Error
	if !errors.As(err, &refused) {
		t.Fatalf("expected a slate error, got %T: %v", err, err)
	}
	// NotFound and not Aborted: a row that moved can be re-read and the
	// decision remade, and a row that is gone cannot, so a caller retrying an
	// Aborted would loop.
	if refused.Code != codes.NotFound {
		t.Fatalf("code = %v, want NotFound (%q)", refused.Code, refused.Reason)
	}
}

func TestAStaleFirstKeyRefusesTheWholeStatement(t *testing.T) {
	session := notesSeeded(t)
	if _, err := session.Update(testContext(t), "notes", noteRow(0, 99)); err != nil {
		t.Fatalf("moving row 0: %v", err)
	}

	// The stale key first, so a loop reporting the last outcome would answer
	// with a success.
	_, err := session.DeleteIfUnchanged(testContext(t), "notes",
		slate.RowDelete{Key: noteKey(0), Was: noteRow(0, 0)},
		slate.RowDelete{Key: noteKey(1), Was: noteRow(1, 1)},
	)
	if err == nil {
		t.Fatal("a stale first key was accepted")
	}
	if !notePresent(t, session, 1) {
		t.Fatal("the second row was deleted by a refused statement")
	}
}

func TestAStaleConditionalDeleteInsideATransactionIsRefused(t *testing.T) {
	// The session path is separate code from the autocommit one, and only a
	// stale row tells a conditional delete apart from a plain one.
	session := notesSeeded(t)
	ctx := testContext(t)
	if _, err := session.Update(ctx, "notes", noteRow(2, 77)); err != nil {
		t.Fatalf("moving the row: %v", err)
	}

	txn, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	if _, err := txn.DeleteIfUnchanged(ctx, "notes", slate.RowDelete{
		Key: noteKey(2), Was: noteRow(2, 2),
	}); err == nil {
		t.Fatal("a stale conditional delete inside a transaction was accepted")
	}
	_ = txn.Rollback(ctx)
	if !notePresent(t, session, 2) {
		t.Fatal("the row is gone")
	}
}

func TestNoRowDeletesIsADeleteOfNothing(t *testing.T) {
	session := notesSeeded(t)
	result, err := session.DeleteIfUnchanged(testContext(t), "notes")
	if err != nil {
		t.Fatalf("no keys: %v", err)
	}
	if result.Affected != 0 {
		t.Fatalf("affected = %d, want 0", result.Affected)
	}
	if !notePresent(t, session, 2) {
		t.Fatal("a conditional delete with no pairs removed a row")
	}
}
