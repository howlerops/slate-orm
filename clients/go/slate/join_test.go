package slate_test

import (
	"fmt"
	"strings"
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

// Keys and no aggregates are the distinct combinations of those keys, which is
// what SELECT DISTINCT means. The server used to refuse this along with the
// genuinely meaningless case (no keys and no aggregates), so a caller wanting
// distinct values had to ask for a count and throw it away.
func TestGroupKeysWithNoAggregatesAreTheDistinctValues(t *testing.T) {
	session := library(t)

	stream, err := session.Aggregate(testContext(t),
		slate.Query{Table: "books"},
		slate.Grouping{GroupBy: []slate.Column{slate.Key0(1)}})
	if err != nil {
		t.Fatalf("aggregating: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}

	// The oracle: the author ids in the rows themselves, read back through the
	// same client rather than written down here.
	rows, err := session.Query(testContext(t), slate.Query{Table: "books"})
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	all, err := rows.Collect()
	if err != nil {
		t.Fatalf("draining rows: %v", err)
	}
	distinct := map[string]struct{}{}
	for _, row := range all {
		distinct[fmt.Sprint(row[1])] = struct{}{}
	}
	if len(distinct) < 2 {
		t.Fatalf("a one-value column proves nothing: %v", distinct)
	}
	if len(groups) != len(distinct) {
		t.Fatalf("got %d groups for %d distinct author ids", len(groups), len(distinct))
	}
	for _, group := range groups {
		if _, ok := distinct[fmt.Sprint(group.Key[0])]; !ok {
			t.Errorf("group key %v is not an author id in the rows", group.Key[0])
		}
		// And nothing beside the key: a server that helpfully added a count
		// would pass every assertion above.
		if len(group.Values) != 0 {
			t.Errorf("group %v carries %d values, want none", group.Key, len(group.Values))
		}
	}
}

// Neither keys nor aggregates is still refused: it asks for one group with
// nothing in it.
func TestNeitherKeysNorAggregatesIsRefused(t *testing.T) {
	session := library(t)

	stream, err := session.Aggregate(testContext(t),
		slate.Query{Table: "books"}, slate.Grouping{})
	if err == nil {
		_, err = stream.Collect()
	}
	if err == nil {
		t.Fatal("an aggregate with no keys and no aggregates must be refused")
	}
	if !strings.Contains(err.Error(), "nothing in it") {
		t.Errorf("the refusal should say what was asked for: %v", err)
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
			Aggregates: []slate.Aggregate{slate.Count(), slate.MaxOf(slate.Key0(3))},
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

// ExplainAggregate describes the grouped read, not the read it groups.
//
// The whole point of the separate RPC. Grouping narrows each input's
// projection to the group keys, the aggregates' columns and the join keys, so
// the two plans decode different amounts of every row — and on a fixture with
// no usable index the access path is the same either way, which is why the
// assertion is on Decodes rather than on IndexOnly or on the display string.
func TestExplainingAGroupedJoinIsNotExplainingTheJoin(t *testing.T) {
	session := library(t)
	join := authorsBooks(slate.Inner)

	plain, err := session.ExplainJoin(testContext(t), join)
	if err != nil {
		t.Fatalf("explaining the join: %v", err)
	}
	// Group by authors.country (ordinal 2), max over books.year (ordinal 3 of
	// input 1). Chosen so that the columns
	// the grouping names are *not* the join keys: a plan that ignored the
	// grouping entirely would still narrow — to the join keys alone — and
	// "narrower than ungrouped" would pass. What must hold is that these
	// columns, and not some other narrow set, are what the plan decodes.
	grouped, err := session.ExplainAggregateJoin(testContext(t), join, slate.Grouping{
		GroupBy:    []slate.Column{slate.At(0, 2)},
		Aggregates: []slate.Aggregate{slate.Count(), slate.MaxOf(slate.At(1, 3))},
	})
	if err != nil {
		t.Fatalf("explaining the grouped join: %v", err)
	}
	if grouped.Join == nil {
		t.Fatal("a grouped join must explain as a join")
	}
	if grouped.Input != nil {
		t.Error("the one-table field must stay unset for a join")
	}
	if len(grouped.Join.Inputs) != len(plain.Inputs) {
		t.Fatalf("input counts differ: %d vs %d", len(plain.Inputs), len(grouped.Join.Inputs))
	}

	narrowed := false
	for i := range plain.Inputs {
		wide := plain.Inputs[i].Plan.Decodes
		narrow := grouped.Join.Inputs[i].Plan.Decodes
		if len(narrow) > len(wide) {
			t.Errorf("grouping made input %d decode more: %v -> %v", i, wide, narrow)
		}
		if len(narrow) < len(wide) {
			narrowed = true
		}
	}
	if !narrowed {
		t.Errorf("grouping narrowed nothing, so the plan is not the grouped read's:\n%s\nvs\n%s",
			plain.Display, grouped.Display)
	}
	if !strings.HasPrefix(grouped.Display, "Group by [") {
		t.Errorf("the display does not say what is being grouped: %q", grouped.Display)
	}

	// The grouping's own columns, in each input's own ordinals.
	if !contains(grouped.Join.Inputs[0].Plan.Decodes, 2) {
		t.Errorf("the group key authors.country is not decoded: %v",
			grouped.Join.Inputs[0].Plan.Decodes)
	}
	if !contains(grouped.Join.Inputs[1].Plan.Decodes, 3) {
		t.Errorf("the aggregated books.year is not decoded: %v",
			grouped.Join.Inputs[1].Plan.Decodes)
	}
}

func contains(haystack []uint32, needle uint32) bool {
	for _, v := range haystack {
		if v == needle {
			return true
		}
	}
	return false
}

// One table lands in the other field, and only in it.
func TestExplainingAGroupedTableAnswersInTheInputField(t *testing.T) {
	session := library(t)

	plan, err := session.ExplainAggregate(testContext(t),
		slate.Query{Table: "books"},
		slate.Grouping{
			GroupBy:    []slate.Column{slate.Key0(1)},
			Aggregates: []slate.Aggregate{slate.Count()},
		})
	if err != nil {
		t.Fatalf("explaining a grouped table: %v", err)
	}
	if plan.Input == nil {
		t.Fatal("a grouped table must explain as a table")
	}
	if plan.Join != nil {
		t.Error("the join field must stay unset for one table")
	}
	if plan.Input.Table != "books" {
		t.Errorf("the plan names %q", plan.Input.Table)
	}
}

// A chain's own computed value, read back on the row path and grouped by.
//
// `Chain::compute` and `Join::compute` are one field on the wire and two code
// paths in the kernel: a join's is filled in by the cursor as it pairs rows, a
// chain's by a pass over the accumulated rows after the last step. Nothing in
// any client suite had asked the second one for anything, so this does — with
// an expression reading all three inputs, which is what makes it a chain's
// value rather than a join's.
//
// The third input is `books` joined back to itself on its own id, which keeps
// one row per book and so leaves the counts unchanged. That matters: a chain
// whose last step multiplied rows would make the assertions below true for the
// wrong reason.
func TestAChainsComputedValueReadsEveryInput(t *testing.T) {
	session := library(t)

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	books := b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})
	again := b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(books, 0), Own: 0}},
	})
	chain := b.Query()
	chain = slate.JoinQuery{
		Inputs: chain.Inputs,
		Compute: []slate.Scalar{
			// Reads inputs 0, 1 and 2. Evaluated against a prefix of the
			// accumulated row it would be null; against the last step alone it
			// would be missing the name.
			slate.Concat(
				slate.Ref(slate.At(authors, 1)),
				slate.Lit(slate.String("/")),
				slate.Ref(slate.At(books, 2)),
				slate.Lit(slate.String("/")),
				slate.Ref(slate.At(again, 0)),
			),
		},
	}

	stream, err := session.Join(testContext(t), chain)
	if err != nil {
		t.Fatalf("chaining: %v", err)
	}
	defer stream.Close()

	seen := 0
	for stream.Next() {
		computed := stream.Computed()
		row := stream.Row()
		if len(row) != 3 {
			t.Fatalf("a chained row has one slice per input, got %d", len(row))
		}
		if len(computed) != 1 {
			t.Fatalf("got %d computed values, want 1", len(computed))
		}
		name, okName := row[0][1].(slate.String)
		title, okTitle := row[1][2].(slate.String)
		id, okID := row[2][0].(slate.Uint)
		if !okName || !okTitle || !okID {
			t.Fatalf("the row is %v", row)
		}
		// `Concat` renders a non-string as its text, so the id is its digits.
		// It used to render the Debug form — `Uint(10)` — which is the bug a
		// Python client test caught and this would have inherited.
		want := slate.String(fmt.Sprintf("%s/%s/%d", name, title, uint64(id)))
		if computed[0] != want {
			t.Errorf("computed %v, want %v", computed[0], want)
		}
		seen++
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("draining: %v", err)
	}
	if seen != 4 {
		t.Fatalf("got %d chained rows, want 4", seen)
	}
}

