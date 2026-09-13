package slate_test

import (
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// Authors and their books, the same shape the Rust wire tests join.
const joinTables = `
[[tables]]
name = "authors"
id = 10
columns = [
  { name = "id",      type = "u64" },
  { name = "name",    type = "str" },
  { name = "country", type = "str" },
]
primary_key = ["id"]

[[tables]]
name = "books"
id = 11
columns = [
  { name = "id",        type = "u64" },
  { name = "author_id", type = "u64" },
  { name = "title",     type = "str" },
  { name = "year",      type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["authors", "books"]
actions = ["everything"]
`

// seedLibrary writes three authors and five books.
//
// Author 1 has three books, author 2 has one, author 3 has none — so an inner
// join and a left join disagree, a count per author is not uniform, and an
// ordering by that count is not the same as an ordering by key. Each of those
// is load-bearing for a test below.
func seedLibrary(t *testing.T, session *slate.Session) {
	t.Helper()
	ctx := testContext(t)
	for _, author := range [][]slate.Value{
		{slate.Uint(1), slate.String("ada"), slate.String("UK")},
		{slate.Uint(2), slate.String("bo"), slate.String("US")},
		{slate.Uint(3), slate.String("cy"), slate.String("UK")},
	} {
		if _, err := session.Insert(ctx, "authors", author); err != nil {
			t.Fatalf("seeding authors: %v", err)
		}
	}
	for _, book := range [][]slate.Value{
		{slate.Uint(10), slate.Uint(1), slate.String("a-one"), slate.Int(2001)},
		{slate.Uint(11), slate.Uint(1), slate.String("a-two"), slate.Int(1990)},
		{slate.Uint(12), slate.Uint(1), slate.String("a-three"), slate.Int(2003)},
		{slate.Uint(13), slate.Uint(2), slate.String("b-one"), slate.Int(2010)},
		{slate.Uint(14), slate.Uint(9), slate.String("orphan"), slate.Int(2020)},
	} {
		if _, err := session.Insert(ctx, "books", book); err != nil {
			t.Fatalf("seeding books: %v", err)
		}
	}
}

// library starts a node with the join tables and seeds it.
func library(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, joinTables).client(t).Session()
	seedLibrary(t, session)
	return session
}

// authorsBooks is authors joined to their books, as the given join type.
func authorsBooks(join slate.JoinType) slate.JoinQuery {
	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	b.Add(slate.JoinInput{
		Table: "books",
		Type:  join,
		// authors.id equals books.author_id: the left side names its input,
		// the right side is an ordinal of the input it is written on.
		On: []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})
	return b.Query()
}

func TestAnInnerJoinKeepsOnlyMatchedRows(t *testing.T) {
	session := library(t)
	join := authorsBooks(slate.Inner)

	stream, err := session.Join(testContext(t), join)
	if err != nil {
		t.Fatalf("joining: %v", err)
	}
	rows, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	// Four books have a matching author; the orphan does not, and author 3 has
	// no books.
	if len(rows) != 4 {
		t.Fatalf("got %d joined rows, want 4", len(rows))
	}
	for _, row := range rows {
		if len(row) != 2 {
			t.Fatalf("a joined row has one slice per input, got %d", len(row))
		}
		if row[0] == nil || row[1] == nil {
			t.Error("an inner join has no unmatched side")
		}
	}
}

// A left join pads the unmatched side with a nil slice rather than a row of
// nulls, which is the distinction a flat row cannot make.
func TestALeftJoinKeepsTheUnmatchedLeftSide(t *testing.T) {
	session := library(t)
	join := authorsBooks(slate.Left)

	stream, err := session.Join(testContext(t), join)
	if err != nil {
		t.Fatalf("joining: %v", err)
	}
	rows, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	if len(rows) != 5 {
		t.Fatalf("got %d rows, want 5 (four matches plus author 3)", len(rows))
	}
	unmatched := 0
	for _, row := range rows {
		if row[1] == nil {
			unmatched++
		}
	}
	if unmatched != 1 {
		t.Errorf("exactly author 3 is unmatched, got %d unmatched rows", unmatched)
	}
}

func TestAJoinLimitApplies(t *testing.T) {
	session := library(t)
	join := authorsBooks(slate.Inner)
	two := uint64(2)
	join.Limit = &two

	stream, err := session.Join(testContext(t), join)
	if err != nil {
		t.Fatalf("joining: %v", err)
	}
	rows, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	if len(rows) != 2 {
		t.Errorf("got %d rows, want 2", len(rows))
	}
}

