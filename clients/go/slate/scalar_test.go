package slate_test

import (
	"strings"
	"testing"
	"time"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// Computed values, and the two kinds of reference that name them.
//
// Go had no `Scalar` surface at all until now: it could not build a computed
// value, could not name one, and dropped the ones a row carried. Python could
// do all three. That gap is why the three-SDK conformance runner could not
// compare any of the date-and-time work.
//
// The oracle here is Go's own `time` package, which knows the Gregorian
// calendar independently of the kernel's transcribed `civil_from_days`. A test
// that computed the expected year by doing what the kernel does would prove
// only that the arithmetic is reproducible.

// A table of timestamps, so the calendar functions have something to read.
const eventTables = `
[[tables]]
name = "events"
id = 20
columns = [
  { name = "id",   type = "u64" },
  { name = "at",   type = "i64" },
  { name = "kind", type = "str" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["events"]
actions = ["everything"]
`

// Instants chosen to land in different years, months and weekdays, including a
// leap day and a value before the epoch — the two cases a naive `days / 365`
// or an unsigned division gets wrong.
var instants = []time.Time{
	time.Date(2024, time.February, 29, 13, 45, 0, 0, time.UTC),
	time.Date(1999, time.December, 31, 23, 59, 59, 0, time.UTC),
	time.Date(2001, time.January, 1, 0, 0, 0, 0, time.UTC),
	time.Date(1969, time.July, 20, 20, 17, 0, 0, time.UTC),
	time.Date(2010, time.June, 15, 6, 0, 0, 0, time.UTC),
}

func eventsSession(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, eventTables).client(t).Session()
	ctx := testContext(t)
	for i, at := range instants {
		_, err := session.Insert(ctx, "events", []slate.Value{
			slate.Uint(uint64(i)),
			slate.Int(at.Unix()),
			slate.String("e"),
		})
		if err != nil {
			t.Fatalf("seeding events: %v", err)
		}
	}
	return session
}

const (
	eventID = slate.Ordinal(0)
	eventAt = slate.Ordinal(1)
)

// collectComputed drains a query, returning each row's id and computed values.
func collectComputed(t *testing.T, session *slate.Session, q slate.Query) map[uint64][]slate.Value {
	t.Helper()
	stream, err := session.Query(testContext(t), q)
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	defer stream.Close()
	out := map[uint64][]slate.Value{}
	for stream.Next() {
		// Computed first: Row advances the cursor.
		computed := stream.Computed()
		row := stream.Row()
		id, ok := row[eventID].(slate.Uint)
		if !ok {
			t.Fatalf("id is not an unsigned integer: %v", row[eventID])
		}
		out[uint64(id)] = computed
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("draining: %v", err)
	}
	return out
}

// The calendar functions agree with Go's own `time`.
func TestCalendarPartsAgreeWithGoTime(t *testing.T) {
	session := eventsSession(t)
	got := collectComputed(t, session, slate.Query{
		Table: "events",
		Compute: []slate.Scalar{
			slate.YearOf(slate.Col(eventAt)),
			slate.MonthOf(slate.Col(eventAt)),
			slate.DayOfMonthOf(slate.Col(eventAt)),
			slate.DayOfWeekOf(slate.Col(eventAt)),
			slate.Extract(slate.Hour, slate.Col(eventAt)),
		},
	})
	if len(got) != len(instants) {
		t.Fatalf("got %d rows, want %d", len(got), len(instants))
	}
	for i, at := range instants {
		values := got[uint64(i)]
		if len(values) != 5 {
			t.Fatalf("%s: got %d computed values, want 5", at, len(values))
		}
		want := []int64{
			int64(at.Year()),
			int64(at.Month()),
			int64(at.Day()),
			int64(at.Weekday()), // Go's Sunday is 0, which is what the kernel uses.
			int64(at.Hour()),
		}
		for n, expected := range want {
			actual, ok := values[n].(slate.Int)
			if !ok {
				t.Fatalf("%s: computed value %d is not an integer: %v", at, n, values[n])
			}
			if int64(actual) != expected {
				t.Errorf("%s: computed value %d is %d, want %d", at, n, actual, expected)
			}
		}
	}
}

