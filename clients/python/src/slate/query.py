"""Reads: a single-table query, a join or chain, and a grouped aggregate.

Every builder here is mutable and returns itself, so a read is written as a
sequence of statements or as a chain, whichever suits. Mutability is not
incidental: a computed value has to be declared before it can be named, an
input has to be added before its columns exist, and an immutable builder would
turn both of those into a threading exercise.

# What each builder does not offer, and why that is the point

- A join input has no `sort`, `limit` or `offset`. The kernel documents them as
  ignored there, the server refuses them rather than ignoring them, and this
  builder has no way to set one — so the refusal is unreachable through this
  API rather than being a runtime surprise. `JoinQuery` carries the limit and
  offset that do apply.
- An aggregate's *input* has no projection, no sort, no limit and no offset:
  the projection is the kernel's to narrow (that is what lets an index answer
  `COUNT(*)` without reading a row), and a sort or limit there would order and
  cut the rows going *into* the groups, which is never what a caller means.
  The sort, limit and offset that do apply are on the grouped result, over
  groups, and live on the grouping itself.
- There is no `input=` argument anywhere. An input's position is assigned by
  `JoinQuery.add`, in call order, which is the order the wire declares them in.
"""

from __future__ import annotations

import dataclasses
import enum
from collections.abc import Iterable, Sequence

from ._proto.slate.v1 import records_pb2 as pb
from .expr import (
    ColumnRef,
    Columns,
    Expr,
    aggregate_ref,
    computed_ref,
    group_key_ref,
    joined_computed_ref,
)
from .scalar import Scalar
from .schema import Table, fingerprint_of

__all__ = [
    "Agg",
    "AggregateQuery",
    "GroupedJoinQuery",
    "JoinAlgorithm",
    "JoinInput",
    "JoinQuery",
    "JoinType",
    "NullsOrder",
    "Query",
    "ScanOrder",
    "SortKey",
    "asc",
    "desc",
]


class ScanOrder(enum.Enum):
    """Which way the underlying scan runs."""

    ASCENDING = pb.SCAN_ORDER_ASCENDING
    DESCENDING = pb.SCAN_ORDER_DESCENDING


class NullsOrder(enum.Enum):
    """Where nulls go in a sort.

    `NATURAL` is the direction's own place, which is where the storage already
    puts them — and therefore the only setting that can be served without
    materialising the whole result.
    """

    NATURAL = pb.NULLS_ORDER_UNSPECIFIED
    FIRST = pb.NULLS_ORDER_FIRST
    LAST = pb.NULLS_ORDER_LAST


@dataclasses.dataclass(frozen=True)
class SortKey:
    """One key of an `ORDER BY`."""

    column: ColumnRef
    descending: bool = False
    nulls: NullsOrder = NullsOrder.NATURAL

    def to_proto(self) -> pb.SortKey:
        return pb.SortKey(
            column=self.column.to_proto(),
            direction=pb.SORT_DIRECTION_DESC if self.descending else pb.SORT_DIRECTION_ASC,
            nulls=self.nulls.value,
        )


def asc(column: ColumnRef, *, nulls: NullsOrder = NullsOrder.NATURAL) -> SortKey:
    """Ascending."""
    return SortKey(column, descending=False, nulls=nulls)


def desc(column: ColumnRef, *, nulls: NullsOrder = NullsOrder.NATURAL) -> SortKey:
    """Descending."""
    return SortKey(column, descending=True, nulls=nulls)


