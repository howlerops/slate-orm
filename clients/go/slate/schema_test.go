package slate_test

import (
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// docs, as the harness's fixture actually declares it.
func docsTable() slate.TableDef {
	return slate.TableDef{
		Name: "docs",
		Columns: []slate.ColumnDef{
			{Name: "id", Type: slate.TypeUint},
			{Name: "kind", Type: slate.TypeString},
			{Name: "size", Type: slate.TypeInt},
		},
		PrimaryKey: []string{"id"},
	}
}

// A correct declaration is accepted, and every path carries it.
//
// Worth checking all four rather than one: the claim is attached at ten call
// sites, and a missed one is invisible — the request simply is not checked.
func TestACorrectDeclarationIsAccepted(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Declaring(slate.Schemas{"docs": docsTable()}).Session()
	ctx := testContext(t)

	row := []slate.Value{slate.Uint(1), slate.String("note"), slate.Int(10)}
	if _, err := session.Insert(ctx, "docs", row); err != nil {
		t.Fatalf("insert with a correct declaration: %v", err)
	}
	if _, _, err := session.Get(ctx, "docs", []slate.Value{slate.Uint(1)}); err != nil {
		t.Fatalf("get: %v", err)
	}
	updated := []slate.Value{slate.Uint(1), slate.String("edited"), slate.Int(11)}
	if _, err := session.Update(ctx, "docs", updated); err != nil {
		t.Fatalf("update: %v", err)
	}
	if _, err := session.Delete(ctx, "docs", []slate.Value{slate.Uint(1)}); err != nil {
		t.Fatalf("delete: %v", err)
	}
}

// The mistake ordinals make easy, and the reason this feature exists.
//
// A client that swaps two columns in its declaration reads `kind` where the
// table has `size`. Without a declaration that is silent and permanent: every
// row comes back with the fields transposed and nothing says so. With one, the
// first request is refused.
func TestASwappedColumnOrderIsRefused(t *testing.T) {
	server := start(t, "")
	swapped := docsTable()
	swapped.Columns[1], swapped.Columns[2] = swapped.Columns[2], swapped.Columns[1]

	session := server.client(t).Declaring(slate.Schemas{"docs": swapped}).Session()
	_, err := session.Insert(testContext(t), "docs",
		[]slate.Value{slate.Uint(1), slate.Int(10), slate.String("note")})

	if err == nil {
		t.Fatal("a declaration with the columns in the wrong order must be refused")
	}
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Errorf("err = %v, want KindInvalidRequest", err)
	}
}

func TestAWrongColumnNameIsRefused(t *testing.T) {
	server := start(t, "")
	renamed := docsTable()
	renamed.Columns[1].Name = "category"

	session := server.client(t).Declaring(slate.Schemas{"docs": renamed}).Session()
	_, err := session.Insert(testContext(t), "docs",
		[]slate.Value{slate.Uint(1), slate.String("note"), slate.Int(10)})
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Errorf("err = %v, want KindInvalidRequest", err)
	}
}

func TestAWrongColumnTypeIsRefused(t *testing.T) {
	server := start(t, "")
	retyped := docsTable()
	retyped.Columns[2].Type = slate.TypeUint // the table says i64

	session := server.client(t).Declaring(slate.Schemas{"docs": retyped}).Session()
	_, err := session.Insert(testContext(t), "docs",
		[]slate.Value{slate.Uint(1), slate.String("note"), slate.Int(10)})
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Errorf("err = %v, want KindInvalidRequest", err)
	}
}

// A table with no declaration is served as it always was.
//
// The check is opt-in per table, so declaring `docs` must not start refusing
// requests for a table the caller said nothing about.
func TestAnUndeclaredTableIsUnaffected(t *testing.T) {
	server := start(t, `
[[tables]]
name = "other"
id = 7
columns = [{ name = "id", type = "u64" }]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["other"]
actions = ["everything"]
`)
	session := server.client(t).Declaring(slate.Schemas{"docs": docsTable()}).Session()
	if _, err := session.Insert(testContext(t), "other", []slate.Value{slate.Uint(1)}); err != nil {
		t.Fatalf("an undeclared table should be unchecked: %v", err)
	}
}

