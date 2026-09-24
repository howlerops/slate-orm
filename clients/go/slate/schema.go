package slate

import (
	"strconv"

	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
)

// ColumnType is a column's declared type, as the catalog spells it.
type ColumnType string

// The column types.
const (
	TypeBool   ColumnType = "bool"
	TypeBytes  ColumnType = "bytes"
	TypeString ColumnType = "string"
	TypeInt    ColumnType = "i64"
	TypeUint   ColumnType = "u64"
	TypeFloat  ColumnType = "f64"
	TypeUUID   ColumnType = "uuid"
	TypeVector ColumnType = "vector"
	// TypeDecimal is an exact decimal. The scale is not part of the type; see
	// [ColumnDef.Scale].
	TypeDecimal ColumnType = "decimal"
	// TypeArray is a homogeneous list. The element type is not part of the
	// type; see [ColumnDef.Element].
	TypeArray ColumnType = "array"
)

// ColumnDef is one column of a [TableDef].
type ColumnDef struct {
	Name string
	Type ColumnType
	// Scale is the digits after the decimal point, for a [TypeDecimal] column.
	// Zero and meaningless for every other type.
	//
	// It is used for rendering — see [Units.StringWithScale] — and it is part
	// of the fingerprint: the one property in it that addresses no column. A
	// client with an ordinal wrong reads the wrong column and usually notices;
	// one with a scale wrong reads the *right* column and renders every value
	// a power of ten out, for ever, with nothing anywhere reporting it. The
	// wire carries units and never the scale, so the fingerprint is the only
	// place this can be caught.
	Scale int
	// Element is what a [TypeArray] column's elements are. Empty and
	// meaningless for every other type.
	//
	// In the fingerprint, by exactly the argument [ColumnDef.Scale] gives one
	// type over: it addresses no column, and a client that has it wrong reads
	// the *right* column and decodes every element as the wrong type, with
	// the wire carrying no element type to notice by.
	Element ColumnType
}

// TableDef is this client's declaration of a table.
//
// Optional. Everything in this package works without one — the wire carries
// ordinals, and this client resolves nothing. What a declaration buys is the
// *check*: a request that carries one is refused if the server's catalog
// disagrees, instead of being answered with the wrong column.
//
// That matters most for the mistake ordinals make easy. Declaring
// `{id, title, year}` for a table that is really `{id, year, title}` produces
// a client that reads titles as years, silently and forever. With a
// declaration the first request says so.
type TableDef struct {
	// Name as the catalog spells it.
	Name string
	// Columns in ordinal order. `Columns[2]` *is* ordinal 2.
	Columns []ColumnDef
	// PrimaryKey names the key columns, in key order.
	PrimaryKey []string
}

// Ordinal is the position of a named column, and whether it exists.
func (t TableDef) Ordinal(name string) (Ordinal, bool) {
	for index, column := range t.Columns {
		if column.Name == name {
			return Ordinal(index), true
		}
	}
	return 0, false
}

// AsView returns this table's columns and key under a view's name.
//
// A view may not narrow columns — `docs/views.md` refuses a projection,
// because it would make the caller's ordinals *view* ordinals rather than the
// base table's — so a view's declaration is exactly its base table's with the
// name changed, and the server checks a claim about a view under the view's
// own name against those same columns.
//
// The generated file builds its views this way already, inline. Owning the
// construction here lets a hand-written declaration name a view without
// knowing that a view's ordinals are its base table's, which is the fact most
// easily got wrong and the one that mis-decodes a row rather than erroring.
//
// Columns is copied: a TableDef holds a slice, and two declarations sharing
// one backing array would let a later append to either be seen by both.
func (t TableDef) AsView(name string) TableDef {
	columns := make([]ColumnDef, len(t.Columns))
	copy(columns, t.Columns)
	key := make([]string, len(t.PrimaryKey))
	copy(key, t.PrimaryKey)
	return TableDef{Name: name, Columns: columns, PrimaryKey: key}
}

// FNV-1a, 64-bit.
const (
	fnvOffset uint64 = 0xcbf2_9ce4_8422_2325
	fnvPrime  uint64 = 0x0000_0100_0000_01b3
)

type fnv uint64

func (h *fnv) bytes(data []byte) {
	for _, b := range data {
		*h ^= fnv(b)
		*h = fnv(uint64(*h) * fnvPrime)
	}
}

// text writes a string length-prefixed rather than delimited, so that no
// column name can be spelled to look like the end of a field — `a\nb` and two
// columns must not hash alike.
func (h *fnv) text(s string) {
	h.bytes([]byte(strconv.Itoa(len(s))))
	h.bytes([]byte(":"))
	h.bytes([]byte(s))
}

func (h *fnv) number(n int) {
	h.bytes([]byte(strconv.Itoa(n)))
	h.bytes([]byte(";"))
}

