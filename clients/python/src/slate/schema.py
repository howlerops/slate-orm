"""A table, as this client understands it.

# The client has to hold a copy of the schema, and that is a real cost

The protocol publishes no schema, on purpose: "The catalog is the server's, and
a request names a table and its columns by the names and ordinals that catalog
already has. A client that could describe a table could describe one that is
not there." A `Describe` RPC was considered and rejected — two round trips, or
a cache that can go stale, and a stale width is a silently re-pointed
predicate.

The consequence for a client in another language is that it must state the
schema locally. `Table` is that statement. Two things follow, and both are
worth being honest about rather than hiding:

- **An ordinal is still a number the client got from somewhere.** `ColumnRef`
  removed the cross-table arithmetic; it did not remove the within-table
  ordinal, which is what `Table` maps a name onto. A `Table` that disagrees
  with the server's catalog points every reference past the disagreement at the
  wrong column, and the server cannot tell — it is a legitimate reference. On
  the Rust side the derive macro generates these constants from the same
  declaration the server uses, so they cannot disagree. There is no equivalent
  here.
- **So the declaration has to be checked against something.** This package
  ships `Table.check_against` and the test suite uses it: it reads one row of
  the real table and requires the width to match, which catches the column
  added or removed at the end. It does not catch a rename or a reorder that
  preserves the width, and it says so. That is a mitigation, not a fix; the fix
  would be a schema on the wire, which the protocol refuses.

`Table` deliberately produces no `ColumnRef`s. See `expr.Columns` for why.
"""

from __future__ import annotations

import dataclasses
from collections.abc import Sequence

from .types import ValueType

__all__ = ["Column", "Table"]


@dataclasses.dataclass(frozen=True)
class Column:
    """One column: its name and its declared type.

    The type is not decoration. The wire has both `int64_value` and
    `uint64_value`, the kernel orders values type first, and a predicate that
    sends the wrong one does not match — so this is what lets a caller write
    `c.size >= 15` instead of `c.size >= i64(15)`.
    """

    name: str
    type: ValueType
    #: Digits after the decimal point, for a `DECIMAL` column; zero and
    #: meaningless for every other type. Read it with `Table.scale_of`.
    #:
    #: Part of the fingerprint, and the one property in it that addresses no
    #: column. It is hashed because a wrong scale is worse than the mistakes
    #: that rule is about: a wrong ordinal reads the wrong column and shows,
    #: and a wrong scale reads the *right* column and renders every value a
    #: hundred times wrong, for ever, with no error at any layer. Safe to hash
    #: for a reason particular to a scale — changing one is already a refused
    #: migration, so it cannot invalidate a fleet the way hashing a `CHECK`
    #: would. See `fingerprint_of`, which hashes it only for a decimal, so a
    #: table without one is unaffected.
    scale: int = 0
    #: What an `ARRAY` column's elements are, and `None` for every other type.
    #:
    #: In the fingerprint, by exactly the argument `scale` gives one type over:
    #: it addresses no column, and a client that has it wrong reads the *right*
    #: column and decodes every element as the wrong type, with the wire
    #: carrying no element type to notice by.
    element: ValueType | None = None

    def __post_init__(self) -> None:
        if self.scale < 0:
            raise ValueError(f"column `{self.name}` declares a negative scale")
        # Both directions, as the server refuses both: an array with no
        # element type cannot say what it holds, and an element type on
        # anything else means the author believes that column is a list.
        if self.type is ValueType.ARRAY and self.element is None:
            raise ValueError(
                f"column `{self.name}` is an array and declares no element type; "
                "every value in an array column holds the same type"
            )
        if self.element is not None and self.type is not ValueType.ARRAY:
            raise ValueError(
                f"column `{self.name}` is {self.type.name.lower()} and has no elements; "
                "only an array column does"
            )
        if self.element is ValueType.ARRAY:
            raise ValueError(f"column `{self.name}` declares an array of arrays")
        if self.scale and self.type is not ValueType.DECIMAL:
            raise ValueError(
                f"column `{self.name}` is {self.type.name.lower()} and has no scale; "
                "only a decimal column does"
            )


