package slate_test

// An array column, through a real node.
//
// The kernel stores arrays and the wire carries them; this is the Go half of
// proving a list survives the round trip with its elements' *types* intact and
// not merely their text. A `["1"]` decoded as strings and a `[1]` decoded as
// integers are different rows to this server — its ordering is type-first — so
// every assertion below checks the element's Go type as well as its value.
//
// Two element types on one table, because a client with one array column can
// hard-code the element type it decodes and pass.

import (
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

const arrayTables = `
[[tables]]
name = "posts"
id = 60
columns = [
  { name = "id",     type = "u64" },
  { name = "title",  type = "str" },
  { name = "tags",   type = "array", element = "str" },
  { name = "scores", type = "array", element = "i64", nullable = true },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["posts"]
actions = ["everything"]
`

func postRow(id uint64, title string, tags []string, scores []int64) []slate.Value {
	tagValues := make(slate.Array, len(tags))
	for i, tag := range tags {
		tagValues[i] = slate.String(tag)
	}
	var scoreValue slate.Value = slate.Null{}
	if scores != nil {
		list := make(slate.Array, len(scores))
		for i, n := range scores {
			list[i] = slate.Int(n)
		}
		scoreValue = list
	}
	return []slate.Value{slate.Uint(id), slate.String(title), tagValues, scoreValue}
}

// posted starts a node holding three posts.
func posted(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, arrayTables).client(t).Session()
	if _, err := session.Insert(testContext(t), "posts",
		postRow(1, "first", []string{"rust", "db"}, []int64{3, 1}),
		postRow(2, "second", []string{"db"}, nil),
		postRow(3, "third", []string{}, []int64{}),
	); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	return session
}

func postAt(t *testing.T, session *slate.Session, id uint64) []slate.Value {
	t.Helper()
	row, found, err := session.Get(testContext(t), "posts", []slate.Value{slate.Uint(id)})
	if err != nil {
		t.Fatalf("getting %d: %v", id, err)
	}
	if !found {
		t.Fatalf("row %d is not there", id)
	}
	return row
}

func TestAnArrayRoundTripsThroughTheServer(t *testing.T) {
	session := posted(t)
	row := postAt(t, session, 1)

	tags, ok := row[2].(slate.Array)
	if !ok {
		// Not a t.Errorf and a continue: an array coming back as something
		// else is the whole failure this file is about, and every assertion
		// after it would be about the wrong type.
		t.Fatalf("tags came back as %T, not slate.Array", row[2])
	}
	if len(tags) != 2 {
		t.Fatalf("tags has %d elements, want 2", len(tags))
	}
	if tags[0] != slate.String("rust") || tags[1] != slate.String("db") {
		t.Fatalf("tags = %v, want [rust db]", tags)
	}

	scores, ok := row[3].(slate.Array)
	if !ok {
		t.Fatalf("scores came back as %T, not slate.Array", row[3])
	}
	// The element's type, not only its number. An `i64` arriving as a `u64`
	// would print the same and would not compare equal to the stored value.
	if _, ok := scores[0].(slate.Int); !ok {
		t.Fatalf("an i64 element came back as %T", scores[0])
	}
	if scores[0] != slate.Int(3) || scores[1] != slate.Int(1) {
		t.Fatalf("scores = %v, want [3 1]", scores)
	}
}

func TestAnEmptyArrayIsNotANull(t *testing.T) {
	// The distinction a list type loses first: "no scores" and "scores
	// unknown" are different values, and they encode differently — the array
	// tag and a terminator against a bare null tag.
	session := posted(t)

	third := postAt(t, session, 3)
	empty, ok := third[3].(slate.Array)
	if !ok {
		t.Fatalf("an empty array came back as %T", third[3])
	}
	if len(empty) != 0 {
		t.Fatalf("an empty array came back with %d elements", len(empty))
	}

	second := postAt(t, session, 2)
	if _, ok := second[3].(slate.Null); !ok {
		t.Fatalf("a null array came back as %T, not slate.Null", second[3])
	}
}

func TestAnElementOfTheWrongTypeIsRefused(t *testing.T) {
	// `slate.Array` cannot express its element type — that lives on the
	// column, the way a decimal's scale does — so a mixed list compiles. The
	// server is the only thing that knows, and it must say which element.
	session := posted(t)
	_, err := session.Insert(testContext(t), "posts",
		[]slate.Value{
			slate.Uint(9),
			slate.String("bad"),
			slate.Array{slate.String("ok"), slate.Int(7)},
			slate.Null{},
		})
	if err == nil {
		t.Fatal("an i64 in an array of strings was accepted")
	}
	if !strings.Contains(err.Error(), "element 1") {
		t.Fatalf("the refusal should name which element: %v", err)
	}
}

func TestANestedArrayIsRefused(t *testing.T) {
	// Refused by the server before the kernel sees it: the depth is chosen by
	// whoever sends the message, and refusing at depth one means there is no
	// depth to bound.
	session := posted(t)
	_, err := session.Insert(testContext(t), "posts",
		[]slate.Value{
			slate.Uint(10),
			slate.String("nested"),
			slate.Array{slate.Array{slate.String("inner")}},
			slate.Null{},
		})
	if err == nil {
		t.Fatal("an array of arrays was accepted")
	}
}
