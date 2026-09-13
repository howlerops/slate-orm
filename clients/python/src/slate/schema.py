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
