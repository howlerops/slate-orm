package slate_test

// A decimal column, and the conditional update it exists to protect.
//
// Both were built in the kernel and reachable only from Rust until the
// protocol grew `Value.decimal_value` and `UpdateRequest.expected`.
//
// A decimal here is a count of the column's smallest unit and nothing else.
// [slate.Units](1250) in a column declared scale 2 is 12.50, and the identical
// value in a scale-0 column is 1250 — nothing on the wire says which, because
// the protocol publishes no schema. So these tests are written to fail if a
// decimal ever arrives as a [slate.Int]: that is the confusion that loses two
// decimal places with no error anywhere.

import (
	"errors"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

const decimalTables = `
[[tables]]
name = "prices"
id = 50
columns = [
  { name = "id",     type = "u64" },
  { name = "label",  type = "str" },
  { name = "amount", type = "decimal", scale = 2 },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["prices"]
actions = ["everything"]
`

// priceScale is the scale `prices.amount` is declared with, restated here
// because the protocol publishes no schema and a client's idea of a scale is a
// local declaration. It is exactly the hole that makes `StringWithScale` take
// the scale as an argument.
const priceScale = 2

func priceRow(id uint64, label string, units int64) []slate.Value {
	return []slate.Value{slate.Uint(id), slate.String(label), slate.Units(units)}
}

// priced starts a node holding three prices: tea 2.50, coffee 12.50, free 0.
func priced(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, decimalTables).client(t).Session()
	if _, err := session.Insert(testContext(t), "prices",
		priceRow(1, "tea", 250),
		priceRow(2, "coffee", 1250),
		priceRow(3, "free", 0),
	); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	return session
}

func amountOf(t *testing.T, session *slate.Session, id uint64) slate.Units {
	t.Helper()
	row, found, err := session.Get(testContext(t), "prices", []slate.Value{slate.Uint(id)})
	if err != nil {
		t.Fatalf("getting %d: %v", id, err)
	}
	if !found {
		t.Fatalf("row %d is not there", id)
	}
	units, ok := row[2].(slate.Units)
	if !ok {
		// Not a `t.Errorf` and a continue: reading a decimal as an `Int` is
		// the whole failure this file is about, and every assertion after it
		// would be comparing the wrong type.
		t.Fatalf("a decimal came back as %T, not slate.Units", row[2])
	}
	return units
}

func TestADecimalRoundTripsThroughTheServer(t *testing.T) {
	session := priced(t)
	if got := amountOf(t, session, 2); got != 1250 {
		t.Fatalf("amount = %d, want 1250", got)
	}
	if got := amountOf(t, session, 2).StringWithScale(priceScale); got != "12.50" {
		t.Fatalf("rendered %q, want \"12.50\"", got)
	}
}

func TestANegativeDecimalSurvives(t *testing.T) {
	// A refund is the value most likely to be mangled by an encoding that
	// assumed a price is positive.
	session := priced(t)
	if _, err := session.Insert(testContext(t), "prices", priceRow(9, "refund", -75)); err != nil {
		t.Fatalf("inserting: %v", err)
	}
	if got := amountOf(t, session, 9); got != -75 {
		t.Fatalf("amount = %d, want -75", got)
	}
}

func TestAnIntegerInADecimalColumnIsRefused(t *testing.T) {
	// `slate.Int`, not `slate.Units` — the mistake a caller who did not notice
	// the new type would make. The kernel orders values type first, so storing
	// it would put a value in the column that does not sort with its
	// neighbours.
	session := priced(t)
	_, err := session.Insert(testContext(t), "prices",
		[]slate.Value{slate.Uint(8), slate.String("wrong"), slate.Int(100)})
	if err == nil {
		t.Fatal("an i64 in a decimal column was accepted")
	}
}

func TestUnitsRenderAgainstAScale(t *testing.T) {
	// The table the Python and TypeScript clients render identically; a
	// difference between the three is a bug in one of them rather than a
	// dialect.
	for _, c := range []struct {
		units  slate.Units
		scale  int
		expect string
	}{
		{1250, 2, "12.50"},
		{250, 2, "2.50"},
		{0, 2, "0.00"},
		{-75, 2, "-0.75"},
		{5, 3, "0.005"},
		{1250, 0, "1250"},
		// math.MinInt64, whose magnitude does not fit in an int64 at all, so
		// the renderer has to build it in a uint64. It catches a renderer that
		// tried to stay in int64; it does *not* distinguish -uint64(v) from
		// uint64(-v), which Go makes identical for every input.
		{-9223372036854775808, 2, "-92233720368547758.08"},
		// A negative scale is a caller's mistake in a render path, and a
		// render path is the worst place to panic. It reads as 0.
		{1250, -1, "1250"},
	} {
		if got := c.units.StringWithScale(c.scale); got != c.expect {
			t.Errorf("Units(%d).StringWithScale(%d) = %q, want %q",
				c.units, c.scale, got, c.expect)
		}
	}
}

