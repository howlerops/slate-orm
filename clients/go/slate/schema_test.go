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