class _QueryBase:
    """The parts of a `Query` that a join input and an aggregate input share.

    Split out so that `JoinInput` and `AggregateQuery` can offer exactly the
    fields the server accepts in their position, rather than offering all of
    them and refusing four at runtime.
    """

    def __init__(self, table: Table, input: int) -> None:
        self.table = table
        self._input = input
        self._filter: Expr | None = None
        self._order = ScanOrder.ASCENDING
        self._compute: list[Scalar] = []
        self._hint: pb.AccessHint | None = None

    @property
    def c(self) -> Columns:
        """This input's columns, bound to its position.

        The only way to obtain a reference. See `expr.Columns`.
        """
        return Columns(self.table, self._input)

    def computed(self, index: int) -> ColumnRef:
        """The `index`th value this input computes, counting from zero.

        Never an offset. The server adds the table's width; this side does not
        know it and does not need to.
        """
        return computed_ref(self._input, index)

    def _base_proto(self) -> pb.Query:
        # The schema check rides on `Query` rather than on the request, because
        # a join carries one `Query` per input and a client can be right about
        # one table and wrong about another. Building it here means every
        # shape — a plain read, a join input, an aggregate's source — carries
        # it without each call site remembering to.
        query = pb.Query(
            table=self.table.name,
            order=self._order.value,
            compute=[s.to_proto() for s in self._compute],
            schema=pb.SchemaCheck(
                columns=self.table.width, fingerprint=fingerprint_of(self.table)
            ),
        )
        if self._filter is not None:
            query.filter.CopyFrom(self._filter.to_proto())
        if self._hint is not None:
            query.hint.CopyFrom(self._hint)
        return query


class Query(_QueryBase):
    """A read of one table."""

    def __init__(self, table: Table) -> None:
        # A single-table read has exactly one input, and it is input 0. That is
        # the wire's degenerate case rather than a second scheme, which is why
        # nothing here takes an input number.
        super().__init__(table, input=0)
        self._projection: pb.Projection | None = None
        self._sort: list[SortKey] = []
        self._limit: int | None = None
        self._offset = 0

    def where(self, filter: Expr) -> Query:
        """Keep only rows this admits. Replaces any earlier filter."""
        self._filter = filter
        return self

    def order(self, order: ScanOrder) -> Query:
        """Which way the underlying scan runs. Not an `ORDER BY`; see `sort`."""
        self._order = order
        return self

    def descending(self) -> Query:
        return self.order(ScanOrder.DESCENDING)

    def compute(self, *values: Scalar) -> Query:
        """Append computed values. `computed(i)` names the `i`th."""
        self._compute.extend(values)
        return self

    def select(self, *columns: ColumnRef) -> Query:
        """Return only these stored columns; the rest come back null.

        Only stored columns can be named. A computed value is returned whatever
        the projection says — it was computed rather than read, so leaving it
        out would save nothing — and naming one here is refused by the server
        rather than ignored.
        """
        self._projection = pb.Projection(columns=[c.to_proto() for c in columns])
        return self

    def select_all(self) -> Query:
        """Every stored column. The default."""
        self._projection = pb.Projection(all_columns=True)
        return self

    def select_none(self) -> Query:
        """No stored columns at all, which is what a count wants."""
        self._projection = pb.Projection()
        return self

    def sort(self, *keys: SortKey) -> Query:
        """Order the result. Replaces any earlier sort."""
        self._sort = list(keys)
        return self

    def limit(self, limit: int | None) -> Query:
        self._limit = limit
        return self

    def offset(self, offset: int) -> Query:
        self._offset = offset
        return self

    def using_index(self, name: str) -> Query:
        """Take this index instead of the cheapest path.

        Advice, not an instruction: an index the table does not have is
        ignored, matching the kernel, because a query that stops working
        because an index was renamed is worse than one that gets slower. The
        server records that it did so in `Explain`'s warnings — and *only*
        there, so a plain `Query` cannot tell you your hint did nothing.
        """
        self._hint = pb.AccessHint(index=name)
        return self

    def using_table_scan(self) -> Query:
        self._hint = pb.AccessHint(table_scan=pb.UNIT)
        return self

    def to_proto(self) -> pb.Query:
        query = self._base_proto()
        if self._projection is not None:
            query.projection.CopyFrom(self._projection)
        query.sort.extend(k.to_proto() for k in self._sort)
        if self._limit is not None:
            query.limit = self._limit
        query.offset = self._offset
        return query


# --- aggregates -------------------------------------------------------------


