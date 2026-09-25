package slate_test

// `Or` and `GroupOr`, through a real node.
//
// # Why this file exists
//
// `ledger/2026-09-25-the-disjunction-the-kernel-always-had.md` taught the SQL
// front end to write `OR`, and two of its caveats said no client could send
// one and that the gRPC `Query` had no equivalent. Both were wrong:
// `Expr.disjunction` is field 9 of the wire and every client has had a builder
// for it. `ledger/2026-09-25-the-clients-could-always-send-a-disjunction.md`
// withdrew them and added the Python half of the proof.
//
// This is the Go half. It exists because a claim of that shape got written
// twice by somebody reading the front end and generalising, and the only thing
// that stops a third is something that runs.
//
// The oracle is the two arms: a disjunction returns their union and a
// conjunction their intersection, and the two must differ on this fixture or
// neither assertion says anything. Both are checked, in that order, so a
// fixture that stopped discriminating fails loudly rather than passing
// vacuously.

import (
	"sort"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// Sizes chosen so `size > 20` and `kind = "note"` overlap on exactly one row
// and each admits one the other does not. A fixture where one arm contained
// the other would make a union and a conjunction agree.
var disjoined = [][3]any{
	{uint64(1), "note", int64(10)},  // neither
	{uint64(2), "note", int64(30)},  // both
	{uint64(3), "memo", int64(40)},  // size only
	{uint64(4), "note", int64(5)},   // kind only
	{uint64(5), "memo", int64(15)},  // neither
	{uint64(6), "sheet", int64(50)}, // size only
}

func disjunctive(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, "").client(t).Session()
	rows := make([][]slate.Value, 0, len(disjoined))
	for _, row := range disjoined {
		rows = append(rows, []slate.Value{
			slate.Uint(row[0].(uint64)),
			slate.String(row[1].(string)),
			slate.Int(row[2].(int64)),
		})
	}
	if _, err := session.Insert(testContext(t), "docs", rows...); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	return session
}

// matching returns the ids the filter admits, sorted.
func matching(t *testing.T, session *slate.Session, filter slate.Expr) []uint64 {
	t.Helper()
	stream, err := session.Query(testContext(t), slate.Query{
		Table:  "docs",
		Filter: slate.Filter(filter),
		Sort:   []slate.SortKey{{Column: 0}},
	})
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	defer stream.Close()
	found := []uint64{}
	for stream.Next() {
		id, ok := stream.Row()[0].(slate.Uint)
		if !ok {
			t.Fatalf("id came back as %T", stream.Row()[0])
		}
		found = append(found, uint64(id))
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("draining: %v", err)
	}
	return found
}

func union(a, b []uint64) []uint64 {
	seen := map[uint64]bool{}
	for _, v := range append(append([]uint64{}, a...), b...) {
		seen[v] = true
	}
	out := make([]uint64, 0, len(seen))
	for v := range seen {
		out = append(out, v)
	}
	sort.Slice(out, func(i, j int) bool { return out[i] < out[j] })
	return out
}

func intersection(a, b []uint64) []uint64 {
	in := map[uint64]bool{}
	for _, v := range a {
		in[v] = true
	}
	out := []uint64{}
	for _, v := range b {
		if in[v] {
			out = append(out, v)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i] < out[j] })
	return out
}

