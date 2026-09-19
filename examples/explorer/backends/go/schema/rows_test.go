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