// Fingerprint is this client's claim about the table, in the server's
// canonical form.
//
// Only what a client can address and can be wrong about: the table's name, and
// per ordinal the column's name and declared type, plus the primary key and
// the column count. Nullability, `DEFAULT`, `CHECK`, foreign keys and indexes
// address no column and are deliberately absent — hashing them would make an
// unrelated migration break every client, which is the failure mode that makes
// a fingerprint worse than none.
func (t TableDef) Fingerprint() uint64 {
	h := fnv(fnvOffset)
	h.bytes([]byte("slate.v1.schema/1"))
	h.text(t.Name)
	for ordinal, column := range t.Columns {
		h.number(ordinal)
		h.text(column.Name)
		h.text(string(column.Type))
		// A decimal's scale, and only a decimal's. It addresses no column --
		// the test every other excluded property fails -- and is hashed anyway
		// because the failure it prevents is worse: a wrong ordinal reads the
		// wrong column and usually shows, a wrong scale reads the right column
		// and renders every value a power of ten out, for ever, with nothing
		// anywhere reporting it. The wire carries units and never the scale.
		if column.Type == TypeArray {
			h.text(string(column.Element))
		}
		if column.Type == TypeDecimal {
			h.number(column.Scale)
		}
	}
	h.bytes([]byte("key"))
	h.number(len(t.PrimaryKey))
	for _, name := range t.PrimaryKey {
		// A key naming a column this declaration does not have is a broken
		// declaration. Hashing the miss as ordinal 0 would make it collide
		// with a correct declaration whose key is the first column, so it
		// hashes as a value no real ordinal takes.
		ordinal, ok := t.Ordinal(name)
		if !ok {
			h.bytes([]byte("?"))
			continue
		}
		h.number(int(ordinal))
	}
	h.bytes([]byte("columns"))
	h.number(len(t.Columns))
	return uint64(h)
}

func (t TableDef) claim() *pb.SchemaCheck {
	return &pb.SchemaCheck{
		Columns:     uint32(len(t.Columns)),
		Fingerprint: t.Fingerprint(),
	}
}

// Schemas is a client's declarations, by table name.
//
// Held on the [Client] so a query names a table by string and the check rides
// along without every call site remembering it. A table with no declaration
// sends no claim and is served as it always was — the check is opt-in per
// table, which is what lets a caller declare the two tables they care about
// getting right.
type Schemas map[string]TableDef

func (s Schemas) claimFor(table string) *pb.SchemaCheck {
	if s == nil {
		return nil
	}
	if def, ok := s[table]; ok {
		return def.claim()
	}
	return nil
}

// CheckRule is one `CHECK` constraint, as the catalog publishes it.
//
// Data, not behaviour. Nothing here evaluates a predicate: doing so would mean
// a second implementation of the server's expression language in Go, and two
// implementations of one rule disagree. This is what a caller shows a person
// before they submit, and what maps the `check` key in a refusal's details
// back to a field.
//
// Empty strings rather than pointers for the absent cases. A check about two
// columns has no `Column` and most have no `Message`; Go's zero value says
// that adequately and a `*string` would make every read a nil check.
type CheckRule struct {
	// The column the rule is about, or "" when it is about several.
	Column string
	// The sentence to show a person, or "" when the schema wrote none.
	Message string
	// The text the predicate was parsed from, or "" for a check built in Rust.
	Predicate string
}

// ForeignKey is one foreign key, as the catalog publishes it.
//
// Data, like [CheckRule], and for the same reason: nothing here enforces
// anything, because the server does. What it carries is the one fact a caller
// cannot derive — which table a [Relation] read as [Parents] answers with.
//
// [Relation] names a relationship by the child table and the key's name and
// stops there, deliberately: a client that described the relationship could
// describe it differently from the next client. But [Session.Related] also
// needs the table its rows decode as, which for [Parents] is the *parent* and
// is nowhere in the client. Before this it was a string the caller typed from
// memory.
//
// What the wrong one costs was measured rather than assumed, by making
// [ForeignKey.Answers] return the child either way and running the three-SDK
// conformance suite: **the server refuses it**. `Related` sends the named
// table's declaration, so the schema check sees `sales`' columns claimed for
// `books` and says so at length. That is the good failure and it is why this
// is a convenience rather than a fix for a silent bug — but it is only good
// while the two tables' declarations *differ*. Two that fingerprint alike
// would be decoded positionally against each other with nothing said.
type ForeignKey struct {
	// The key's name, which is what [Relation.Through] wants.
	Name string
	// The table holding the key. [Relation.On], either direction.
	Child string
	// The table it points at. The table [Parents] answers with.
	Parent string
	// "restrict" or "cascade", as the catalog spells it. Data only: the
	// server applies it and this client never does.
	OnDelete string
}

// Children reads the rows holding this key — a book's sales.
//
// Pairs with [ForeignKey.Child], which is the table the rows come back as.
func (k ForeignKey) Children() Relation {
	return Relation{On: k.Child, Through: k.Name, Way: Children}
}

// Parents reads the rows this key points at — a sale's book.
//
// Pairs with [ForeignKey.Parent], which is the table the rows come back as.
func (k ForeignKey) Parents() Relation {
	return Relation{On: k.Child, Through: k.Name, Way: Parents}
}

// Answers is the table a read this way decodes as.
//
// The whole reason this type exists: [Session.Related] takes the table
// separately because it does not hold the catalog, and getting it wrong reads
// one table's rows against another's ordinals.
func (k ForeignKey) Answers(way Way) string {
	if way == Parents {
		return k.Parent
	}
	return k.Child
}