@dataclasses.dataclass(frozen=True)
class Agg:
    """One aggregate function, and the column it reads.

    Built with the factories below rather than directly: `Agg.count()` takes no
    column and every other function requires one, which is a rule the server
    enforces ("an aggregate over nothing is refused rather than read as
    `COUNT(*)`, which is a different number") and which these signatures make
    unrepresentable.
    """

    # The generated enum wrapper rather than a bare `int`, so that a
    # function value from somewhere else is a type error here rather than
    # an `AGGREGATE_FUNCTION_UNSPECIFIED` the server refuses at runtime.
    function: pb.AggregateFunction.ValueType
    column: ColumnRef | None = None

    @staticmethod
    def count(column: ColumnRef | None = None) -> Agg:
        """`COUNT(*)` with no column, `COUNT(column)` with one.

        The two are different numbers — `COUNT(*)` counts every row including
        one whose every column is null — and they are different functions on
        the wire. One Python name because they are one name in SQL, and the
        presence of the argument is what picks.
        """
        if column is None:
            return Agg(pb.AGGREGATE_FUNCTION_COUNT)
        return Agg(pb.AGGREGATE_FUNCTION_COUNT_COLUMN, column)

    @staticmethod
    def min(column: ColumnRef) -> Agg:
        return Agg(pb.AGGREGATE_FUNCTION_MIN, column)

    @staticmethod
    def max(column: ColumnRef) -> Agg:
        return Agg(pb.AGGREGATE_FUNCTION_MAX, column)

    @staticmethod
    def sum(column: ColumnRef) -> Agg:
        return Agg(pb.AGGREGATE_FUNCTION_SUM, column)

    @staticmethod
    def avg(column: ColumnRef) -> Agg:
        return Agg(pb.AGGREGATE_FUNCTION_AVG, column)

    @staticmethod
    def count_distinct(column: ColumnRef) -> Agg:
        """Exact, over the encoded value: two rows count as one exactly when
        they would collide in an index."""
        return Agg(pb.AGGREGATE_FUNCTION_COUNT_DISTINCT, column)

    def to_proto(self) -> pb.Aggregate:
        aggregate = pb.Aggregate(function=self.function)
        if self.column is not None:
            aggregate.column.CopyFrom(self.column.to_proto())
        return aggregate


class _Grouping:
    """The grouping half of an aggregate: keys, aggregates, HAVING, and the
    window over the *groups*.

    Split from the source so that a grouped table and a grouped join can offer
    the same grouping without either offering the other's source fields. The
    alternative — one `AggregateQuery` holding both a table and a join and
    ignoring whichever is unset — would make `.where()` and `.using_index()`
    silently do nothing on the join-shaped one, which is the runtime surprise
    this module's header says it exists to avoid.
    """

    def __init__(self) -> None:
        self._group_by: list[ColumnRef] = []
        self._aggregates: list[Agg] = []
        self._having: Expr | None = None
        self._sort: list[SortKey] = []
        self._limit: int | None = None
        self._offset = 0

    def key(self, index: int) -> ColumnRef:
        """The `index`th GROUP BY key, for `having` and `sort`."""
        return group_key_ref(index)

    def agg(self, index: int) -> ColumnRef:
        """The `index`th aggregate, for `having` and `sort`.

        `having` and `sort` are evaluated over the *group*, so they name keys
        and aggregates rather than columns. A raw column here is a kind
        mismatch the server refuses by name — SQL's "column must appear in the
        GROUP BY clause", made decidable by `ColumnRef` carrying a kind at all.
        """
        return aggregate_ref(index)

    def _grouping_proto(self, query: pb.AggregateQuery) -> None:
        query.group_by.extend(c.to_proto() for c in self._group_by)
        query.aggregates.extend(a.to_proto() for a in self._aggregates)
        query.sort.extend(k.to_proto() for k in self._sort)
        query.offset = self._offset
        if self._having is not None:
            query.having.CopyFrom(self._having.to_proto())
        if self._limit is not None:
            query.limit = self._limit