// The four join types must not all return the same thing, or the type is not
// reaching the server at all.
func TestTheFourJoinTypesDiffer(t *testing.T) {
	session := library(t)
	counts := map[slate.JoinType]int{}
	for _, kind := range []slate.JoinType{slate.Inner, slate.Left, slate.Right, slate.Full} {
		join := authorsBooks(kind)
		stream, err := session.Join(testContext(t), join)
		if err != nil {
			t.Fatalf("joining: %v", err)
		}
		rows, err := stream.Collect()
		if err != nil {
			t.Fatalf("draining: %v", err)
		}
		counts[kind] = len(rows)
	}
	if counts[slate.Inner] >= counts[slate.Left] {
		t.Errorf("a left join keeps more: inner=%d left=%d", counts[slate.Inner], counts[slate.Left])
	}
	if counts[slate.Inner] >= counts[slate.Right] {
		t.Errorf("a right join keeps the orphan: inner=%d right=%d",
			counts[slate.Inner], counts[slate.Right])
	}
	if counts[slate.Full] <= counts[slate.Left] {
		t.Errorf("a full join keeps most: full=%d left=%d", counts[slate.Full], counts[slate.Left])
	}
}

// Forcing each algorithm must not change the answer. This is the oracle the
// Rust suite runs, restated here so that a client that mis-sends the force
// flag is caught rather than silently ignored.
func TestEveryJoinAlgorithmAgrees(t *testing.T) {
	session := library(t)
	var reference int
	for i, algorithm := range []slate.Algorithm{
		slate.Chosen, slate.HashBuildLeft, slate.HashBuildRight, slate.NestedLoop,
	} {
		b := slate.NewJoin()
		authors := b.Add(slate.JoinInput{Table: "authors"})
		b.Add(slate.JoinInput{
			Table: "books",
			On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
			Force: algorithm,
		})

		stream, err := session.Join(testContext(t), b.Query())
		if err != nil {
			t.Fatalf("joining under algorithm %d: %v", algorithm, err)
		}
		rows, err := stream.Collect()
		if err != nil {
			t.Fatalf("draining under algorithm %d: %v", algorithm, err)
		}
		if i == 0 {
			reference = len(rows)
			continue
		}
		if len(rows) != reference {
			t.Errorf("algorithm %d returned %d rows, the planner's choice returned %d",
				algorithm, len(rows), reference)
		}
	}
}