func TestAnUpdateNamingTheRowItReadIsApplied(t *testing.T) {
	session := priced(t)
	result, err := session.UpdateIfUnchanged(testContext(t), "prices", slate.RowUpdate{
		Row: priceRow(2, "coffee", 1400),
		Was: priceRow(2, "coffee", 1250),
	})
	if err != nil {
		t.Fatalf("the row is unchanged: %v", err)
	}
	if result.Affected != 1 {
		t.Fatalf("affected = %d, want 1", result.Affected)
	}
	if got := amountOf(t, session, 2); got != 1400 {
		t.Fatalf("amount = %d, want 1400", got)
	}
}

func TestAnUpdateNamingARowThatMovedIsRefused(t *testing.T) {
	session := priced(t)
	// Somebody else's write lands between this caller's read and its write.
	if _, err := session.Update(testContext(t), "prices", priceRow(2, "coffee", 1300)); err != nil {
		t.Fatalf("the unconditional update: %v", err)
	}

	_, err := session.UpdateIfUnchanged(testContext(t), "prices", slate.RowUpdate{
		Row: priceRow(2, "coffee", 1400),
		Was: priceRow(2, "coffee", 1250),
	})
	if err == nil {
		t.Fatal("a stale conditional update was accepted")
	}
	var refused *slate.Error
	if !errors.As(err, &refused) {
		t.Fatalf("a refusal arrived as %T: %v", err, err)
	}
	// And the refusal is total: the write it guarded did not land.
	if got := amountOf(t, session, 2); got != 1300 {
		t.Fatalf("amount = %d, want 1300 — a refused update wrote anyway", got)
	}
}

func TestAStaleFirstRowRefusesTheRest(t *testing.T) {
	// The order that a loop which does not stop at the first refusal gets
	// wrong: it would report the *last* row's outcome, so a stale first row
	// and a current second row would answer with a success. That is a lost
	// update reported as a successful conditional update.
	session := priced(t)
	if _, err := session.Update(testContext(t), "prices", priceRow(1, "tea", 251)); err != nil {
		t.Fatalf("moving row 1: %v", err)
	}

	_, err := session.UpdateIfUnchanged(testContext(t), "prices",
		slate.RowUpdate{Row: priceRow(1, "tea", 300), Was: priceRow(1, "tea", 250)},
		slate.RowUpdate{Row: priceRow(2, "coffee", 1400), Was: priceRow(2, "coffee", 1250)},
	)
	if err == nil {
		t.Fatal("a stale first row was accepted")
	}
	if got := amountOf(t, session, 2); got != 1250 {
		t.Fatalf("amount = %d, want 1250 — the second row was written anyway", got)
	}
}

func TestAConditionalUpdateInsideATransaction(t *testing.T) {
	// The session path is different code from the autocommit one, and a
	// feature wired into one and not the other is this repository's recurring
	// shape.
	session := priced(t)
	ctx := testContext(t)
	txn, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	if _, err := txn.UpdateIfUnchanged(ctx, "prices", slate.RowUpdate{
		Row: priceRow(2, "coffee", 1500),
		Was: priceRow(2, "coffee", 1250),
	}); err != nil {
		t.Fatalf("inside the transaction: %v", err)
	}
	if err := txn.Commit(ctx); err != nil {
		t.Fatalf("commit: %v", err)
	}
	if got := amountOf(t, session, 2); got != 1500 {
		t.Fatalf("amount = %d, want 1500", got)
	}
}

func TestAStaleConditionalUpdateInsideATransactionIsRefused(t *testing.T) {
	// Written because a mutation survived without it. The happy-path
	// transaction test above passes whether or not the transaction's
	// `UpdateIfUnchanged` actually sends its expected rows — an unconditional
	// update of an unchanged row produces the same outcome. Only a *stale* row
	// tells the two apart, and the transaction path is separate code from the
	// autocommit one on both sides of the wire.
	session := priced(t)
	ctx := testContext(t)
	if _, err := session.Update(ctx, "prices", priceRow(2, "coffee", 1300)); err != nil {
		t.Fatalf("moving the row: %v", err)
	}

	txn, err := session.Begin(ctx)
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	if _, err := txn.UpdateIfUnchanged(ctx, "prices", slate.RowUpdate{
		Row: priceRow(2, "coffee", 1400),
		Was: priceRow(2, "coffee", 1250),
	}); err == nil {
		t.Fatal("a stale conditional update inside a transaction was accepted")
	}
	_ = txn.Rollback(ctx)
	if got := amountOf(t, session, 2); got != 1300 {
		t.Fatalf("amount = %d, want 1300", got)
	}
}

func TestNoRowUpdatesIsAnOrdinaryUpdateOfNothing(t *testing.T) {
	// The degenerate case, which is worth pinning because `expected` is
	// `repeated` on the wire and an empty one means "no condition": a call
	// with no pairs must not become an unconditional update of every row.
	session := priced(t)
	result, err := session.UpdateIfUnchanged(testContext(t), "prices")
	if err != nil {
		t.Fatalf("no rows: %v", err)
	}
	if result.Affected != 0 {
		t.Fatalf("affected = %d, want 0", result.Affected)
	}
	if got := amountOf(t, session, 2); got != 1250 {
		t.Fatalf("amount = %d, want 1250", got)
	}
}