class AggregateQuery(_QueryBase, _Grouping):
    """Aggregates over one table, optionally per group.

    For the join-shaped version see `GroupedJoinQuery`.
    """

    def __init__(self, table: Table) -> None:
        _QueryBase.__init__(self, table, input=0)
        _Grouping.__init__(self)

    def where(self, filter: Expr) -> AggregateQuery:
        """Which rows go into the aggregate. Over rows, not over groups."""
        self._filter = filter
        return self

    def compute(self, *values: Scalar) -> AggregateQuery:
        self._compute.extend(values)
        return self

    def group_by(self, *columns: ColumnRef) -> AggregateQuery:
        """Group by these. None means one group over every matching row."""
        self._group_by = list(columns)
        return self

    def aggregate(self, *aggregates: Agg) -> AggregateQuery:
        """What to compute per group.

        Optional. Group keys with no aggregates are the distinct combinations
        of those keys — `SELECT DISTINCT`. What the server refuses is neither:
        no keys and no aggregates asks for one group with nothing in it.
        """
        self._aggregates.extend(aggregates)
        return self

    def having(self, having: Expr) -> AggregateQuery:
        """Which groups to keep."""
        self._having = having
        return self

    def sort(self, *keys: SortKey) -> AggregateQuery:
        """Order the groups. Names keys and aggregates, not columns."""
        self._sort = list(keys)
        return self

    def limit(self, limit: int | None) -> AggregateQuery:
        """How many groups to return."""
        self._limit = limit
        return self

    def offset(self, offset: int) -> AggregateQuery:
        """How many groups to skip. Applied before the limit."""
        self._offset = offset
        return self

    def using_index(self, name: str) -> AggregateQuery:
        self._hint = pb.AccessHint(index=name)
        return self

    def using_table_scan(self) -> AggregateQuery:
        self._hint = pb.AccessHint(table_scan=pb.UNIT)
        return self

    def to_proto(self) -> pb.AggregateQuery:
        query = pb.AggregateQuery(input=self._base_proto())
        self._grouping_proto(query)
        return query


# --- joins and chains -------------------------------------------------------


class JoinType(enum.Enum):
    """Which rows survive a join."""

    INNER = pb.JOIN_TYPE_INNER
    #: Every row accumulated so far, with this input absent where it matched
    #: nothing.
    LEFT = pb.JOIN_TYPE_LEFT
    #: Every row of this input, with everything before it absent where it
    #: matched nothing.
    RIGHT = pb.JOIN_TYPE_RIGHT
    FULL = pb.JOIN_TYPE_FULL


@dataclasses.dataclass(frozen=True)
class JoinAlgorithm:
    """An algorithm to run instead of the cheapest one.

    Not advice, unlike an index hint. A nested loop streams one side and never
    learns about a row of the other that matched nothing, so asking for one on
    a right or full outer join is an error rather than a quiet fallback to a
    hash join — which would return the inner rows and a short answer.
    """

    _proto: pb.JoinAlgorithm

    @staticmethod
    def hash_build_left() -> JoinAlgorithm:
        """Hold the accumulated side in memory and stream this input past it."""
        return JoinAlgorithm(pb.JoinAlgorithm(hash_build=pb.SIDE_LEFT))

    @staticmethod
    def hash_build_right() -> JoinAlgorithm:
        """Hold this input in memory and stream the accumulated side past it."""
        return JoinAlgorithm(pb.JoinAlgorithm(hash_build=pb.SIDE_RIGHT))

    @staticmethod
    def nested_loop() -> JoinAlgorithm:
        return JoinAlgorithm(pb.JoinAlgorithm(nested_loop=pb.UNIT))

    def to_proto(self) -> pb.JoinAlgorithm:
        return self._proto