// `dateTrunc` to the day agrees with truncating in Go, and `round` returns an
// integer rather than a float — which is what lets it be a group key.
func TestDateTruncAndRound(t *testing.T) {
	session := eventsSession(t)
	got := collectComputed(t, session, slate.Query{
		Table: "events",
		Compute: []slate.Scalar{
			slate.DateTrunc(slate.Day, slate.Col(eventAt)),
			// `at / 86400` as a float would round; as two integers the kernel
			// divides exactly, so rounding it is the identity. Dividing by a
			// float is what makes this exercise `round` at all.
			slate.Round(slate.Div(slate.Col(eventAt), slate.Lit(slate.Float(86400.0)))),
		},
	})
	for i, at := range instants {
		values := got[uint64(i)]
		truncated, ok := values[0].(slate.Int)
		if !ok {
			t.Fatalf("%s: date_trunc produced %v", at, values[0])
		}
		if want := at.Truncate(24 * time.Hour).Unix(); int64(truncated) != want {
			t.Errorf("%s: date_trunc to the day is %d, want %d", at, truncated, want)
		}
		rounded, ok := values[1].(slate.Int)
		if !ok {
			t.Fatalf("%s: round produced %v rather than an integer", at, values[1])
		}
		// Go's math.Round, independently: halves away from zero.
		exact := float64(at.Unix()) / 86400.0
		want := int64(exact + 0.5)
		if exact < 0 {
			want = int64(exact - 0.5)
		}
		if int64(rounded) != want {
			t.Errorf("%s: round is %d, want %d", at, rounded, want)
		}
	}
}

// A later computed value may read an earlier one, and a filter may name one.
func TestAComputedValueReadsAnEarlierOneAndAFilterNamesIt(t *testing.T) {
	session := eventsSession(t)
	got := collectComputed(t, session, slate.Query{
		Table: "events",
		Compute: []slate.Scalar{
			slate.YearOf(slate.Col(eventAt)),
			// Reads computed value 0, which is the direction that is legal.
			slate.Sub(slate.Ref(slate.Computed0(0)), slate.Lit(slate.Int(2000))),
		},
		// And the filter names one too, which is the whole point of them
		// living in the same ordinal space as columns.
		Filter: slate.Filter(slate.Compare(slate.Computed0(0), slate.OpGe, slate.Int(2000))),
	})
	if len(got) == 0 {
		t.Fatal("the filter kept nothing; the fixture has years at or after 2000")
	}
	for id, values := range got {
		year, _ := values[0].(slate.Int)
		since, _ := values[1].(slate.Int)
		if year < 2000 {
			t.Errorf("row %d has year %d, which the filter should have removed", id, year)
		}
		if since != year-2000 {
			t.Errorf("row %d: %d != %d - 2000", id, since, year)
		}
	}
}

// Reading *forwards* is refused, by the server, with a message that says so.
func TestAComputedValueMayNotReadALaterOne(t *testing.T) {
	session := eventsSession(t)
	stream, err := session.Query(testContext(t), slate.Query{
		Table: "events",
		Compute: []slate.Scalar{
			// Reads computed value 1, which does not exist when it runs.
			slate.Ref(slate.Computed0(1)),
			slate.YearOf(slate.Col(eventAt)),
		},
	})
	// The refusal arrives on the stream rather than from the call: `Query`
	// opens a server-streaming RPC and gRPC does not deliver the status until
	// the first message is read. Asserting on the call alone passed while
	// testing nothing, which is how this was caught.
	if err == nil {
		_, err = stream.Collect()
	}
	if err == nil {
		t.Fatal("reading a later computed value should be refused")
	}
	if !strings.Contains(err.Error(), "available") {
		t.Errorf("the refusal should say how many are available: %v", err)
	}
}

