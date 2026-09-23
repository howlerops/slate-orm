package slate_test

import (
	"sort"
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// Window functions from Go, against a real server.
//
// A window is not an aggregate and the difference is the cardinality: a
// grouped read returns one row per group, a window returns one value per
// *input* row. So the two things to establish are that the values arrive, and
// that they arrive in their own list — `RowStream.Windowed`, beside `Row` and
// `Computed`, rather than folded into either. A value in the wrong list is a
// value the caller reads as a different thing, which is the failure the three
// lists exist to prevent and the one that looks like working software right up
// until somebody adds a column.
//
// The numbers are checked against `Aggregate`, the same session talking to a
// different RPC that folds rows away: two operators sharing only the
// accumulator arithmetic, so agreeing is evidence rather than a restatement.

// A table with real ties in `size`, which the seeded fixtures elsewhere do not
// have. Ties are what separate a correct `RANK` from a `ROW_NUMBER` wearing its
// name, and a `RANGE` running total from a `ROWS` one.
const windowTables = `
[[tables]]
name = "readings"
id = 24
columns = [
  { name = "id",   type = "u64" },
  { name = "site", type = "str" },
  { name = "size", type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["readings"]
actions = ["everything"]
`

const (
	readingID   = slate.Ordinal(0)
	readingSite = slate.Ordinal(1)
	readingSize = slate.Ordinal(2)
)

// Two sites of four rows each, and within a site the sizes go 10, 10, 20, 30 —
// so the first two rows are peers and a rank leaves a gap after them.
var readings = []struct {
	id   uint64
	site string
	size int64
}{
	{1, "north", 10}, {2, "north", 10}, {3, "north", 20}, {4, "north", 30},
	{5, "south", 10}, {6, "south", 10}, {7, "south", 20}, {8, "south", 30},
}

func readingsSession(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, windowTables).client(t).Session()
	ctx := testContext(t)
	for _, r := range readings {
		if _, err := session.Insert(ctx, "readings", []slate.Value{
			slate.Uint(r.id),
			slate.String(r.site),
			slate.Int(r.size),
		}); err != nil {
			t.Fatalf("seeding readings: %v", err)
		}
	}
	return session
}

// windowValues drains a query, returning each row's id, site and window values
// — and asserting on the way that the stored columns are only the stored ones.
func windowValues(t *testing.T, session *slate.Session, q slate.Query) map[uint64][]slate.Value {
	t.Helper()
	stream, err := session.Query(testContext(t), q)
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	defer stream.Close()
	out := map[uint64][]slate.Value{}
	for stream.Next() {
		// Windowed first: Row advances the cursor.
		windowed := stream.Windowed()
		computed := stream.Computed()
		row := stream.Row()
		if len(row) != 3 {
			t.Fatalf("a readings row has 3 columns, got %d: %v", len(row), row)
		}
		if len(computed) != 0 {
			t.Fatalf("the query computes nothing, so %v should be empty", computed)
		}
		id, ok := row[readingID].(slate.Uint)
		if !ok {
			t.Fatalf("id is not unsigned: %v", row[readingID])
		}
		out[uint64(id)] = windowed
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("draining: %v", err)
	}
	return out
}

func asUint(t *testing.T, v slate.Value) uint64 {
	t.Helper()
	n, ok := v.(slate.Uint)
	if !ok {
		t.Fatalf("expected an unsigned integer, got %T (%v)", v, v)
	}
	return uint64(n)
}

func asInt(t *testing.T, v slate.Value) int64 {
	t.Helper()
	n, ok := v.(slate.Int)
	if !ok {
		t.Fatalf("expected a signed integer, got %T (%v)", v, v)
	}
	return int64(n)
}