func TestAggregateOverOneTable(t *testing.T) {
	session := library(t)

	stream, err := session.Aggregate(testContext(t),
		slate.Query{Table: "books"},
		slate.Grouping{Aggregates: []slate.Aggregate{slate.Count()}})
	if err != nil {
		t.Fatalf("aggregating: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	if len(groups) != 1 {
		t.Fatalf("no GROUP BY means one group, got %d", len(groups))
	}
	if got := groups[0].Values[0]; got != slate.Uint(5) {
		t.Errorf("count = %v, want 5", got)
	}
}

func TestGroupedAggregate(t *testing.T) {
	session := library(t)

	stream, err := session.Aggregate(testContext(t),
		slate.Query{Table: "books"},
		slate.Grouping{
			GroupBy:    []slate.Column{slate.Key0(1)},
			Aggregates: []slate.Aggregate{slate.Count(), slate.MaxOf(3)},
		})
	if err != nil {
		t.Fatalf("aggregating: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	// Authors 1, 2 and the orphan's 9.
	if len(groups) != 3 {
		t.Fatalf("got %d groups, want 3", len(groups))
	}
	for _, group := range groups {
		if group.Key[0] == slate.Uint(1) {
			if group.Values[0] != slate.Uint(3) {
				t.Errorf("author 1 has three books, got %v", group.Values[0])
			}
			if group.Values[1] != slate.Int(2003) {
				t.Errorf("author 1's latest is 2003, got %v", group.Values[1])
			}
		}
	}
}

func TestAGroupedJoin(t *testing.T) {
	session := library(t)
	join := authorsBooks(slate.Inner)

	stream, err := session.AggregateJoin(testContext(t), join, slate.Grouping{
		GroupBy:    []slate.Column{slate.At(0, 0)},
		Aggregates: []slate.Aggregate{slate.Count()},
	})
	if err != nil {
		t.Fatalf("grouping a join: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	// Authors 1 and 2 have books; author 3 does not and an inner join drops it.
	if len(groups) != 2 {
		t.Fatalf("got %d groups, want 2", len(groups))
	}
}

// The joined schema has to span every input for this to resolve. A group key
// on input 0 does not prove that — narrowing the space to the first input
// passes every other test here.
func TestAGroupKeyOnTheRightSideOfTheJoin(t *testing.T) {
	session := library(t)
	join := authorsBooks(slate.Inner)

	stream, err := session.AggregateJoin(testContext(t), join, slate.Grouping{
		GroupBy:    []slate.Column{slate.At(1, 1)}, // books.author_id
		Aggregates: []slate.Aggregate{slate.Count()},
	})
	if err != nil {
		t.Fatalf("grouping on the right side: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	if len(groups) != 2 {
		t.Fatalf("the same partition named from the other side: got %d groups, want 2",
			len(groups))
	}
}

// Ordering, limiting and offsetting are over *groups*. Ascending by count is
// chosen because it disagrees with the kernel's own order — ascending by
// encoded key — so an ordering the client dropped would change the answer.
func TestGroupsAreOrderedLimitedAndOffset(t *testing.T) {
	session := library(t)
	join := authorsBooks(slate.Inner)
	one := uint64(1)

	ordered := func(offset uint64, limit *uint64) []slate.Group {
		t.Helper()
		stream, err := session.AggregateJoin(testContext(t), join, slate.Grouping{
			GroupBy:    []slate.Column{slate.At(0, 0)},
			Aggregates: []slate.Aggregate{slate.Count()},
			Sort: []slate.GroupSortKey{
				{Column: slate.Agg(0), Direction: slate.Asc},
				{Column: slate.Key(0), Direction: slate.Asc},
			},
			Limit:  limit,
			Offset: offset,
		})
		if err != nil {
			t.Fatalf("ordering groups: %v", err)
		}
		groups, err := stream.Collect()
		if err != nil {
			t.Fatalf("draining: %v", err)
		}
		return groups
	}

	all := ordered(0, nil)
	if len(all) != 2 {
		t.Fatalf("got %d groups, want 2", len(all))
	}
	// Author 2 has one book, author 1 has three: fewest first.
	if all[0].Key[0] != slate.Uint(2) || all[1].Key[0] != slate.Uint(1) {
		t.Fatalf("groups are not ascending by count: %v", all)
	}

	first := ordered(0, &one)
	if len(first) != 1 || first[0].Key[0] != slate.Uint(2) {
		t.Errorf("the limit takes the first group: %v", first)
	}

	second := ordered(1, &one)
	if len(second) != 1 || second[0].Key[0] != slate.Uint(1) {
		t.Errorf("the offset skips it: %v", second)
	}
}

func TestHavingKeepsGroups(t *testing.T) {
	session := library(t)

	stream, err := session.Aggregate(testContext(t),
		slate.Query{Table: "books"},
		slate.Grouping{
			GroupBy:    []slate.Column{slate.Key0(1)},
			Aggregates: []slate.Aggregate{slate.Count()},
			Having:     slate.Filter(slate.GroupGt(slate.Agg(0), slate.Uint(1))),
		})
	if err != nil {
		t.Fatalf("aggregating: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	if len(groups) != 1 {
		t.Fatalf("only author 1 has more than one book, got %d groups", len(groups))
	}
	if groups[0].Key[0] != slate.Uint(1) {
		t.Errorf("the surviving group is author 1, got %v", groups[0].Key[0])
	}
}

// A three-table chain, grouped.
//
// This asserted a refusal until the kernel grew `group_by_chain`. Rewritten
// rather than deleted: a refusal test that outlives the refusal passes forever
// and protects nothing.
func TestGroupingAChain(t *testing.T) {
	session := library(t)

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	books := b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})
	// A third input joining back to the second, which is what makes it a chain
	// rather than a pair.
	b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(books, 0), Own: 0}},
	})

	stream, err := session.AggregateJoin(testContext(t), b.Query(), slate.Grouping{
		GroupBy:    []slate.Column{slate.At(0, 0)},
		Aggregates: []slate.Aggregate{slate.Count()},
	})
	if err != nil {
		t.Fatalf("grouping a chain: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	// Authors 1 and 2 have books; joining books to itself on the id keeps one
	// row per book, so the counts are unchanged.
	if len(groups) != 2 {
		t.Fatalf("got %d groups, want 2", len(groups))
	}
}

func TestExplainJoinDescribesEveryInput(t *testing.T) {
	session := library(t)
	join := authorsBooks(slate.Inner)

	plan, err := session.ExplainJoin(testContext(t), join)
	if err != nil {
		t.Fatalf("explaining a join: %v", err)
	}
	if len(plan.Inputs) != 2 {
		t.Fatalf("got %d input plans, want 2", len(plan.Inputs))
	}
	if plan.Inputs[0].Plan.Table != "authors" || plan.Inputs[1].Plan.Table != "books" {
		t.Errorf("input plans name the wrong tables: %+v", plan.Inputs)
	}
	if plan.Inputs[1].Algorithm == "" {
		t.Error("the second input should name a join algorithm")
	}
}

// Forcing an algorithm must actually reach the planner.
//
// TestEveryJoinAlgorithmAgrees cannot tell: if the force flag is dropped, all
// four runs use the planner's choice and agree trivially, which is precisely
// what "they agree" looks like. Dropping the flag survived that test. The plan
// is where the difference is visible, so this asserts on the plan.
func TestAForcedAlgorithmReachesThePlanner(t *testing.T) {
	session := library(t)

	planFor := func(algorithm slate.Algorithm) string {
		t.Helper()
		b := slate.NewJoin()
		authors := b.Add(slate.JoinInput{Table: "authors"})
		b.Add(slate.JoinInput{
			Table: "books",
			On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
			Force: algorithm,
		})
		plan, err := session.ExplainJoin(testContext(t), b.Query())
		if err != nil {
			t.Fatalf("explaining under algorithm %d: %v", algorithm, err)
		}
		if len(plan.Inputs) != 2 {
			t.Fatalf("got %d input plans, want 2", len(plan.Inputs))
		}
		return plan.Inputs[1].Algorithm
	}

	if got := planFor(slate.NestedLoop); got != "nested loop" {
		t.Errorf("forcing a nested loop gave the %q plan", got)
	}
	if got := planFor(slate.HashBuildLeft); got != "hash" {
		t.Errorf("forcing a hash join gave the %q plan", got)
	}
}