// A join's own computed value: over the joined row, readable from both sides,
// and named with [slate.JoinComputed].
//
// This is the thing no input's own `Compute` can express. `books.year` minus
// nothing useful on the authors side in this fixture, so the expression
// concatenates across the join instead — a value that reads input 0 and input
// 1 at once.
func TestAJoinsComputedValueReadsBothSides(t *testing.T) {
	session := library(t)

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	books := b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})
	join := b.Query()
	join = slate.JoinQuery{
		Inputs: join.Inputs,
		Compute: []slate.Scalar{
			// `authors.name || "/" || books.title`, which needs both inputs.
			slate.Concat(
				slate.Ref(slate.At(authors, 1)),
				slate.Lit(slate.String("/")),
				slate.Ref(slate.At(books, 2)),
			),
		},
	}

	stream, err := session.Join(testContext(t), join)
	if err != nil {
		t.Fatalf("joining: %v", err)
	}
	defer stream.Close()

	seen := 0
	for stream.Next() {
		// Computed first: Row advances the cursor.
		computed := stream.Computed()
		row := stream.Row()
		if len(computed) != 1 {
			t.Fatalf("got %d computed values, want 1", len(computed))
		}
		name, ok := row[0][1].(slate.String)
		if !ok {
			t.Fatalf("the author's name is %v", row[0][1])
		}
		title, ok := row[1][2].(slate.String)
		if !ok {
			t.Fatalf("the book's title is %v", row[1][2])
		}
		want := slate.String(string(name) + "/" + string(title))
		if computed[0] != want {
			t.Errorf("computed %v, want %v", computed[0], want)
		}
		seen++
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("draining: %v", err)
	}
	if seen != 4 {
		t.Fatalf("got %d joined rows, want 4", seen)
	}
}

// And grouping *by* one, which is the query that was not expressible over gRPC
// at all until `JoinQuery.compute` existed.
func TestAGroupedJoinGroupsByTheJoinsComputedValue(t *testing.T) {
	session := library(t)

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	books := b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})
	join := slate.JoinQuery{
		Inputs: b.Query().Inputs,
		Compute: []slate.Scalar{
			// The decade a book came out in, which is a value neither table
			// stores.
			slate.Mul(
				slate.Div(slate.Ref(slate.At(books, 3)), slate.Lit(slate.Int(10))),
				slate.Lit(slate.Int(10)),
			),
		},
	}

	stream, err := session.AggregateJoin(testContext(t), join, slate.Grouping{
		GroupBy:    []slate.Column{slate.JoinComputed(0)},
		Aggregates: []slate.Aggregate{slate.Count()},
	})
	if err != nil {
		t.Fatalf("grouping: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}

	// The four matched books are 2001, 1990, 2003 and 2010: decades 2000 (two),
	// 1990 (one), 2010 (one).
	want := map[int64]uint64{2000: 2, 1990: 1, 2010: 1}
	if len(groups) != len(want) {
		t.Fatalf("got %d groups, want %d: %v", len(groups), len(want), groups)
	}
	for _, g := range groups {
		decade, ok := g.Key[0].(slate.Int)
		if !ok {
			t.Fatalf("the group key is %v, not an integer", g.Key[0])
		}
		count, ok := g.Values[0].(slate.Uint)
		if !ok {
			t.Fatalf("the count is %v", g.Values[0])
		}
		if uint64(count) != want[int64(decade)] {
			t.Errorf("decade %d has %d books, want %d", decade, count, want[int64(decade)])
		}
	}
}

// An input's *own* computed value cannot be named across the join, and the
// server says why rather than resolving it to whatever column sits there.
func TestAnInputsComputedValueIsNotNameableAcrossAJoin(t *testing.T) {
	session := library(t)

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{
		Table:   "authors",
		Compute: []slate.Scalar{slate.Upper(slate.Col(1))},
	})
	b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})

	stream, err := session.AggregateJoin(testContext(t), b.Query(), slate.Grouping{
		// `ComputedAt(0, 0)` names input 0's own computed value, which has no
		// slot in a joined row.
		GroupBy:    []slate.Column{slate.ComputedAt(0, 0)},
		Aggregates: []slate.Aggregate{slate.Count()},
	})
	if err == nil {
		_, err = stream.Collect()
	}
	if err == nil {
		t.Fatal("naming an input's computed value across a join should be refused")
	}
	if !strings.Contains(err.Error(), "joined_computed") {
		t.Errorf("the refusal should name the kind that does work: %v", err)
	}
}