class JoinInput(_QueryBase):
    """One table of a join, and how it attaches to the ones before it.

    Obtained from `JoinQuery.add`, which is what gave it its position. There is
    no constructor a caller would reach for, because a position chosen by hand
    is the mistake the positional model exists to prevent.
    """

    def __init__(self, table: Table, index: int) -> None:
        super().__init__(table, index)
        self._on: list[tuple[ColumnRef, ColumnRef]] = []
        self._projection: pb.Projection | None = None
        self._join_type = JoinType.INNER
        self._having: Expr | None = None
        self._force: JoinAlgorithm | None = None

    @property
    def index(self) -> int:
        """This input's position, which is what a `ColumnRef.input` holds."""
        return self._input

    def where(self, filter: Expr) -> JoinInput:
        """This input's own filter, in its own ordinals."""
        self._filter = filter
        return self

    def compute(self, *values: Scalar) -> JoinInput:
        """Values this input computes.

        Usable in this input's own filter, and nowhere else: the kernel's
        joined ordinal space is packed by declared table width and has no slot
        for a computed value, so naming one from another input is refused.
        """
        self._compute.extend(values)
        return self

    def select(self, *columns: ColumnRef) -> JoinInput:
        """Return only these stored columns of this input; the rest come back null."""
        self._projection = pb.Projection(columns=[c.to_proto() for c in columns])
        return self

    def order(self, order: ScanOrder) -> JoinInput:
        self._order = order
        return self

    def on(self, earlier: ColumnRef, own: str | ColumnRef) -> JoinInput:
        """One equality of the join condition.

        `earlier` must be a column of an input already read — the server
        refuses one that names an input not yet available, by position rather
        than by guessing. `own` is a column of *this* input: a name, or a
        reference from this input's own `c`. A reference from a different input
        is refused here rather than sent, because the wire would accept it and
        answer a question nobody asked.
        """
        if isinstance(own, str):
            own_ref = self.c[own]
        else:
            if own.input != self._input:
                raise ValueError(
                    f"the `own` side of a join condition on input {self._input} names "
                    f"{own}, which belongs to a different input. Name a column of "
                    f"`{self.table.name}`."
                )
            own_ref = own
        if earlier.input >= self._input:
            raise ValueError(
                f"the `earlier` side of a join condition on input {self._input} names "
                f"{earlier}, which is not an input read before it"
            )
        self._on.append((earlier, own_ref))
        return self

    def having(self, having: Expr) -> JoinInput:
        """A condition only a formed row can answer.

        Behaves like SQL's `ON`, not `WHERE`: a preserved row whose other side
        is absent reads as null there, so the condition is unknown and the pair
        is not admitted — but the preserved row itself still comes back,
        unmatched.
        """
        self._having = having
        return self

    def force(self, algorithm: JoinAlgorithm) -> JoinInput:
        self._force = algorithm
        return self

    def to_proto(self) -> pb.JoinInput:
        query = self._base_proto()
        if self._projection is not None:
            query.projection.CopyFrom(self._projection)
        wire = pb.JoinInput(
            query=query,
            on=[
                pb.JoinOn(earlier=earlier.to_proto(), own=own.to_proto())
                for earlier, own in self._on
            ],
            join_type=self._join_type.value,
        )
        if self._having is not None:
            wire.having.CopyFrom(self._having.to_proto())
        if self._force is not None:
            wire.force.CopyFrom(self._force.to_proto())
        return wire


