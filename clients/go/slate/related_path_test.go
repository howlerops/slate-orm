package slate_test

import (
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// Three tables and two foreign keys, so a relationship *path* has somewhere to
// go: libraries -> shelves -> copies.
//
// A path needs two keys where a relationship needs one, which is the whole
// reason this fixture is not `relatedTables` with a row added.
const pathTables = `
[[tables]]
name = "libraries"
id = 20
columns = [
  { name = "id",   type = "u64" },
  { name = "name", type = "str" },
]
primary_key = ["id"]

[[tables]]
name = "shelves"
id = 21
# "region" sits in front of "id" on purpose, so that shelves.id is ordinal 1
# rather than 0.
#
# The second level's key_ordinal is the primary key of the level above, and
# with "id" at ordinal 0 the number a client must read off the wire is the same
# number it would get by hardcoding zero. A mutation that ignored key_ordinal
# entirely passed every test in this file until this column existed - it was
# caught only by the Python suite, whose fixture is tenant-scoped and so has
# "id" at ordinal 1 by accident. A fixture where the right answer is zero
# cannot tell "read the ordinal" from "assume the first column".
columns = [
  { name = "region",     type = "str" },
  { name = "id",         type = "u64" },
  { name = "library_id", type = "u64" },
  { name = "label",      type = "str" },
]
primary_key = ["id"]

[[tables.foreign_keys]]
name = "shelf_library"
parent = "libraries"
columns = ["library_id"]

[[tables]]
name = "copies"
id = 22
columns = [
  { name = "id",       type = "u64" },
  { name = "shelf_id", type = "u64" },
  { name = "barcode",  type = "str" },
]
primary_key = ["id"]

[[tables.foreign_keys]]
name = "copy_shelf"
parent = "shelves"
columns = ["shelf_id"]

[[security.grants]]
role = "app"
tables = ["libraries", "shelves", "copies"]
actions = ["everything"]
`

// stacked seeds two libraries, four shelves and four copies.
//
// Shelf 103 is deliberately bare. A middle row with nothing below it is the
// case that separates "the rows at the bottom level" from "the rows with no
// children", and those two readings agree on every other input — the Python
// client shipped the second one for an hour and handed back a shelf where a
// copy was asked for.
func stacked(t *testing.T) *slate.Session {
	t.Helper()
	session := start(t, pathTables).client(t).Session()
	ctx := testContext(t)
	for _, library := range [][]slate.Value{
		{slate.Uint(1), slate.String("main")},
		{slate.Uint(2), slate.String("annexe")},
	} {
		if _, err := session.Insert(ctx, "libraries", library); err != nil {
			t.Fatalf("seeding libraries: %v", err)
		}
	}
	for _, shelf := range [][]slate.Value{
		{slate.String("north"), slate.Uint(100), slate.Uint(1), slate.String("alpha")},
		{slate.String("north"), slate.Uint(101), slate.Uint(1), slate.String("beta")},
		{slate.String("south"), slate.Uint(102), slate.Uint(2), slate.String("gamma")},
		{slate.String("south"), slate.Uint(103), slate.Uint(2), slate.String("bare")},
	} {
		if _, err := session.Insert(ctx, "shelves", shelf); err != nil {
			t.Fatalf("seeding shelves: %v", err)
		}
	}
	for _, copy := range [][]slate.Value{
		{slate.Uint(200), slate.Uint(100), slate.String("a-1")},
		{slate.Uint(201), slate.Uint(100), slate.String("a-2")},
		{slate.Uint(202), slate.Uint(101), slate.String("b-1")},
		{slate.Uint(203), slate.Uint(102), slate.String("g-1")},
	} {
		if _, err := session.Insert(ctx, "copies", copy); err != nil {
			t.Fatalf("seeding copies: %v", err)
		}
	}
	return session
}

func libraryPath() []slate.Step {
	return []slate.Step{
		{Relation: slate.Relation{On: "shelves", Through: "shelf_library", Way: slate.Children},
			Table: "shelves"},
		{Relation: slate.Relation{On: "copies", Through: "copy_shelf", Way: slate.Children},
			Table: "copies"},
	}
}

func textAt(t *testing.T, row []slate.Value, at int) string {
	t.Helper()
	text, ok := row[at].(slate.String)
	if !ok {
		t.Fatalf("value %d is not a string: %v", at, row[at])
	}
	return string(text)
}

func TestRelatedPathReturnsATreePerParent(t *testing.T) {
	session := stacked(t)
	trees, err := session.RelatedPath(
		testContext(t),
		libraryPath(),
		[]slate.Value{slate.Uint(1)},
		[]slate.Value{slate.Uint(2)},
	)
	if err != nil {
		t.Fatalf("RelatedPath: %v", err)
	}
	if len(trees) != 2 {
		t.Fatalf("got %d trees, want one per key", len(trees))
	}

	var shelves []string
	var barcodes []string
	for _, tree := range trees {
		for _, node := range tree {
			shelves = append(shelves, textAt(t, node.Row, 3))
			for _, leaf := range node.Related {
				barcodes = append(barcodes, textAt(t, leaf.Row, 2))
			}
		}
	}
	if got := strings.Join(shelves, ","); got != "alpha,beta,gamma,bare" {
		t.Fatalf("shelves = %q", got)
	}
	if got := strings.Join(barcodes, ","); got != "a-1,a-2,b-1,g-1" {
		t.Fatalf("barcodes = %q", got)
	}
}

func TestRelatedThroughDropsTheMiddleLevel(t *testing.T) {
	session := stacked(t)
	through, err := session.RelatedThrough(
		testContext(t),
		libraryPath(),
		[]slate.Value{slate.Uint(1)},
		[]slate.Value{slate.Uint(2)},
	)
	if err != nil {
		t.Fatalf("RelatedThrough: %v", err)
	}
	// Library 2's bare shelf contributes nothing at all — not itself, which is
	// what "nodes with no children" would have returned.
	want := [][]string{{"a-1", "a-2", "b-1"}, {"g-1"}}
	for at, rows := range through {
		got := make([]string, 0, len(rows))
		for _, row := range rows {
			got = append(got, textAt(t, row, 2))
		}
		if strings.Join(got, ",") != strings.Join(want[at], ",") {
			t.Fatalf("parent %d: got %v, want %v", at, got, want[at])
		}
	}
}

func TestAParentWithNothingRelatedGetsAnEmptyTree(t *testing.T) {
	session := stacked(t)
	trees, err := session.RelatedPath(
		testContext(t),
		libraryPath(),
		[]slate.Value{slate.Uint(9)},
	)
	if err != nil {
		t.Fatalf("RelatedPath: %v", err)
	}
	if len(trees) != 1 || len(trees[0]) != 0 {
		t.Fatalf("an unrelated key should give one empty tree, got %v", trees)
	}
}

func TestAnEmptyPathIsRefusedByTheClient(t *testing.T) {
	session := stacked(t)
	// Refused here rather than at the server: a path of no steps has no answer
	// shape, and the round trip would only confirm it.
	_, err := session.RelatedPath(testContext(t), nil, []slate.Value{slate.Uint(1)})
	if err == nil {
		t.Fatal("an empty path should be refused")
	}
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Fatalf("want KindInvalidRequest, got %v", err)
	}
}

func TestAPathPastTheDepthLimitIsRefusedByName(t *testing.T) {
	session := stacked(t)
	// The shipped limit is four; six steps is past it. The path also does not
	// compose, and the depth is checked first on purpose — the limit bounds
	// the resolution that would otherwise happen.
	deep := append(append(libraryPath(), libraryPath()...), libraryPath()...)
	_, err := session.RelatedPath(testContext(t), deep, []slate.Value{slate.Uint(1)})
	if err == nil {
		t.Fatal("a path past the depth limit should be refused")
	}
	if !strings.Contains(err.Error(), "max_relation_depth") {
		t.Fatalf("the refusal should name the knob, got %v", err)
	}
}
