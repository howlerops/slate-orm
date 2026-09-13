"""What comes back: rows, joined rows and groups.

# The one place this client does arithmetic, and it is the protocol's fault

`ColumnRef` exists so that a *request* never carries an ordinal a client
computed. The response side has no such thing. A `Row` is a flat repeated
`Value`, and the `.proto` says a query's computed values arrive "after the
table's own columns, in the order they were requested". So to read computed
value `i`, a client adds the table's width to `i` — which is exactly the
arithmetic `ColumnRef` was introduced to remove, performed on the way back
instead of the way out, and with the same failure: add a column to the table
and every computed value moves.

`Row.computed(i)` does that addition, using the locally declared `Table`. It is
the only width arithmetic in this package, it is here rather than spread across
the call sites, and it is reported as a protocol gap rather than presented as a
solution. A `Row` with no table attached refuses `computed()` instead of
guessing, because guessing would return a stored column.

Joined rows and groups have no such problem, and the difference is instructive:
`JoinedRow` keeps one `Row` per input and `Group` keeps its keys apart from its
aggregates, so neither needs a width. Only the single-table-plus-computed shape
was left flat.
"""

from __future__ import annotations

from collections.abc import Iterator, Sequence

from ._proto.slate.v1 import records_pb2 as pb
from .schema import Table
from .values import PyValue, from_value

__all__ = ["Group", "JoinedRow", "Row"]


class Row(Sequence[PyValue]):
    """One row: a value per column, in ordinal order.

    Indexable by position, and by name when the `Table` it came from is known.
    """

    __slots__ = ("_computed", "_table", "_values")

    def __init__(
        self,
        values: Sequence[PyValue],
        table: Table | None = None,
        computed: Sequence[PyValue] = (),
    ) -> None:
        self._values = tuple(values)
        self._table = table
        self._computed = tuple(computed)

    @staticmethod
    def from_proto(wire: pb.Row, table: Table | None = None) -> Row:
        return Row(
            [from_value(v) for v in wire.values],
            table,
            [from_value(v) for v in wire.computed],
        )

    # There is deliberately no `to_proto` here. A `Row` that came back from a
    # read can be handed straight to `insert` or `update`, which encode it in
    # `_rows_proto` against the table's declared types — the one place that
    # knows them, and the place that has to, since a value's integer width has
    # to match the column on the write path. A second encoder on this class
    # would be an untested duplicate of that rule with less information.

    def __len__(self) -> int:
        return len(self._values)

    def __getitem__(self, index: int) -> PyValue:  # type: ignore[override]
        return self._values[index]

    def __iter__(self) -> Iterator[PyValue]:
        return iter(self._values)

    def __eq__(self, other: object) -> bool:
        if isinstance(other, Row):
            return self._values == other._values
        if isinstance(other, (tuple, list)):
            return list(self._values) == list(other)
        return NotImplemented

    def __hash__(self) -> int:
        return hash(self._values)

    def __repr__(self) -> str:
        if self._table is None:
            return f"Row{self._values!r}"
        pairs = ", ".join(
            f"{name}={value!r}"
            # Not strict: this is `__repr__`, and a row narrower than its
            # table is exactly the state worth being able to print.
            for name, value in zip(self._table.column_names, self._values, strict=False)
        )
        return f"{self._table.name}({pairs})"

    @property
    def values(self) -> tuple[PyValue, ...]:
        """Every value, stored columns and computed values alike."""
        return self._values

    def get(self, name: str) -> PyValue:
        """A stored column, by name.

        A column outside the query's projection comes back as `None`, which is
        a real answer rather than a missing one: a projection narrows what is
        decoded, and the kernel's own oracle found the bug where the answer
        depended on which access path was chosen.
        """
        if self._table is None:
            raise LookupError(
                "this row was read without a table, so it has no column names"
            )
        ordinal = self._table.ordinal_of(name)
        if ordinal is None:
            raise KeyError(f"table `{self._table.name}` has no column `{name}`")
        return self._values[ordinal]

    @property
    def computed_values(self) -> tuple[PyValue, ...]:
        """Every computed value, in the order the query asked for them.

        Beside [`values`][slate.Row.values] rather than appended to it: the
        wire keeps the two apart (finding 4), and joining them here would put
        back the width arithmetic that removal was for.
        """
        return self._computed

    def computed(self, index: int) -> PyValue:
        """The `index`th computed value of the query that produced this row.

        This used to be the one width addition in the package: the wire
        returned a flat row and the client added `table.width + index` to find
        a computed value, which is the arithmetic `ColumnRef` had already
        removed from the request side and the response side put back. It was
        reported as finding 4 and the wire now carries them in their own field,
        so no width is involved and a row read without a table can still be
        asked for one.
        """
        if index >= len(self._computed):
            raise IndexError(
                f"the row carries {len(self._computed)} computed values, so "
                f"there is no computed value {index}. The query did not compute "
                f"one."
            )
        return self._computed[index]


class JoinedRow(Sequence["Row | None"]):
    """One row of a join or chain: one entry per input, in request order.

    `None` where an outer join preserved a row that matched nothing on that
    input. The inputs are kept apart rather than concatenated, so nothing here
    has to know another input's width — the response side of a join got the
    treatment the response side of a computed value did not.
    """

    __slots__ = ("_inputs",)

    def __init__(self, inputs: Sequence[Row | None]) -> None:
        self._inputs = tuple(inputs)

    @staticmethod
    def from_proto(wire: pb.JoinedRow, tables: Sequence[Table | None]) -> JoinedRow:
        rows: list[Row | None] = []
        for at, joined in enumerate(wire.inputs):
            table = tables[at] if at < len(tables) else None
            # Message presence in proto3 is reliable, which is why this needs
            # no companion flag where a scalar would have.
            rows.append(Row.from_proto(joined.row, table) if joined.HasField("row") else None)
        return JoinedRow(rows)

    def __len__(self) -> int:
        return len(self._inputs)

    def __getitem__(self, index: int) -> Row | None:  # type: ignore[override]
        return self._inputs[index]

    def __iter__(self) -> Iterator[Row | None]:
        return iter(self._inputs)

    def __repr__(self) -> str:
        return f"JoinedRow({', '.join(repr(r) for r in self._inputs)})"


class Group(Sequence[PyValue]):
    """One row of a grouped result: its keys, then its aggregates.

    Kept apart on the wire and kept apart here. `Sequence` over the aggregates
    rather than over everything, because "the third value" of a group is
    ambiguous and the wire went to some trouble to make it not be.
    """

    __slots__ = ("_key", "_values")

    def __init__(self, key: Sequence[PyValue], values: Sequence[PyValue]) -> None:
        self._key = tuple(key)
        self._values = tuple(values)

    @staticmethod
    def from_proto(wire: pb.Group) -> Group:
        return Group(
            [from_value(v) for v in wire.key], [from_value(v) for v in wire.values]
        )

    @property
    def key(self) -> tuple[PyValue, ...]:
        """The GROUP BY keys, in the order they were requested."""
        return self._key

    @property
    def aggregates(self) -> tuple[PyValue, ...]:
        """The aggregates, in the order they were requested."""
        return self._values

    def aggregate(self, index: int) -> PyValue:
        return self._values[index]

    def key_at(self, index: int) -> PyValue:
        return self._key[index]

    def __len__(self) -> int:
        return len(self._values)

    def __getitem__(self, index: int) -> PyValue:  # type: ignore[override]
        return self._values[index]

    def __iter__(self) -> Iterator[PyValue]:
        return iter(self._values)

    def __repr__(self) -> str:
        return f"Group(key={self._key!r}, aggregates={self._values!r})"
