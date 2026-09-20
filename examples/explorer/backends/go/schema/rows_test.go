// Hand-written, beside a generated file, and it must stay that way: a test
// the generator emitted would agree with the generator by construction.
//
// It exists because the Go and TypeScript decoders were shipped compiled but
// never *run*. `go build` and `go vet` prove they type-check; nothing proved
// they decode the right column, and the failure this whole generator exists to
// prevent — reading the neighbouring column — type-checks perfectly whenever
// the neighbour happens to share a type.
package schema

import (
	"os"
	"regexp"
	"strings"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// A well-formed `books` row: the ordinals the catalog declares, in order.
func bookRow() []slate.Value {
	return []slate.Value{
		slate.Uint(7),
		slate.Uint(3),
		slate.String("A Book In Flight"),
		slate.Int(2026),
		slate.Float(4.5),
		slate.Int(1_700_000_000),
		slate.Vector([]float32{0.1, 0.2, 0.3, 0.4}),
		slate.Units(1250),
	}
}

func TestScanBooksDecodesEveryColumn(t *testing.T) {
	book, err := ScanBooks(bookRow())
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if book.Id != 7 {
		t.Errorf("id = %d, want 7", book.Id)
	}
	if book.AuthorId != 3 {
		t.Errorf("author_id = %d, want 3", book.AuthorId)
	}
	if book.Title != "A Book In Flight" {
		t.Errorf("title = %q", book.Title)
	}
	if book.Year != 2026 {
		t.Errorf("year = %d, want 2026", book.Year)
	}
	if book.Rating != 4.5 {
		t.Errorf("rating = %v, want 4.5", book.Rating)
	}
	// A decimal is a count of the smallest unit; the scale lives in the
	// schema, so 1250 at scale 2 is 12.50 and this type does not know that.
	if book.Price != slate.Units(1250) {
		t.Errorf("price = %v, want 1250", book.Price)
	}
	if len(book.Embedding) != 4 {
		t.Errorf("embedding has %d dimensions, want 4", len(book.Embedding))
	}
}

// The reason the decoder asserts instead of casting.
//
// `id` and `author_id` are both `Uint`, so swapping them is invisible — that
// pair is checked separately below, by value. This swaps two columns of
// *different* types, which is the case a type assertion can catch and a plain
// cast cannot.
func TestScanBooksRefusesATransposedRow(t *testing.T) {
	row := bookRow()
	row[2], row[3] = row[3], row[2] // title <-> year
	_, err := ScanBooks(row)
	if err == nil {
		t.Fatal("a transposed row decoded without complaint")
	}
	if !strings.Contains(err.Error(), "books.title") {
		t.Errorf("error should name the column, got %q", err)
	}
}

// Two columns of the same type transposed cannot be detected, and the decoder
// does not pretend otherwise: it returns the values as they arrived.
//
// Asserted rather than left implicit, because "the decoder catches
// transposition" is exactly the kind of claim that grows in the retelling. It
// catches a *type* mismatch. Same-typed neighbours are what the ordinals in
// the generated declaration are for, and what the schema fingerprint protects.
func TestScanBooksCannotSeeASameTypedSwap(t *testing.T) {
	row := bookRow()
	row[0], row[1] = row[1], row[0] // id <-> author_id, both Uint
	book, err := ScanBooks(row)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if book.Id != 3 || book.AuthorId != 7 {
		t.Errorf("expected the swap to pass through, got id=%d author_id=%d", book.Id, book.AuthorId)
	}
}

func TestScanBooksRefusesAShortRow(t *testing.T) {
	_, err := ScanBooks(bookRow()[:4])
	if err == nil {
		t.Fatal("a short row decoded without complaint")
	}
	if !strings.Contains(err.Error(), "8 columns") {
		t.Errorf("error should say how many columns it wanted, got %q", err)
	}
}

func TestScanBooksRefusesANullInANonNullableColumn(t *testing.T) {
	row := bookRow()
	row[2] = slate.Null{}
	_, err := ScanBooks(row)
	if err == nil {
		t.Fatal("a null in a non-nullable column decoded without complaint")
	}
	if !strings.Contains(err.Error(), "not nullable") {
		t.Errorf("got %q", err)
	}
}

// The published constraint, generated from the catalog.
func TestBooksChecksCarriesTheRule(t *testing.T) {
	rule, ok := BooksChecks["year_is_positive"]
	if !ok {
		t.Fatalf("no such check, have %v", BooksChecks)
	}
	if rule.Column != "year" {
		t.Errorf("column = %q, want year", rule.Column)
	}
	if rule.Message == "" {
		t.Error("the check should carry the sentence a form shows")
	}
	if rule.Predicate != "year > 0" {
		t.Errorf("predicate = %q", rule.Predicate)
	}
}

// --- every other table ------------------------------------------------------
//
// `books` had all of the above and the other four had nothing: generated,
// compiled, `go vet`-ed and never executed. The demo reads `authors`, `sales`,
// `editions` and `shipments` through the client's untyped `Value`s, so a wrong
// ordinal in any of their decoders would have been found by nobody.
//
// One case each rather than the five `books` gets, because what differs
// between tables is the column list and not the decoder's shape — the
// generator emits one shape. What each case is really asserting is that *this
// table's* ordinals are the catalog's. The transposition and null cases stay
// on `books`, which is where the shape is checked.

// decoders is every generated `Scan…` with a row the catalog says is valid.
//
// `TestEveryGeneratedDecoderIsExercised` below fails if a `Scan…` exists in the
// generated file and is missing here, which is the point of the table: adding
// a table to `head.toml` regenerates a decoder, and a decoder nothing runs is
// how this gap opened in the first place.
var decoders = map[string]func(*testing.T){
	"Authors": checkAuthors,
	// `books` is already covered five ways above; this entry is here so the
	// coverage check below sees it, and running its happy path twice costs
	// nothing.
	"Books":     func(t *testing.T) { TestScanBooksDecodesEveryColumn(t) },
	"Sales":     checkSales,
	"Editions":  checkEditions,
	"Shipments": checkShipments,
}

func TestEveryGeneratedDecoderRuns(t *testing.T) {
	for name, run := range decoders {
		t.Run(name, run)
	}
}

func checkAuthors(t *testing.T) {
	got, err := ScanAuthors([]slate.Value{
		slate.Uint(3),
		slate.String("Ursula"),
		slate.String("US"),
		slate.Int(1929),
	})
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	want := Authors{Id: 3, Name: "Ursula", Country: "US", Born: 1929}
	if got != want {
		t.Errorf("got %+v, want %+v", got, want)
	}
	// `name` and `country` are both strings and adjacent, so the values are
	// distinguishable on purpose: a decoder reading ordinal 2 for `name` would
	// pass a test that used the same string for both.
}

func checkSales(t *testing.T) {
	got, err := ScanSales([]slate.Value{slate.Uint(11), slate.Uint(7), slate.Int(430)})
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	want := Sales{Id: 11, BookId: 7, Units: 430}
	if got != want {
		t.Errorf("got %+v, want %+v", got, want)
	}
}

func checkEditions(t *testing.T) {
	got, err := ScanEditions([]slate.Value{slate.Uint(5), slate.Uint(7), slate.String("paperback")})
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	want := Editions{Id: 5, BookId: 7, Format: "paperback"}
	if got != want {
		t.Errorf("got %+v, want %+v", got, want)
	}
}

func checkShipments(t *testing.T) {
	// The only generated Go decoder with a nullable column, so this is the
	// only place the `*int64` branch runs at all. Both ways round, because
	// "null becomes nil" and "a value becomes a pointer to it" are two
	// branches and a test of one says nothing about the other.
	live, err := ScanShipments([]slate.Value{
		slate.Uint(9), slate.Uint(7), slate.String("shipped"), slate.Null{},
	})
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if live.Id != 9 || live.BookId != 7 || live.Status != "shipped" {
		t.Errorf("got %+v", live)
	}
	if live.DeletedAt != nil {
		t.Errorf("deleted_at = %v, want nil", *live.DeletedAt)
	}

	retired, err := ScanShipments([]slate.Value{
		slate.Uint(9), slate.Uint(7), slate.String("shipped"), slate.Int(1_700_000_042),
	})
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	if retired.DeletedAt == nil || *retired.DeletedAt != 1_700_000_042 {
		t.Errorf("deleted_at = %v, want 1700000042", retired.DeletedAt)
	}
}

// TestEveryGeneratedDecoderIsExercised reads the generated file and fails if it
// declares a `Scan…` the table above does not run.
//
// The table is hand-written, which is the whole point — a test the generator
// emitted would agree with it by construction — and a hand-written list beside
// a generated file is the thing this repository has watched drift five times.
// Parsing the source is cheap and turns "somebody remembers" into a failure.
func TestEveryGeneratedDecoderIsExercised(t *testing.T) {
	source, err := os.ReadFile("schema.go")
	if err != nil {
		t.Fatalf("reading the generated file: %v", err)
	}
	declared := regexp.MustCompile(`(?m)^func Scan(\w+)\(`).FindAllStringSubmatch(string(source), -1)
	if len(declared) < 5 {
		t.Fatalf("found %d decoders in schema.go; the pattern is not matching", len(declared))
	}
	for _, match := range declared {
		if _, covered := decoders[match[1]]; !covered {
			t.Errorf("Scan%s is generated and nothing runs it; add it to `decoders`", match[1])
		}
	}
	for name := range decoders {
		if !regexp.MustCompile(`func Scan` + name + `\(`).Match(source) {
			t.Errorf("`decoders` names Scan%s, which schema.go no longer declares", name)
		}
	}
}

// TestShipmentsChecksCarriesTheEnumeration is the second published constraint,
// and the one codegen narrows a *type* from — `Status` is a plain `string` in
// Go, which has no union of string literals, so the values are only ever
// available as this predicate's text.
func TestShipmentsChecksCarriesTheEnumeration(t *testing.T) {
	rule, ok := ShipmentsChecks["status_known"]
	if !ok {
		t.Fatalf("no such check, have %v", ShipmentsChecks)
	}
	if rule.Column != "status" {
		t.Errorf("column = %q, want status", rule.Column)
	}
	for _, value := range []string{"pending", "shipped", "delivered"} {
		if !strings.Contains(rule.Predicate, value) {
			t.Errorf("the predicate %q does not mention %q", rule.Predicate, value)
		}
	}
}

// --- the generated foreign keys ---------------------------------------------

// TestSalesForeignKeyCarriesItsParent is the whole reason foreign keys are
// generated: `Parent` is the table a `Parents` read answers with, and it is the
// one fact a client holding a `Relation` cannot derive.
func TestSalesForeignKeyCarriesItsParent(t *testing.T) {
	key, ok := SalesForeignKeys["sale_book"]
	if !ok {
		t.Fatalf("no such key, have %v", SalesForeignKeys)
	}
	want := slate.ForeignKey{
		Name: "sale_book", Child: "sales", Parent: "books", OnDelete: "restrict",
	}
	if key != want {
		t.Fatalf("got %+v, want %+v", key, want)
	}
	// The pair the adapter actually uses, and the pair a transposition breaks:
	// a `Parents` read of this key answers with `books` and a `Children` read
	// with `sales`. Swapping them turns four conformance cases red with a
	// schema-check refusal, which was measured; this test is the fast version
	// of that, and the slow version is what says the adapter really uses it.
	if got := key.Answers(slate.Parents); got != "books" {
		t.Errorf("Answers(Parents) = %q, want books", got)
	}
	if got := key.Answers(slate.Children); got != "sales" {
		t.Errorf("Answers(Children) = %q, want sales", got)
	}
	if relation := key.Parents(); relation.On != "sales" || relation.Through != "sale_book" {
		t.Errorf("Parents() = %+v", relation)
	}
}

// TestEveryDeclaredForeignKeyIsGenerated reads the catalog's own output rather
// than this file's idea of it.
//
// A weaker check than the decoder one above — it asserts the *set*, not each
// key's parent — and it is here for the same reason: the demo's three keys are
// spread over three tables, and a fourth added to `head.toml` should not be
// generated into a file nothing looks at.
func TestEveryDeclaredForeignKeyIsGenerated(t *testing.T) {
	generated := map[string]slate.ForeignKey{}
	for _, table := range []map[string]slate.ForeignKey{
		SalesForeignKeys, EditionsForeignKeys, ShipmentsForeignKeys,
	} {
		for name, key := range table {
			generated[name] = key
		}
	}
	source, err := os.ReadFile("schema.go")
	if err != nil {
		t.Fatalf("reading the generated file: %v", err)
	}
	declared := regexp.MustCompile(`(?m)^var (\w+)ForeignKeys = `).FindAllStringSubmatch(string(source), -1)
	if len(declared) != 3 {
		t.Fatalf("found %d foreign-key maps in schema.go, want 3", len(declared))
	}
	if len(generated) != 3 {
		t.Fatalf("the three maps hold %d keys between them, want 3: %v", len(generated), generated)
	}
	// Every one of them points at `books`, which is what makes the demo able
	// to show a *path* — two steps in opposite directions through one parent.
	for name, key := range generated {
		if key.Parent != "books" {
			t.Errorf("%s points at %q; the demo's three all point at books", name, key.Parent)
		}
	}
}

// --- the write side ----------------------------------------------------------
//
// The encoders are the decoders' twin and arrived much later: until they
// existed, a caller built a row to insert as a positional list with nothing
// checking the order, which is the failure the decoders exist to catch, from
// the other end. Go's own types keep uint64 and int64 apart, so what is at
// risk here is the order rather than the tag.

func TestRowAttachesTheTagEachColumnDeclares(t *testing.T) {
	book, err := ScanBooks(bookRow())
	if err != nil {
		t.Fatalf("decoding: %v", err)
	}
	row := book.Row()
	if _, ok := row[0].(slate.Uint); !ok {
		t.Errorf("books.id encoded as %T, want slate.Uint", row[0])
	}
	if _, ok := row[3].(slate.Int); !ok {
		t.Errorf("books.year encoded as %T, want slate.Int", row[3])
	}
	if _, ok := row[7].(slate.Units); !ok {
		t.Errorf("books.price encoded as %T, want slate.Units", row[7])
	}
}

func TestRowWritesANullForAnAbsentValue(t *testing.T) {
	absent := Shipments{Id: 9, BookId: 7, Status: "shipped"}
	if _, ok := absent.Row()[3].(slate.Null); !ok {
		t.Errorf("an absent deleted_at encoded as %T, want slate.Null", absent.Row()[3])
	}
	at := int64(42)
	present := Shipments{Id: 9, BookId: 7, Status: "shipped", DeletedAt: &at}
	if got, ok := present.Row()[3].(slate.Int); !ok || int64(got) != 42 {
		t.Errorf("a present deleted_at encoded as %#v", present.Row()[3])
	}
}

// roundTrip holds one well-formed instance of each generated type.
//
// An oracle rather than a fixed expectation: each case agrees with an
// independent implementation — the decoder — rather than with a list somebody
// typed, so it catches an encoder ordinal nobody thought to assert on. What it
// cannot catch is both being wrong the same way, which the fixed cases above
// are for.
var roundTrip = map[string]func(t *testing.T){
	"Authors": func(t *testing.T) {
		want := Authors{Id: 3, Name: "Ursula", Country: "US", Born: 1929}
		got, err := ScanAuthors(want.Row())
		if err != nil || got != want {
			t.Errorf("round trip: got %#v, %v; want %#v", got, err, want)
		}
	},
	"Books": func(t *testing.T) {
		want, err := ScanBooks(bookRow())
		if err != nil {
			t.Fatalf("decoding: %v", err)
		}
		got, err := ScanBooks(want.Row())
		if err != nil {
			t.Fatalf("round trip: %v", err)
		}
		// A slice field makes Books uncomparable with ==, so the columns that
		// matter are checked one by one.
		if got.Id != want.Id || got.Title != want.Title || got.Year != want.Year ||
			got.Price != want.Price || len(got.Embedding) != len(want.Embedding) {
			t.Errorf("round trip: got %#v, want %#v", got, want)
		}
	},
	"Sales": func(t *testing.T) {
		want := Sales{Id: 11, BookId: 7, Units: 430}
		got, err := ScanSales(want.Row())
		if err != nil || got != want {
			t.Errorf("round trip: got %#v, %v; want %#v", got, err, want)
		}
	},
	"Editions": func(t *testing.T) {
		want := Editions{Id: 5, BookId: 7, Format: "paperback"}
		got, err := ScanEditions(want.Row())
		if err != nil || got != want {
			t.Errorf("round trip: got %#v, %v; want %#v", got, err, want)
		}
	},
	"Shipments": func(t *testing.T) {
		want := Shipments{Id: 9, BookId: 7, Status: "shipped"}
		got, err := ScanShipments(want.Row())
		if err != nil || got.Id != want.Id || got.Status != want.Status ||
			got.DeletedAt != nil {
			t.Errorf("round trip: got %#v, %v; want %#v", got, err, want)
		}
	},
}

func TestEveryRowSurvivesARoundTrip(t *testing.T) {
	for name, run := range roundTrip {
		t.Run(name, run)
	}
}

func TestEveryGeneratedEncoderIsExercised(t *testing.T) {
	// The same guard the decoders have: a sixth table gets an encoder, this
	// map is hand-written, and without a check the new one is covered by
	// nobody and nobody finds out.
	source, err := os.ReadFile("schema.go")
	if err != nil {
		t.Fatalf("reading the generated file: %v", err)
	}
	declared := regexp.MustCompile(`(?m)^func \(r (\w+)\) Row\(\)`).FindAllStringSubmatch(string(source), -1)
	if len(declared) < 5 {
		t.Fatalf("found %d encoders; the pattern is not matching", len(declared))
	}
	for _, match := range declared {
		if _, ok := roundTrip[match[1]]; !ok {
			t.Errorf("%s has a generated Row() and nothing runs it", match[1])
		}
	}
	if len(roundTrip) != len(declared) {
		t.Errorf("roundTrip has %d entries, schema.go declares %d", len(roundTrip), len(declared))
	}
}
