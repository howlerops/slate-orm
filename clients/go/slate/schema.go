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
)

// ColumnDef is one column of a [TableDef].
type ColumnDef struct {
	Name string
	Type ColumnType
	// Scale is the digits after the decimal point, for a [TypeDecimal] column.
	// Zero and meaningless for every other type.
	//
	// Deliberately not part of the fingerprint, because the server does not
	// hash it either: a scale addresses no column, so a client that has it
	// wrong still reaches the right one. It is here for rendering — see
	// [Units.StringWithScale], and it is part of the fingerprint: the one
	// property in it that addresses no column. A client with an ordinal wrong
	// reads the wrong column and usually notices; one with a scale wrong reads
	// the *right* column and renders every value a power of ten out, for ever,
	// with nothing anywhere reporting it.
	Scale int
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