class JoinQuery:
    """A read over several tables, joined in the order added.

    Left-deep and explicit: there is no join-order search, because a wrong
    order costs round trips rather than a constant factor and the estimates
    feeding a search compound. The order tables are added in is the order they
    are joined in.

    Two inputs become a kernel `Join` and three or more a `Chain`. That is a
    dispatch inside the server, not a difference here.
    """

    def __init__(self) -> None:
        self._inputs: list[JoinInput] = []
        self._limit: int | None = None
        self._offset = 0
        self._build_limit: int | None = None
        self._compute: list[Scalar] = []

    def add(
        self,
        table: Table,
        *,
        on: Iterable[tuple[ColumnRef, str | ColumnRef]] = (),
        join_type: JoinType = JoinType.INNER,
    ) -> JoinInput:
        """Add a table, and return the handle its columns come from.

        The position is assigned here, in call order. Adding the same `Table`
        twice makes two inputs, which is all a self-join needs — an input is a
        position, which a self-join distinguishes and a name cannot.
        """
        wire_input = JoinInput(table, len(self._inputs))
        wire_input._join_type = join_type
        self._inputs.append(wire_input)
        for earlier, own in on:
            wire_input.on(earlier, own)
        return wire_input

    def limit(self, limit: int | None) -> JoinQuery:
        """Maximum joined rows to return."""
        self._limit = limit
        return self

    def offset(self, offset: int) -> JoinQuery:
        """Joined rows to discard first."""
        self._offset = offset
        return self

    def build_limit(self, rows: int) -> JoinQuery:
        """Rows a build side may hold in memory before the read is refused.

        May be lowered and not raised. The server's limit is what stops a
        mistyped join key becoming an out-of-memory kill, and a limit a client
        can raise is not a limit — a higher value is clamped and reported as a
        warning rather than refused, and the warning is only visible through
        `explain_join`.
        """
        self._build_limit = rows
        return self

    def compute(self, *values: Scalar) -> JoinQuery:
        """Values computed per *joined* row, named with `computed(i)`.

        The arrangement `Query.compute` uses on one table, lifted one level.
        What is new is that the expression is evaluated over the joined row, so
        it may read both sides at once — which is the thing no input's own
        `compute` can express, and the reason this lives here rather than
        there.

        An input's computed value is appended to *that input's* row, and a
        joined row is packed by declared table width, so such a value has no
        slot in the joined space and naming one across inputs is refused. A
        value declared here does have one, past every input's columns.

        Each may read every input's columns and the values *before* it, so the
        second may read `computed(0)` and not the other way round.
        """
        self._compute.extend(values)
        return self

    def computed(self, index: int) -> ColumnRef:
        """The `index`th value the join itself computes, counting from zero.

        Not `inputs[n].computed(i)`, which names an input's own — a different
        kind on the wire, with a different space and a different refusal.
        """
        return joined_computed_ref(index)

    @property
    def inputs(self) -> Sequence[JoinInput]:
        return tuple(self._inputs)

    def to_proto(self) -> pb.JoinQuery:
        query = pb.JoinQuery(
            inputs=[i.to_proto() for i in self._inputs],
            offset=self._offset,
            compute=[s.to_proto() for s in self._compute],
        )
        if self._limit is not None:
            query.limit = self._limit
        if self._build_limit is not None:
            query.build_limit = self._build_limit
        return query


class GroupedJoinQuery(_Grouping):
    """Aggregates over a join, per group.

    Two inputs exactly. The kernel groups a two-table join and does not group a
    chain, and the server refuses a third input by name rather than planning it
    as something else — so this builder takes the join it is given and lets
    that refusal arrive with its reason, rather than reimplementing the count
    here where it could drift from the server's.

    Group keys are named in the *joined* schema: `join.inputs()[1].c.author_id`
    is the right side's column, and resolving it is the server's job. That is
    the one thing this shape gets wrong most easily, so there is a test for it
    in both other clients.
    """

    def __init__(self, join: JoinQuery) -> None:
        super().__init__()
        self.join = join

    def group_by(self, *columns: ColumnRef) -> GroupedJoinQuery:
        """Group by these. None means one group over every joined row."""
        self._group_by = list(columns)
        return self

    def aggregate(self, *aggregates: Agg) -> GroupedJoinQuery:
        """What to compute per group.

        Optional, as on `AggregateQuery`: keys with no aggregates are the
        distinct combinations of those keys over the joined rows. Neither keys
        nor aggregates is refused.
        """
        self._aggregates.extend(aggregates)
        return self

    def having(self, having: Expr) -> GroupedJoinQuery:
        """Which groups to keep."""
        self._having = having
        return self

    def sort(self, *keys: SortKey) -> GroupedJoinQuery:
        """Order the groups. Names keys and aggregates, not columns."""
        self._sort = list(keys)
        return self

    def limit(self, limit: int | None) -> GroupedJoinQuery:
        """How many groups to return."""
        self._limit = limit
        return self

    def offset(self, offset: int) -> GroupedJoinQuery:
        """How many groups to skip. Applied before the limit."""
        self._offset = offset
        return self

    def to_proto(self) -> pb.AggregateQuery:
        query = pb.AggregateQuery(join=self.join.to_proto())
        self._grouping_proto(query)
        return query