// A row number restarts in each partition, and arrives in its own list.
func TestRowNumberNumbersEachPartitionFromOne(t *testing.T) {
	session := readingsSession(t)
	got := windowValues(t, session, slate.Query{
		Table: "readings",
		Window: []slate.Window{
			slate.RowNumberOver().Over(
				[]slate.Column{slate.Key0(readingSite)},
				[]slate.SortKey{{Column: readingID}},
			),
		},
	})

	bySite := map[string][]uint64{}
	for _, r := range readings {
		values := got[r.id]
		if len(values) != 1 {
			t.Fatalf("row %d carries %d window value(s), want 1", r.id, len(values))
		}
		bySite[r.site] = append(bySite[r.site], asUint(t, values[0]))
	}
	for site, numbers := range bySite {
		sort.Slice(numbers, func(i, j int) bool { return numbers[i] < numbers[j] })
		want := []uint64{1, 2, 3, 4}
		if len(numbers) != len(want) {
			t.Fatalf("%s has %d rows, want %d", site, len(numbers), len(want))
		}
		for i := range want {
			if numbers[i] != want[i] {
				t.Fatalf("%s numbered %v, want %v", site, numbers, want)
			}
		}
	}
}

// RANK leaves the gap that DENSE_RANK closes, over a column with real ties.
func TestRankLeavesTheGapDenseRankCloses(t *testing.T) {
	session := readingsSession(t)
	clause := func(w slate.Window) slate.Window {
		return w.Over(
			[]slate.Column{slate.Key0(readingSite)},
			[]slate.SortKey{{Column: readingSize}},
		)
	}
	got := windowValues(t, session, slate.Query{
		Table: "readings",
		Window: []slate.Window{
			clause(slate.RankOver()),
			clause(slate.DenseRankOver()),
		},
	})

	// north is 10, 10, 20, 30: ranks 1, 1, 3, 4 and dense ranks 1, 1, 2, 3.
	// The two disagree exactly where the tie is, which is the point.
	wantRank := map[uint64]uint64{1: 1, 2: 1, 3: 3, 4: 4}
	wantDense := map[uint64]uint64{1: 1, 2: 1, 3: 2, 4: 3}
	for id, rank := range wantRank {
		values := got[id]
		if len(values) != 2 {
			t.Fatalf("row %d carries %d window values, want 2", id, len(values))
		}
		if n := asUint(t, values[0]); n != rank {
			t.Errorf("row %d ranked %d, want %d", id, n, rank)
		}
		if n := asUint(t, values[1]); n != wantDense[id] {
			t.Errorf("row %d dense-ranked %d, want %d", id, n, wantDense[id])
		}
	}
}

// The oracle: a whole-partition aggregate equals the grouped aggregate.
func TestPartitionAggregateAgreesWithGroupBy(t *testing.T) {
	session := readingsSession(t)
	ctx := testContext(t)

	got := windowValues(t, session, slate.Query{
		Table: "readings",
		Window: []slate.Window{
			// No order: the whole partition, on every row.
			slate.Over(slate.SumOf(slate.Key0(readingSize))).Over(
				[]slate.Column{slate.Key0(readingSite)}, nil,
			),
		},
	})

	stream, err := session.Aggregate(ctx, slate.Query{Table: "readings"}, slate.Grouping{
		GroupBy:    []slate.Column{slate.Key0(readingSite)},
		Aggregates: []slate.Aggregate{slate.SumOf(slate.Key0(readingSize))},
	})
	if err != nil {
		t.Fatalf("aggregating: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining the groups: %v", err)
	}
	if len(groups) != 2 {
		t.Fatalf("want 2 groups, got %d", len(groups))
	}
	bySite := map[string]int64{}
	for _, g := range groups {
		site, ok := g.Key[0].(slate.String)
		if !ok {
			t.Fatalf("a group key is not a string: %v", g.Key[0])
		}
		bySite[string(site)] = asInt(t, g.Values[0])
	}

	for _, r := range readings {
		if n := asInt(t, got[r.id][0]); n != bySite[r.site] {
			t.Errorf("row %d: window says %d, GROUP BY says %d", r.id, n, bySite[r.site])
		}
	}
}