// Sorting by a computed value, which [slate.SortKey.Ref] is for.
//
// The same gap the TypeScript suite had: every other test named a computed
// value in a filter, a group key or a projection and none in an ordering, so a
// client that ignored `Ref` and sorted by `Column` — which defaults to 0, the
// id — went unnoticed. The fixture's years are deliberately a different
// permutation from its ids, so the two orders cannot coincide.
func TestASortKeyMayNameAComputedValue(t *testing.T) {
	session := eventsSession(t)
	stream, err := session.Query(testContext(t), slate.Query{
		Table:   "events",
		Compute: []slate.Scalar{slate.YearOf(slate.Col(eventAt))},
		Sort:    []slate.SortKey{{Ref: &[]slate.Column{slate.Computed0(0)}[0]}},
	})
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	defer stream.Close()

	var years []int64
	var ids []uint64
	for stream.Next() {
		computed := stream.Computed()
		row := stream.Row()
		y, ok := computed[0].(slate.Int)
		if !ok {
			t.Fatalf("the year is %v", computed[0])
		}
		id, _ := row[eventID].(slate.Uint)
		years = append(years, int64(y))
		ids = append(ids, uint64(id))
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("draining: %v", err)
	}
	if len(years) != len(instants) {
		t.Fatalf("got %d rows, want %d", len(years), len(instants))
	}
	for i := 1; i < len(years); i++ {
		if years[i] < years[i-1] {
			t.Fatalf("years arrived out of order: %v", years)
		}
	}
	// And the id order is not the year order, so sorting by column 0 would be
	// a different answer.
	ascending := true
	for i := 1; i < len(ids); i++ {
		if ids[i] < ids[i-1] {
			ascending = false
		}
	}
	if ascending {
		t.Fatal("the fixture's ids are already in year order; this test proves nothing")
	}
}

// Truncating to a month and a year, which `DateTrunc` cannot do.
//
// `TimeUnit` promises a fixed number of seconds and a month has none, so this
// decodes the date, drops the fields below the boundary and encodes it again.
// Go's own `time` is the oracle: `time.Date(y, m, 1, ...)` in UTC.
func TestCalendarTruncationAgreesWithGoTime(t *testing.T) {
	session := eventsSession(t)
	got := collectComputed(t, session, slate.Query{
		Table: "events",
		Compute: []slate.Scalar{
			slate.MonthStartOf(slate.Col(eventAt)),
			slate.YearStartOf(slate.Col(eventAt)),
		},
	})
	for i, at := range instants {
		values := got[uint64(i)]
		if len(values) != 2 {
			t.Fatalf("%s: got %d computed values, want 2", at, len(values))
		}
		wantMonth := time.Date(at.Year(), at.Month(), 1, 0, 0, 0, 0, time.UTC).Unix()
		wantYear := time.Date(at.Year(), time.January, 1, 0, 0, 0, 0, time.UTC).Unix()
		gotMonth, ok := values[0].(slate.Int)
		if !ok {
			t.Fatalf("%s: month_start is %v", at, values[0])
		}
		gotYear, ok := values[1].(slate.Int)
		if !ok {
			t.Fatalf("%s: year_start is %v", at, values[1])
		}
		if int64(gotMonth) != wantMonth {
			t.Errorf("%s: month start is %d, want %d", at, gotMonth, wantMonth)
		}
		if int64(gotYear) != wantYear {
			t.Errorf("%s: year start is %d, want %d", at, gotYear, wantYear)
		}
		// And it floors rather than rounding: the 1969 instant must go
		// backwards to 1969-07-01, not forwards into 1970.
		if int64(gotMonth) > at.Unix() {
			t.Errorf("%s: truncation moved forwards to %d", at, gotMonth)
		}
	}
}