// The fingerprint must be the *server's*, not merely self-consistent.
//
// A client whose hash is internally consistent and different from the server's
// refuses every request, and the tests above would not notice — they assert
// that a wrong declaration is refused, and a wrong *hash* refuses everything
// including the right ones. Only `TestACorrectDeclarationIsAccepted` covers
// that, and it covers it by accident.
//
// So the value is pinned. It is not this implementation's own output written
// down: it is what the Python client — a separate port, already checked against
// a live server — computes for the same table. Two independent ports agreeing
// on a constant is evidence; one port agreeing with itself is not.
func TestTheFingerprintMatchesTheServers(t *testing.T) {
	// docs {id u64, kind str, size i64}, primary key (id).
	//
	//	>>> from slate.schema import fingerprint_of
	//	>>> hex(fingerprint_of(DOCS))
	//	'0x97c3c1256af4cfdb'
	const canonical uint64 = 0x97c3_c125_6af4_cfdb
	if got := docsTable().Fingerprint(); got != canonical {
		t.Errorf("fingerprint = %#016x, the canonical form is %#016x", got, canonical)
	}
}

// A key naming a column the declaration does not have must not collide with a
// correct declaration whose key is the first column.
//
// The obvious implementation hashes the miss as ordinal 0, which makes
// `primary_key: ["nope"]` hash identically to `primary_key: ["id"]` — a broken
// declaration that the server accepts.
func TestAKeyNamingAMissingColumnDoesNotCollide(t *testing.T) {
	correct := docsTable()
	broken := docsTable()
	broken.PrimaryKey = []string{"nope"}

	if correct.Fingerprint() == broken.Fingerprint() {
		t.Error("a key naming a column that does not exist hashes as ordinal 0")
	}
}

// A *read* against a misdeclared table is refused too.
//
// The first version of this feature checked only the five requests with a
// top-level `SchemaCheck` field — insert, upsert, update, delete, get — and
// left every read unchecked. That is the larger half of the exposure: a client
// that reads a table with its columns transposed gets transposed rows on every
// query, which is precisely the silent corruption the check exists to stop.
//
// The claim rides on the `Query` message, so it covers `Query`, `Explain`,
// `Join` and `Aggregate` alike.
func TestAMisdeclaredTableIsRefusedOnRead(t *testing.T) {
	server := start(t, "")
	swapped := docsTable()
	swapped.Columns[1], swapped.Columns[2] = swapped.Columns[2], swapped.Columns[1]
	session := server.client(t).Declaring(slate.Schemas{"docs": swapped}).Session()
	ctx := testContext(t)

	stream, err := session.Query(ctx, slate.Query{Table: "docs"})
	if err == nil {
		_, err = stream.Collect()
	}
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Errorf("a query against a misdeclared table: err = %v, want KindInvalidRequest", err)
	}

	if _, err := session.Explain(ctx, slate.Query{Table: "docs"}); !slate.IsKind(
		err, slate.KindInvalidRequest,
	) {
		t.Errorf("explain against a misdeclared table: err = %v", err)
	}
}

// And a correct declaration still reads.
func TestACorrectDeclarationStillReads(t *testing.T) {
	server := start(t, "")
	session := server.client(t).Declaring(slate.Schemas{"docs": docsTable()}).Session()
	ctx := testContext(t)

	if _, err := session.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(1), slate.String("note"), slate.Int(10)}); err != nil {
		t.Fatalf("seeding: %v", err)
	}
	stream, err := session.Query(ctx, slate.Query{Table: "docs"})
	if err != nil {
		t.Fatalf("querying: %v", err)
	}
	rows, err := stream.Collect()
	if err != nil {
		t.Fatalf("draining: %v", err)
	}
	if len(rows) != 1 {
		t.Errorf("got %d rows, want 1", len(rows))
	}
}

// A renamed column is accepted under its previous spelling.
//
// The ledger entry that added these checks recorded the opposite —
// "neither client accepts a renamed column's previous spelling, which the
// server does accept ... where the Python client would be served" — on the
// reasoning that `TableDef` has nowhere to record a previous name.
//
// It has nowhere to record one and does not need one. A client declares the
// spelling *it* uses; the server enumerates every spelling the catalog would
// accept (`fingerprint::accepted`, a product over each column's renames) and
// compares. So the old name passes, the new name passes, and no client models
// renames at all — Python included, which the entry claimed was better off.
// Withdrawn, and this is what withdraws it.
func TestARenamedColumnIsAcceptedUnderItsPreviousName(t *testing.T) {
	const renamedTable = `
[[tables]]
name = "papers"
id = 40
columns = [
  { name = "id",   type = "u64" },
  { name = "kind", type = "str", previous_names = ["category"] },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["papers"]
actions = ["everything"]
`
	server := start(t, renamedTable)

	old := slate.TableDef{
		Name: "papers",
		Columns: []slate.ColumnDef{
			{Name: "id", Type: slate.TypeUint},
			{Name: "category", Type: slate.TypeString}, // the previous spelling
		},
		PrimaryKey: []string{"id"},
	}
	current := old
	current.Columns = []slate.ColumnDef{
		{Name: "id", Type: slate.TypeUint},
		{Name: "kind", Type: slate.TypeString},
	}

	for name, table := range map[string]slate.TableDef{"previous": old, "current": current} {
		session := server.client(t).Declaring(slate.Schemas{"papers": table}).Session()
		row := []slate.Value{slate.Uint(1), slate.String("note")}
		if _, err := session.Insert(testContext(t), "papers", row); err != nil {
			t.Errorf("declaring the %s spelling: %v", name, err)
		}
		if _, err := session.Delete(testContext(t), "papers", []slate.Value{slate.Uint(1)}); err != nil {
			t.Errorf("cleaning up after the %s spelling: %v", name, err)
		}
	}

	// The control: a name the table never had, current or previous.
	never := old
	never.Columns = []slate.ColumnDef{
		{Name: "id", Type: slate.TypeUint},
		{Name: "genre", Type: slate.TypeString},
	}
	session := server.client(t).Declaring(slate.Schemas{"papers": never}).Session()
	_, err := session.Insert(testContext(t), "papers",
		[]slate.Value{slate.Uint(2), slate.String("note")})
	if !slate.IsKind(err, slate.KindInvalidRequest) {
		t.Errorf("a name the table never had was accepted: %v", err)
	}
}