// And grouping a chain *by* the value it computes, which resolves in the
// chain's joined space after the grouped path has narrowed each step.
func TestGroupingAChainByItsComputedValue(t *testing.T) {
	session := library(t)

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	books := b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})
	b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(books, 0), Own: 0}},
	})
	chain := b.Query()
	chain = slate.JoinQuery{
		Inputs: chain.Inputs,
		// The decade a book came out in, which no table stores.
		Compute: []slate.Scalar{
			slate.Mul(
				slate.Div(slate.Ref(slate.At(books, 3)), slate.Lit(slate.Int(10))),
				slate.Lit(slate.Int(10)),
			),
		},
	}

	stream, err := session.AggregateJoin(testContext(t), chain, slate.Grouping{
		GroupBy:    []slate.Column{slate.JoinComputed(0)},
		Aggregates: []slate.Aggregate{slate.Count()},
	})
	if err != nil {
		t.Fatalf("grouping a chain by its computed value: %v", err)
	}
	groups, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}

	// The four matched books are from 2001, 1990, 2003 and 2010, so three
	// decades with the 2000s holding two. Written out rather than folded from
	// another query: the numbers come from the fixture above, so a grouping
	// that landed on some table's column instead fails here rather than
	// agreeing with an equally wrong fold.
	want := map[int64]uint64{1990: 1, 2000: 2, 2010: 1}
	if len(groups) != len(want) {
		t.Fatalf("got %d groups, want %d: %+v", len(groups), len(want), groups)
	}
	for _, group := range groups {
		decade, ok := group.Key[0].(slate.Int)
		if !ok {
			t.Fatalf("the key is %v, not an i64 decade", group.Key[0])
		}
		count, ok := group.Values[0].(slate.Uint)
		if !ok {
			t.Fatalf("a count is a u64, got %v", group.Values[0])
		}
		if uint64(count) != want[int64(decade)] {
			t.Errorf("decade %d counted %d, want %d", decade, count, want[int64(decade)])
		}
	}
}