@dataclasses.dataclass(frozen=True)
class Table:
    """A table's name, its columns in ordinal order, and its primary key.

    Ordinal order is the whole content of this object: `columns[2]` *is*
    ordinal 2, and every reference the client builds is that index. Declaring
    the columns in a different order from the server's catalog is the failure
    mode; see the module docstring.
    """

    name: str
    columns: tuple[Column, ...]
    primary_key: tuple[str, ...]

    def __init__(
        self,
        name: str,
        columns: Sequence[Column | tuple[str, ValueType]],
        primary_key: Sequence[str],
    ) -> None:
        resolved = tuple(
            c if isinstance(c, Column) else Column(c[0], c[1]) for c in columns
        )
        seen = [c.name for c in resolved]
        if len(set(seen)) != len(seen):
            raise ValueError(f"table `{name}` declares a column name twice: {seen}")
        for key in primary_key:
            if key not in seen:
                raise ValueError(
                    f"table `{name}` names `{key}` in its primary key, but has no such column"
                )
        object.__setattr__(self, "name", name)
        object.__setattr__(self, "columns", resolved)
        object.__setattr__(self, "primary_key", tuple(primary_key))

    def as_view(self, name: str) -> Table:
        """This table's columns and key, under a view's name.

        A view may not narrow columns — `docs/views.md` refuses a projection,
        because it would make the caller's ordinals *view* ordinals rather than
        the base table's — so a view's declaration is exactly its base table's
        with the name changed. The server checks a claim about a view under the
        view's own name against those same columns.

        The generated module builds its `VIEWS` this way already, inline. This
        is the same construction, owned by the library, so that a hand-written
        declaration can name a view without knowing that a view's ordinals are
        its base table's — which is the fact a caller most easily gets wrong,
        and the one that produces a silently mis-decoded row rather than an
        error.
        """
        return Table(name, self.columns, self.primary_key)

    @property
    def width(self) -> int:
        """How many columns the table declares."""
        return len(self.columns)

    @property
    def column_names(self) -> tuple[str, ...]:
        return tuple(c.name for c in self.columns)

    def ordinal_of(self, name: str) -> int | None:
        for at, column in enumerate(self.columns):
            if column.name == name:
                return at
        return None

    def scale_of(self, name: str) -> int | None:
        """The declared scale of a decimal column, or `None` for anything else.

        `None` rather than 0 for a non-decimal, the same distinction
        `ColumnDef::scale` makes on the Rust side: a caller cannot read a scale
        off a type that does not have one.
        """
        for column in self.columns:
            if column.name == name:
                return column.scale if column.type is ValueType.DECIMAL else None
        return None

    def type_of(self, name: str) -> ValueType | None:
        for column in self.columns:
            if column.name == name:
                return column.type
        return None

    def key_types(self) -> tuple[ValueType, ...]:
        """The declared types of the primary-key columns, in key order.

        A primary key on the wire is a `Row` holding "only the key columns, in
        key order" — a different shape from a full row, and one whose integer
        widths still have to be right.
        """
        types: list[ValueType] = []
        for key in self.primary_key:
            declared = self.type_of(key)
            if declared is None:  # pragma: no cover - refused in __init__
                raise ValueError(f"`{key}` is not a column of `{self.name}`")
            types.append(declared)
        return tuple(types)

    def column_types(self) -> tuple[ValueType, ...]:
        return tuple(c.type for c in self.columns)


# --- the schema check -------------------------------------------------------
#
# Finding 2 was that nothing could catch a client's declaration drifting from
# the server's catalog. `ColumnRef` removed the *cross-table* width arithmetic
# from the request side, but the within-table ordinal stayed an unverifiable
# local guess: a `Table` naming `category` where the server has `kind` filters
# the wrong column, and every layer accepts it because it is a well-formed
# reference to a real ordinal.
#
# The server now takes a `SchemaCheck` on every read and write. This is the
# client half. The canonical form is defined by `crates/slate-server/src/
# fingerprint.rs`, and the test file there prints this same algorithm in Python
# as its cross-implementation pin — so if these two ever disagree, that test
# fails on the Rust side rather than this failing silently here.

#: FNV-1a 64. Chosen by the server; restated rather than imported because a
#: fingerprint whose two implementations share code proves nothing.
_FNV_OFFSET = 0xCBF29CE484222325
_FNV_PRIME = 0x100000001B3
_MASK = 0xFFFFFFFFFFFFFFFF


def _fnv1a(data: bytes) -> int:
    h = _FNV_OFFSET
    for byte in data:
        h ^= byte
        h = (h * _FNV_PRIME) & _MASK
    return h


def _length_prefixed(text: str) -> bytes:
    """`text` as `<len>:<bytes>`.

    Length-prefixed rather than delimited so that no column name can be spelled
    to look like the end of one field and the start of another.
    """
    raw = text.encode()
    return str(len(raw)).encode() + b":" + raw


def _digits(n: int) -> bytes:
    return str(n).encode() + b";"


def fingerprint_of(table: Table) -> int:
    """This client's claim about `table`, in the server's canonical form.

    Only what a client can address and can be wrong about: the table name, and
    per ordinal the column's name and declared type, plus the primary key and
    the column count. Nullability, `DEFAULT`, `CHECK`, foreign keys and indexes
    address no column, so they are deliberately absent — hashing them would
    make an unrelated migration break every client, which is the failure mode
    that makes a fingerprint worse than none.
    """
    out = b"slate.v1.schema/1" + _length_prefixed(table.name)
    for ordinal, column in enumerate(table.columns):
        out += _digits(ordinal) + _length_prefixed(column.name) + _length_prefixed(column.type.value)
        # A decimal's scale, and only a decimal's. It addresses no column --
        # the test every other excluded property fails -- and is hashed anyway
        # because the failure it prevents is worse: a wrong ordinal reads the
        # wrong column and usually shows, a wrong scale reads the right column
        # and renders every value a power of ten out, for ever, with nothing
        # anywhere reporting it. The wire carries units and never the scale.
        if column.type is ValueType.ARRAY and column.element is not None:
            out += _length_prefixed(column.element.value)
        if column.type is ValueType.DECIMAL:
            out += _digits(column.scale)
    key_ordinals = [
        next(i for i, c in enumerate(table.columns) if c.name == name)
        for name in table.primary_key
    ]
    out += b"key" + _digits(len(key_ordinals))
    for k in key_ordinals:
        out += _digits(k)
    return _fnv1a(out + b"columns" + _digits(len(table.columns)))