func TestAClientSendsADisjunctionInAFilter(t *testing.T) {
	session := disjunctive(t)

	big := matching(t, session, slate.Gt(2, slate.Int(20)))
	named := matching(t, session, slate.Eq(1, slate.String("note")))

	// The arms must discriminate, or a server that ANDed them would satisfy
	// every assertion below.
	if len(big) == 0 || len(named) == 0 {
		t.Fatalf("an empty arm proves nothing: big=%v named=%v", big, named)
	}
	if sameIDs(big, named) {
		t.Fatalf("the arms admit the same rows (%v), so union and intersection agree", big)
	}

	both := matching(t, session, slate.Or(
		slate.Gt(2, slate.Int(20)),
		slate.Eq(1, slate.String("note")),
	))
	if want := union(big, named); !sameIDs(both, want) {
		t.Fatalf("Or admitted %v, want the union %v", both, want)
	}

	// And the conjunction really is smaller, which is what makes the line
	// above a test of the connective rather than of the fixture.
	conjoined := matching(t, session, slate.And(
		slate.Gt(2, slate.Int(20)),
		slate.Eq(1, slate.String("note")),
	))
	if want := intersection(big, named); !sameIDs(conjoined, want) {
		t.Fatalf("And admitted %v, want the intersection %v", conjoined, want)
	}
	if sameIDs(conjoined, both) {
		t.Fatalf("And and Or admitted the same rows (%v)", both)
	}
}

func TestOrWithNoPartsIsFalseAndAdmitsNothing(t *testing.T) {
	// The documented identity. `And()` is `True` and `Or()` is `False`, which
	// is what keeps a filter built up in a loop from flipping meaning when the
	// loop adds nothing. A server that treated an empty disjunction as `True`
	// would return the whole table, which is the dangerous direction.
	session := disjunctive(t)
	if found := matching(t, session, slate.Or()); len(found) != 0 {
		t.Fatalf("an empty Or admitted %v, want nothing", found)
	}
	if found := matching(t, session, slate.And()); len(found) != len(disjoined) {
		t.Fatalf("an empty And admitted %v, want all %d rows", found, len(disjoined))
	}
}

// groupKinds returns the `kind` of every group a HAVING keeps, sorted.
func groupKinds(t *testing.T, session *slate.Session, having *slate.Expr) []string {
	t.Helper()
	stream, err := session.Aggregate(testContext(t),
		slate.Query{Table: "docs"},
		slate.Grouping{
			GroupBy:    []slate.Column{slate.Key0(1)},
			Aggregates: []slate.Aggregate{slate.Count()},
			Having:     having,
		})
	if err != nil {
		t.Fatalf("aggregating: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	kinds := make([]string, 0, len(groups))
	for _, group := range groups {
		kind, ok := group.Key[0].(slate.String)
		if !ok {
			t.Fatalf("the key came back as %T", group.Key[0])
		}
		kinds = append(kinds, string(kind))
	}
	sort.Strings(kinds)
	return kinds
}

func sameKinds(a, b []string) bool {
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

func TestAClientSendsADisjunctionInAHaving(t *testing.T) {
	// The counts are note=3, memo=2, sheet=1. `> 2` keeps note and `< 2` keeps
	// sheet, so the union leaves memo out — without which this would pass
	// against a server that ignored HAVING entirely.
	session := disjunctive(t)

	everything := groupKinds(t, session, nil)
	if want := []string{"memo", "note", "sheet"}; !sameKinds(everything, want) {
		t.Fatalf("ungrouped kinds are %v, want %v", everything, want)
	}

	many := groupKinds(t, session, slate.Filter(slate.GroupGt(slate.Agg(0), slate.Uint(2))))
	few := groupKinds(t, session, slate.Filter(slate.GroupLt(slate.Agg(0), slate.Uint(2))))
	both := groupKinds(t, session, slate.Filter(slate.Or(
		slate.GroupGt(slate.Agg(0), slate.Uint(2)),
		slate.GroupLt(slate.Agg(0), slate.Uint(2)),
	)))

	if want := []string{"note"}; !sameKinds(many, want) {
		t.Fatalf("count > 2 kept %v, want %v", many, want)
	}
	if want := []string{"sheet"}; !sameKinds(few, want) {
		t.Fatalf("count < 2 kept %v, want %v", few, want)
	}
	if want := []string{"note", "sheet"}; !sameKinds(both, want) {
		t.Fatalf("the ORed HAVING kept %v, want %v", both, want)
	}
	if sameKinds(both, everything) {
		t.Fatalf("the ORed HAVING kept every group, so it filtered nothing")
	}
}