// A running aggregate gives peers the same value — SQL's RANGE frame, not ROWS.
func TestARunningTotalGivesPeersTheSameValue(t *testing.T) {
	session := readingsSession(t)
	got := windowValues(t, session, slate.Query{
		Table: "readings",
		Window: []slate.Window{
			slate.Over(slate.Count()).Over(
				[]slate.Column{slate.Key0(readingSite)},
				[]slate.SortKey{{Column: readingSize}},
			),
		},
	})

	// north is 10, 10, 20, 30. Under RANGE the two tens are peers and both see
	// 2; under ROWS they would see 1 and 2, which is the mistake this catches.
	want := map[uint64]uint64{1: 2, 2: 2, 3: 3, 4: 4}
	for id, n := range want {
		if got := asUint(t, got[id][0]); got != n {
			t.Errorf("row %d running count %d, want %d", id, got, n)
		}
	}
}

// A sort can name a window, and only a sort can.
func TestASortCanNameAWindow(t *testing.T) {
	session := readingsSession(t)
	stream, err := session.Query(testContext(t), slate.Query{
		Table: "readings",
		Window: []slate.Window{
			slate.Over(slate.SumOf(slate.Key0(readingSize))).Over(
				[]slate.Column{slate.Key0(readingSite)}, nil,
			),
			slate.RowNumberOver().Over(
				[]slate.Column{slate.Key0(readingSite)},
				[]slate.SortKey{{Column: readingID}},
			),
		},
		Sort: []slate.SortKey{
			{Ref: refOf(slate.Windowed(1)), Direction: slate.Desc},
			{Column: readingID},
		},
	})
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	defer stream.Close()

	var numbers []uint64
	for stream.Next() {
		numbers = append(numbers, asUint(t, stream.Windowed()[1]))
		stream.Row()
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("draining: %v", err)
	}
	if len(numbers) != len(readings) {
		t.Fatalf("got %d rows, want %d", len(numbers), len(readings))
	}
	for i := 1; i < len(numbers); i++ {
		if numbers[i-1] < numbers[i] {
			t.Fatalf("not descending by the window's value: %v", numbers)
		}
	}
	// Not vacuous: the values actually differ, so "descending" is a claim.
	if numbers[0] == numbers[len(numbers)-1] {
		t.Fatalf("every row carries the same value, so any order would pass: %v", numbers)
	}
}

func refOf(c slate.Column) *slate.Column { return &c }

// refusalOf opens a query and drains it, returning why it stopped.
//
// Draining rather than checking the call's own error, and the difference is
// not incidental: `Session.Query` opens a *stream*, and gRPC does not deliver
// a rejected request until the first message is read. So a refusal this client
// reports arrives from `Err`, not from `Query` — which is why the three tests
// below each looked like "the server accepted it" when written the obvious
// way, and why this helper exists rather than three copies of the loop.
func refusalOf(t *testing.T, session *slate.Session, q slate.Query) error {
	t.Helper()
	stream, err := session.Query(testContext(t), q)
	if err != nil {
		return err
	}
	defer stream.Close()
	for stream.Next() {
		stream.Row()
	}
	return stream.Err()
}

// A filter cannot name a window, and the refusal survives the trip back.
func TestAFilterNamingAWindowIsRefused(t *testing.T) {
	session := readingsSession(t)
	err := refusalOf(t, session, slate.Query{
		Table: "readings",
		Window: []slate.Window{
			slate.RowNumberOver().Over(nil, []slate.SortKey{{Column: readingID}}),
		},
		Filter: slate.Filter(slate.Compare(slate.Windowed(0), slate.OpEq, slate.Uint(1))),
	})
	if err == nil {
		t.Fatal("a filter over a window was accepted")
	}
	if !strings.Contains(err.Error(), "after the filter") {
		t.Fatalf("the refusal did not say why: %v", err)
	}
}