// A decimal's scale is part of the fingerprint, and it is the one property in
// there that addresses no column.
//
// Every other excluded property — nullability, DEFAULT, CHECK, an index — is
// excluded because getting it wrong does not make a client read the wrong
// column. A scale fails that test too, and is included anyway, because the
// failure it prevents is worse than the one the test is about: a wrong ordinal
// reads the wrong column and usually shows, a wrong scale reads the *right*
// column and renders every value a power of ten out, for ever, with nothing
// anywhere reporting it. The wire carries units and never the scale, so this
// hash is the only place it can be caught.
//
// Pinned against the Python client's output, as the DOCS value above is.
func TestADecimalsScaleIsPartOfTheFingerprint(t *testing.T) {
	priced := func(scale int) slate.TableDef {
		return slate.TableDef{
			Name: "prices",
			Columns: []slate.ColumnDef{
				{Name: "id", Type: slate.TypeUint},
				{Name: "label", Type: slate.TypeString},
				{Name: "amount", Type: slate.TypeDecimal, Scale: scale},
			},
			PrimaryKey: []string{"id"},
		}
	}
	//	>>> hex(fingerprint_of(PRICES))    # amount at scale 2
	//	'0xdab8856481bc4a6d'
	//	>>> hex(fingerprint_of(WRONG))     # the same table at scale 4
	//	'0xdaba08fbb666133f'
	const atTwo uint64 = 0xdab8_8564_81bc_4a6d
	const atFour uint64 = 0xdaba_08fb_b666_133f
	if got := priced(2).Fingerprint(); got != atTwo {
		t.Errorf("scale 2 = %#016x, the canonical form is %#016x", got, atTwo)
	}
	if got := priced(4).Fingerprint(); got != atFour {
		t.Errorf("scale 4 = %#016x, the canonical form is %#016x", got, atFour)
	}
	if priced(2).Fingerprint() == priced(4).Fingerprint() {
		t.Error("two scales hashed alike, which is the whole point")
	}
}

// AsView renames a declaration and copies what it shares.
//
// The copy is the point: `TableDef` holds slices, and a view built by
// reference would let an append to either declaration's `Columns` be seen by
// the other — which produces a wrong ordinal, and a silently mis-decoded row
// rather than an error.
func TestAsViewCopiesWhatItShares(t *testing.T) {
	base := docsTable()
	view := base.AsView("recent_docs")

	if view.Name != "recent_docs" {
		t.Fatalf("name = %q, want recent_docs", view.Name)
	}
	if len(view.Columns) != len(base.Columns) {
		t.Fatalf("columns = %d, want %d", len(view.Columns), len(base.Columns))
	}
	for i := range base.Columns {
		if view.Columns[i] != base.Columns[i] {
			t.Fatalf("column %d = %+v, want %+v", i, view.Columns[i], base.Columns[i])
		}
		got, ok := view.Ordinal(base.Columns[i].Name)
		if !ok || int(got) != i {
			t.Fatalf("ordinal of %q = %d %v, want %d true", base.Columns[i].Name, got, ok, i)
		}
	}

	// A view may not narrow columns, so its fingerprint over the same columns
	// is the base table's claim under another name — the server checks it that
	// way, and a divergence here would be a claim the server rejects.
	view.Columns[0].Name = "mutated"
	if base.Columns[0].Name == "mutated" {
		t.Fatal("writing the view's column changed the base table's: the slice is shared")
	}
}