// And *ordering* those groups by the chain's computed value, which
// `ledger/2026-09-15-a-chains-computed-value-and-a-type-tag-in-a-label.md`
// recorded as untested from any client.
//
// Descending, deliberately. The kernel's own order is ascending by encoded
// key, so an ascending assertion passes against a server that dropped the
// sort entirely — which is the shape of test that records a feature as
// covered while covering nothing. Descending disagrees with the default, so
// the answer changes if the sort does not arrive.
func TestOrderingAChainsGroupsByItsComputedValue(t *testing.T) {
	session := library(t)

	b := slate.NewJoin()
	authors := b.Add(slate.JoinInput{Table: "authors"})
	books := b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(authors, 0), Own: 1}},
	})
	b.Add(slate.JoinInput{
		Table: "books",
		On:    []slate.On{{Earlier: slate.At(books, 0), Own: 0}},
	})
	chain := slate.JoinQuery{
		Inputs: b.Query().Inputs,
		Compute: []slate.Scalar{
			slate.Mul(
				slate.Div(slate.Ref(slate.At(books, 3)), slate.Lit(slate.Int(10))),
				slate.Lit(slate.Int(10)),
			),
		},
	}

	decades := func(direction slate.Direction) []int64 {
		t.Helper()
		stream, err := session.AggregateJoin(testContext(t), chain, slate.Grouping{
			GroupBy:    []slate.Column{slate.JoinComputed(0)},
			Aggregates: []slate.Aggregate{slate.Count()},
			Sort:       []slate.GroupSortKey{{Column: slate.Key(0), Direction: direction}},
		})
		if err != nil {
			t.Fatalf("ordering a chain's groups by its computed value: %v", err)
		}
		groups, err := stream.Collect()
		if err != nil {
			t.Fatalf("draining: %v", err)
		}
		out := make([]int64, 0, len(groups))
		for _, group := range groups {
			decade, ok := group.Key[0].(slate.Int)
			if !ok {
				t.Fatalf("the key is %v, not an i64 decade", group.Key[0])
			}
			out = append(out, int64(decade))
		}
		return out
	}

	// Compared as text because `[]int64` is not comparable with `==` and
	// pulling in `reflect` for three integers is more machinery than the
	// assertion is worth.
	descending := fmt.Sprint(decades(slate.Desc))
	if descending != "[2010 2000 1990]" {
		t.Fatalf("descending by the computed decade: got %s", descending)
	}

	// The ascending run is here to prove the descending one was the sort
	// talking rather than the fixture: the two must be reverses of each
	// other, which they cannot be if the server ignored `Sort` in both.
	ascending := fmt.Sprint(decades(slate.Asc))
	if ascending != "[1990 2000 2010]" {
		t.Fatalf("ascending by the computed decade: got %s", ascending)
	}
}