// An unordered rank would be 1 on every row, so it is refused.
func TestAnUnorderedRankIsRefused(t *testing.T) {
	session := readingsSession(t)
	err := refusalOf(t, session, slate.Query{
		Table:  "readings",
		Window: []slate.Window{slate.RankOver()},
	})
	if err == nil {
		t.Fatal("an unordered rank was accepted")
	}
}

// LagOver at zero is the current row spelled obscurely, and is refused.
func TestLagAtOffsetZeroIsRefused(t *testing.T) {
	session := readingsSession(t)
	err := refusalOf(t, session, slate.Query{
		Table: "readings",
		Window: []slate.Window{
			slate.LagOver(slate.Key0(readingSize), 0).Over(
				nil, []slate.SortKey{{Column: readingID}},
			),
		},
	})
	if err == nil {
		t.Fatal("LAG at offset zero was accepted")
	}
}

// Only the field a function uses is sent, so a rank does not arrive carrying
// the zero-valued Aggregate — which is COUNT(*) and which the server refuses.
//
// The struct has one field per function and Go has no sum type, so the zero
// value of `Aggregate` is a *real* aggregate rather than an absence. Sending it
// unconditionally would make every rank a request the server rejects, and the
// rejection would look like a server bug.
func TestARankDoesNotCarryTheZeroAggregate(t *testing.T) {
	session := readingsSession(t)
	got := windowValues(t, session, slate.Query{
		Table: "readings",
		Window: []slate.Window{
			slate.RankOver().Over(nil, []slate.SortKey{{Column: readingSize}}),
		},
	})
	if len(got) != len(readings) {
		t.Fatalf("got %d rows, want %d", len(got), len(readings))
	}
}

// LagOver and LeadOver step through the partition, and the step is per
// partition rather than over the whole result.
//
// Written because a mutation survived: sending `LagOver`'s column as nil broke
// nothing, which meant the suite asked for a lag value nowhere. Every other
// test here uses a ranking function or an aggregate, and those carry no column
// at all — so the one line in `toProto` that fills `Column` was exercised by
// the refusal tests only, which pass whatever the reason for the refusal.
func TestLagAndLeadStepThroughThePartition(t *testing.T) {
	session := readingsSession(t)
	clause := func(w slate.Window) slate.Window {
		return w.Over(
			[]slate.Column{slate.Key0(readingSite)},
			[]slate.SortKey{{Column: readingID}},
		)
	}
	got := windowValues(t, session, slate.Query{
		Table: "readings",
		Window: []slate.Window{
			clause(slate.LagOver(slate.Key0(readingSize), 1)),
			clause(slate.LeadOver(slate.Key0(readingSize), 1)),
		},
	})

	// north is ids 1..4 with sizes 10, 10, 20, 30; south is 5..8 with the same.
	// Row 5 is the first of its partition, so its lag is null rather than row
	// 4's 30 — which is the assertion that the partition is honoured and not
	// just the order.
	type step struct{ lag, lead any }
	want := map[uint64]step{
		1: {nil, int64(10)},
		2: {int64(10), int64(20)},
		3: {int64(10), int64(30)},
		4: {int64(20), nil},
		5: {nil, int64(10)},
		8: {int64(20), nil},
	}
	check := func(id uint64, which int, label string, want any) {
		t.Helper()
		v := got[id][which]
		if want == nil {
			if _, ok := v.(slate.Null); !ok {
				t.Errorf("row %d %s is %v, want null", id, label, v)
			}
			return
		}
		if n := asInt(t, v); n != want.(int64) {
			t.Errorf("row %d %s is %d, want %d", id, label, n, want)
		}
	}
	for id, w := range want {
		check(id, 0, "lag", w.lag)
		check(id, 1, "lead", w.lead)
	}
}
