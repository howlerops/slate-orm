package slate_test

// `Contains`, through a real node and onto a real inverted index.
//
// The kernel's own suite proves a text index holds one entry per term and that
// a search over it agrees with a scan. This is the Go half of the other claim:
// that a caller can express the search, that the *server* does the splitting,
// and that `text = true` in a TOML schema declares an index a query can reach.
//
// The table below is this file's own, which is what makes it a test of the
// TOML surface as well as of the client: `slate-serverd` reads this text and
// builds the index from it.
//
// Every search that claims the index says so with a hint.
// `TestTheIndexAndAScanFindTheSameRows` checks the two plans differ before it
// compares their rows, because a text index is not chosen by cost at four rows
// — `docs/full-text.md` measures the crossover at roughly one row in 24,000 —
// so an unhinted search here would be a table scan and would pass with the
// index deleted.

import (
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

const fulltextTables = `
[[tables]]
name = "articles"
id = 61
columns = [
  { name = "id",    type = "u64" },
  { name = "title", type = "str" },
]
primary_key = ["id"]

[[tables.indexes]]
name = "by_title_text"
id = 61
columns = ["title"]
text = true

[[security.grants]]
role = "app"
tables = ["articles"]
actions = ["everything"]
`

const (
	articleID    slate.Ordinal = 0
	articleTitle slate.Ordinal = 1
)

// Prose chosen so the terms overlap three different ways: "rust" and "the"
// appear in several, "storage" in two, "engine" in one. A corpus where each
// term picked out exactly one row would make a conjunction and a disjunction
// indistinguishable.
var articles = [][2]any{
	{uint64(1), "Rust and the storage engine"},
	{uint64(2), "The storage layer, revisited"},
	{uint64(3), "rust: a retrospective"},
	{uint64(4), "Nothing to do with either"},
}

func articled(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, fulltextTables).client(t).Session()
	rows := make([][]slate.Value, 0, len(articles))
	for _, article := range articles {
		rows = append(rows, []slate.Value{
			slate.Uint(article[0].(uint64)),
			slate.String(article[1].(string)),
		})
	}
	if _, err := session.Insert(testContext(t), "articles", rows...); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	return session
}

// search is a `contains` over the articles, on the text index by default.
func search(text string, hinted bool) slate.Query {
	query := slate.Query{
		Table:  "articles",
		Filter: slate.Filter(slate.Contains(articleTitle, text)),
		Sort:   []slate.SortKey{{Column: articleID}},
		Hint:   slate.UsingIndex("by_title_text"),
	}
	if !hinted {
		query.Hint = slate.UsingTableScan()
	}
	return query
}

func foundIDs(t *testing.T, session *slate.Session, query slate.Query) []uint64 {
	t.Helper()
	stream, err := session.Query(testContext(t), query)
	if err != nil {
		t.Fatalf("searching: %v", err)
	}
	defer stream.Close()
	found := []uint64{}
	for stream.Next() {
		row := stream.Row()
		id, ok := row[articleID].(slate.Uint)
		if !ok {
			t.Fatalf("id came back as %T", row[articleID])
		}
		found = append(found, uint64(id))
	}
	if err := stream.Err(); err != nil {
		t.Fatalf("draining: %v", err)
	}
	return found
}

func sameIDs(a, b []uint64) bool {
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

func TestOneTermFindsEveryRowHoldingIt(t *testing.T) {
	session := articled(t)
	if found := foundIDs(t, session, search("rust", true)); !sameIDs(found, []uint64{1, 3}) {
		t.Fatalf("rust found %v, want [1 3]", found)
	}
}

func TestTheServerLowercasesTheSearch(t *testing.T) {
	// The client sent `RUST`; the rows hold `Rust` and `rust`. The fold
	// happened on the server, because [slate.Contains] does none.
	session := articled(t)
	if found := foundIDs(t, session, search("RUST", true)); !sameIDs(found, []uint64{1, 3}) {
		t.Fatalf("RUST found %v, want [1 3]", found)
	}
}

func TestTwoTermsAreAConjunctionNotADisjunction(t *testing.T) {
	// Rows 1 and 3 hold "rust"; rows 1 and 2 hold "storage". Only row 1 holds
	// both, and an OR would return three.
	session := articled(t)
	if found := foundIDs(t, session, search("rust storage", true)); !sameIDs(found, []uint64{1}) {
		t.Fatalf("`rust storage` found %v, want [1]", found)
	}
}

func TestPunctuationIsASeparatorAndNotPartOfATerm(t *testing.T) {
	// Row 3 is written `rust: a retrospective`, so its first term is `rust`,
	// and a search for `rust:` must find it. If the client sent terms rather
	// than text this is the case where its splitting and the server's would
	// have to agree by luck.
	session := articled(t)
	if found := foundIDs(t, session, search("rust:", true)); !sameIDs(found, []uint64{1, 3}) {
		t.Fatalf("`rust:` found %v, want [1 3]", found)
	}
}

func TestASubstringOfATermIsNotATerm(t *testing.T) {
	// `stor` is a prefix of `storage` and matches nothing. This is the line
	// between Contains and Like("%stor%"), and the reason both exist.
	session := articled(t)
	if found := foundIDs(t, session, search("stor", true)); len(found) != 0 {
		t.Fatalf("`stor` found %v, want nothing", found)
	}
}

func TestTheIndexAndAScanFindTheSameRows(t *testing.T) {
	// The oracle: two access paths, one answer. The plans are compared first,
	// without which this would pass with the hint ignored and both sides
	// scanning.
	session := articled(t)
	for _, text := range []string{"rust", "the storage", "rust storage engine", "nothing"} {
		indexed, scanned := search(text, true), search(text, false)

		byIndex, err := session.Explain(testContext(t), indexed)
		if err != nil {
			t.Fatalf("explaining the indexed search for %q: %v", text, err)
		}
		byScan, err := session.Explain(testContext(t), scanned)
		if err != nil {
			t.Fatalf("explaining the scan for %q: %v", text, err)
		}
		if byIndex.Access == byScan.Access {
			t.Fatalf("both paths planned the same way for %q (%s), so this compares a scan against a scan",
				text, byIndex.Access)
		}
		if !strings.Contains(byIndex.Access, "by_title_text") {
			t.Fatalf("the hinted plan for %q is %q, which does not name the text index", text, byIndex.Access)
		}

		if a, b := foundIDs(t, session, indexed), foundIDs(t, session, scanned); !sameIDs(a, b) {
			t.Fatalf("%q: the index found %v and a scan found %v", text, a, b)
		}
	}
}
