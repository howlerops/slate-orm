"""Column references and predicates.

# The one rule this module exists to enforce

`ColumnRef` names a *producer* and an index inside it. The server does the
ordinal arithmetic, and the whole point of the design — stated at length in
`docs/topology.md` and in the `.proto` — is that a client never adds a table
width to anything, because a width it computed from is silently re-pointed the
day a column is added to an earlier table.

A Python SDK can throw that away in one line, by handing the caller a `Table`
that produces references with `input=0`. Then `books.title` inside a join where
`books` is the second input is a reference to *input 0's* column 3, which is in
range on almost any table and means something else entirely. The server cannot
catch it: it is a legitimate reference to the wrong column.

So `Table` produces no references at all. Every reference in this package comes
from a `Columns` accessor that was handed its input position by the builder
that knows it — `Query.c` is position 0 because a single-table read has one
input, and `JoinInput.c` is whatever position `JoinQuery.add` gave it. There is
no way to write an input number, and no way to obtain a reference that is not
bound to one.

# Equality is a method, not an operator

`<`, `<=`, `>` and `>=` are operators here. `==` and `!=` are not: they are
`.eq()` and `.ne()`.

Overloading `__eq__` to return a predicate is the usual trick, and it costs
more than it looks. `__eq__` is required to return `bool` and to be consistent
with `__hash__`; overriding it makes `ref in some_list`, `assert ref == other`
and every dict lookup do something surprising, and it is a type error that has
to be silenced rather than fixed. The four ordered operators have no such
contract, so they are free.

The result is one asymmetry — `c.size >= 15` next to `c.kind.eq("a")` — which
is stated here rather than hidden. Every comparison also has a method form
(`.lt .le .gt .ge .eq .ne`), so a caller who dislikes the asymmetry can write
all six the same way.
"""

from __future__ import annotations

import dataclasses
import enum
from collections.abc import Iterable, Sequence
from typing import TYPE_CHECKING, Protocol

from ._proto.slate.v1 import records_pb2 as pb
from .types import ValueType
from .values import PyValue, to_value

if TYPE_CHECKING:  # pragma: no cover
    from .scalar import PairOp, Scalar

__all__ = [
    "CmpOp",
    "ColumnRef",
    "Columns",
    "Expr",
    "all_of",
    "any_of",
    "not_",
]


class CmpOp(enum.Enum):
    """A comparison operator, mirroring the wire's `CmpOp`."""

    EQ = pb.CMP_OP_EQ
    NE = pb.CMP_OP_NE
    LT = pb.CMP_OP_LT
    LE = pb.CMP_OP_LE
    GT = pb.CMP_OP_GT
    GE = pb.CMP_OP_GE


class _Kind(enum.Enum):
    """Which producer a reference names. The wire's `ColumnRef.of`."""

    COLUMN = "column"
    COMPUTED = "computed"
    GROUP_KEY = "group_key"
    AGGREGATE = "aggregate"


@dataclasses.dataclass(frozen=True)
class ColumnRef:
    """One slot of a row: a producer and an index inside it.

    Never constructed by a caller. It comes from a `Columns` accessor, which
    got its `input` from the builder that assigned the position — see the
    module docstring for why that matters.

    `declared` is the column's type where one is known, and is carried purely
    so that a bare Python `int` compared against this reference can be encoded
    as the right one of the wire's two integer types. It is `None` for a
    computed value, a group key or an aggregate, where nothing declares a type
    and an integer must be written as `i64(...)` or `u64(...)`.
    """

    input: int
    kind: _Kind
    index: int
    #: For error messages only. A reference is addressed by ordinal on the
    #: wire; the name never leaves this process.
    label: str = ""
    declared: ValueType | None = None

    def to_proto(self) -> pb.ColumnRef:
        ref = pb.ColumnRef(input=self.input)
        setattr(ref, self.kind.value, self.index)
        return ref

    def __str__(self) -> str:
        where = f"input {self.input}"
        what = f"{self.kind.value} {self.index}"
        return f"{where}'s {what}" + (f" ({self.label})" if self.label else "")

    # --- comparisons ------------------------------------------------------

    def _cmp(self, op: CmpOp, other: ColumnRef | PyValue) -> Expr:
        if isinstance(other, ColumnRef):
            return _CompareColumns(self, op, other)
        return _Compare(self, op, to_value(other, self.declared))

    def eq(self, other: ColumnRef | PyValue) -> Expr:
        """`self = other`."""
        return self._cmp(CmpOp.EQ, other)

    def ne(self, other: ColumnRef | PyValue) -> Expr:
        """`self <> other`."""
        return self._cmp(CmpOp.NE, other)

    def lt(self, other: ColumnRef | PyValue) -> Expr:
        """`self < other`."""
        return self._cmp(CmpOp.LT, other)

    def le(self, other: ColumnRef | PyValue) -> Expr:
        """`self <= other`."""
        return self._cmp(CmpOp.LE, other)

    def gt(self, other: ColumnRef | PyValue) -> Expr:
        """`self > other`."""
        return self._cmp(CmpOp.GT, other)

    def ge(self, other: ColumnRef | PyValue) -> Expr:
        """`self >= other`."""
        return self._cmp(CmpOp.GE, other)

    def __lt__(self, other: ColumnRef | PyValue) -> Expr:
        return self.lt(other)

    def __le__(self, other: ColumnRef | PyValue) -> Expr:
        return self.le(other)

    def __gt__(self, other: ColumnRef | PyValue) -> Expr:
        return self.gt(other)

    def __ge__(self, other: ColumnRef | PyValue) -> Expr:
        return self.ge(other)

    # --- the rest of the predicate vocabulary -----------------------------

    def is_null(self) -> Expr:
        """`self IS NULL`."""
        return _IsNull(self, negated=False)

    def is_not_null(self) -> Expr:
        """`self IS NOT NULL`."""
        return _IsNull(self, negated=True)

    def like(self, pattern: str, *, insensitive: bool = False) -> Expr:
        """`self LIKE pattern`, or `ILIKE` when `insensitive`."""
        return _Like(self, pattern, negated=False, insensitive=insensitive)

    def not_like(self, pattern: str, *, insensitive: bool = False) -> Expr:
        """`self NOT LIKE pattern`."""
        return _Like(self, pattern, negated=True, insensitive=insensitive)

    def matches(self, pattern: str, *, insensitive: bool = False) -> Expr:
        """`self ~ pattern`, a regular-expression match.

        The syntax is the Rust `regex` crate's: linear time, no backreferences
        and no lookaround. A pattern that does not compile matches nothing
        rather than failing the query, which is the kernel's choice and not
        this client's to change.
        """
        return _Matches(self, pattern, negated=False, insensitive=insensitive)

    def not_matches(self, pattern: str, *, insensitive: bool = False) -> Expr:
        """`self !~ pattern`."""
        return _Matches(self, pattern, negated=True, insensitive=insensitive)

    def in_(self, values: Iterable[PyValue]) -> Expr:
        """`self IN (values)`."""
        return _InList(self, [to_value(v, self.declared) for v in values])

    # --- arithmetic, which makes this a `Scalar` -------------------------
    #
    # `scalar` imports this module (a `Case` branch holds an `Expr`), so the
    # import here is deferred to call time rather than made at module level.
    # The alternative was one big module holding both, which the kernel does
    # not do either — `Expr` and `Scalar` are separate there.

    def __add__(self, other: object) -> Scalar:
        return _arith("add", self, other)

    def __sub__(self, other: object) -> Scalar:
        return _arith("sub", self, other)

    def __mul__(self, other: object) -> Scalar:
        return _arith("mul", self, other)

    def __truediv__(self, other: object) -> Scalar:
        return _arith("div", self, other)

    def __radd__(self, other: object) -> Scalar:
        return _arith("add", other, self)

    def __rsub__(self, other: object) -> Scalar:
        return _arith("sub", other, self)

    def __rmul__(self, other: object) -> Scalar:
        return _arith("mul", other, self)

    def __rtruediv__(self, other: object) -> Scalar:
        return _arith("div", other, self)


def _arith(op: PairOp, left: object, right: object) -> Scalar:
    from .scalar import _Pair, as_scalar

    def coerce(side: object) -> Scalar:
        # `cast` rather than a runtime check: `as_scalar` refuses anything it
        # cannot encode, with a better message than one written here would be.
        return as_scalar(side)  # type: ignore[arg-type]

    return _Pair(op, coerce(left), coerce(right))


class Expr:
    """A predicate.

    One type for every shape of row — a single table's, a joined row's, a
    group's — which is the kernel's own arrangement and the reason three-valued
    logic has exactly one implementation. The wire keeps that; so does this.

    `&`, `|` and `~` build conjunctions, disjunctions and negations. They bind
    *tighter* than comparison in Python, so `a.x > 1 & a.y < 2` parses as
    `a.x > (1 & a.y) < 2` and is a bug. Parenthesise, or use `all_of` / `any_of`
    / `not_`, which cannot be got wrong.
    """

    __slots__ = ()

    def to_proto(self) -> pb.Expr:  # pragma: no cover - overridden by every case
        raise NotImplementedError

    def __and__(self, other: Expr) -> Expr:
        return all_of([self, other])

    def __or__(self, other: Expr) -> Expr:
        return any_of([self, other])

    def __invert__(self) -> Expr:
        return not_(self)


@dataclasses.dataclass(frozen=True)
class _Literal(Expr):
    value: bool

    def to_proto(self) -> pb.Expr:
        return pb.Expr(literal=self.value)


@dataclasses.dataclass(frozen=True)
class _Compare(Expr):
    column: ColumnRef
    op: CmpOp
    value: pb.Value

    def to_proto(self) -> pb.Expr:
        return pb.Expr(
            compare=pb.Compare(
                column=self.column.to_proto(), op=self.op.value, value=self.value
            )
        )


@dataclasses.dataclass(frozen=True)
class _CompareColumns(Expr):
    left: ColumnRef
    op: CmpOp
    right: ColumnRef

    def to_proto(self) -> pb.Expr:
        return pb.Expr(
            compare_columns=pb.CompareColumns(
                left=self.left.to_proto(),
                op=self.op.value,
                right=self.right.to_proto(),
            )
        )


@dataclasses.dataclass(frozen=True)
class _IsNull(Expr):
    column: ColumnRef
    negated: bool

    def to_proto(self) -> pb.Expr:
        return pb.Expr(
            is_null=pb.IsNull(column=self.column.to_proto(), negated=self.negated)
        )


@dataclasses.dataclass(frozen=True)
class _Like(Expr):
    column: ColumnRef
    pattern: str
    negated: bool
    insensitive: bool

    def to_proto(self) -> pb.Expr:
        return pb.Expr(
            like=pb.Like(
                column=self.column.to_proto(),
                pattern=self.pattern,
                negated=self.negated,
                insensitive=self.insensitive,
            )
        )


@dataclasses.dataclass(frozen=True)
class _Matches(Expr):
    column: ColumnRef
    pattern: str
    negated: bool
    insensitive: bool

    def to_proto(self) -> pb.Expr:
        return pb.Expr(
            matches=pb.Matches(
                column=self.column.to_proto(),
                pattern=self.pattern,
                negated=self.negated,
                insensitive=self.insensitive,
            )
        )


@dataclasses.dataclass(frozen=True)
class _InList(Expr):
    column: ColumnRef
    values: Sequence[pb.Value]

    def to_proto(self) -> pb.Expr:
        return pb.Expr(
            in_list=pb.InList(column=self.column.to_proto(), values=list(self.values))
        )


@dataclasses.dataclass(frozen=True)
class _Junction(Expr):
    parts: Sequence[Expr]
    disjunction: bool

    def to_proto(self) -> pb.Expr:
        exprs = pb.ExprList(exprs=[p.to_proto() for p in self.parts])
        if self.disjunction:
            return pb.Expr(disjunction=exprs)
        return pb.Expr(conjunction=exprs)


@dataclasses.dataclass(frozen=True)
class _Not(Expr):
    inner: Expr

    def to_proto(self) -> pb.Expr:
        return pb.Expr(negation=self.inner.to_proto())


TRUE: Expr = _Literal(True)
FALSE: Expr = _Literal(False)


def all_of(parts: Iterable[Expr]) -> Expr:
    """Every part must hold.

    An empty conjunction is `TRUE`, which is the identity and what the kernel's
    `Expr::True` means. Returning the single part unwrapped when there is one
    keeps a filter's wire form the shape the caller wrote, which matters only
    because it makes a request legible in a log.
    """
    items = list(parts)
    if not items:
        return TRUE
    if len(items) == 1:
        return items[0]
    return _Junction(items, disjunction=False)


def any_of(parts: Iterable[Expr]) -> Expr:
    """At least one part must hold.

    An empty disjunction is `FALSE` — the identity for `or`, and the answer
    that cannot silently widen a query. A caller who builds `any_of(ids)` from
    an empty list means "none of them", and `TRUE` would mean "all rows".
    """
    items = list(parts)
    if not items:
        return FALSE
    if len(items) == 1:
        return items[0]
    return _Junction(items, disjunction=True)


def not_(inner: Expr) -> Expr:
    """The negation. Three-valued: `NOT unknown` is unknown."""
    return _Not(inner)


class Columns:
    """The columns of one input, already bound to its position.

    Obtained from a builder — `Query.c`, `JoinInput.c`, `AggregateQuery.c` —
    never constructed by a caller, because constructing one means choosing an
    input number and choosing an input number is the mistake this design
    exists to make unavailable.

    `c.title` and `c["title"]` are the same reference. An unknown name raises
    immediately, naming the columns there are: the alternative is an ordinal
    the server accepts and answers a different question about.
    """

    __slots__ = ("_input", "_table")

    def __init__(self, table: _TableLike, input: int) -> None:
        self._table = table
        self._input = input

    def __getattr__(self, name: str) -> ColumnRef:
        # `__getattr__` runs only for attributes normal lookup did not find, so
        # `_table` and `_input` never reach here.
        if name.startswith("_"):
            raise AttributeError(name)
        return self[name]

    def __getitem__(self, name: str) -> ColumnRef:
        ordinal = self._table.ordinal_of(name)
        if ordinal is None:
            raise KeyError(
                f"table `{self._table.name}` has no column `{name}`; it has "
                f"{', '.join(self._table.column_names)}"
            )
        return ColumnRef(
            input=self._input,
            kind=_Kind.COLUMN,
            index=ordinal,
            label=f"{self._table.name}.{name}",
            declared=self._table.type_of(name),
        )

    def at(self, ordinal: int) -> ColumnRef:
        """A column by its ordinal, for a table declared without names."""
        return ColumnRef(
            input=self._input,
            kind=_Kind.COLUMN,
            index=ordinal,
            label=f"{self._table.name}[{ordinal}]",
            declared=None,
        )

    def __repr__(self) -> str:
        return f"<Columns of {self._table.name} at input {self._input}>"


class _TableLike(Protocol):
    """What `Columns` needs from a table. Satisfied structurally by `Table`.

    A `Protocol` rather than a base class, so that `expr` does not import
    `schema` — `schema` documents the reference model by pointing here, and a
    cycle between the two would be the wrong shape for that.
    """

    @property
    def name(self) -> str: ...

    @property
    def column_names(self) -> tuple[str, ...]: ...

    def ordinal_of(self, name: str) -> int | None: ...

    def type_of(self, name: str) -> ValueType | None: ...


def computed_ref(input: int, index: int) -> ColumnRef:
    """The `index`th value an input computes.

    Internal: builders expose this as `.computed(i)`. There is no declared type
    for a computed value, so an integer compared against one has to be written
    as `i64(...)` or `u64(...)`.
    """
    return ColumnRef(input=input, kind=_Kind.COMPUTED, index=index, label=f"computed {index}")


def group_key_ref(index: int) -> ColumnRef:
    """The `index`th GROUP BY key. Only legal inside a grouped result's HAVING.

    `input` is zero and must be: the wire requires it, because a group key
    names a grouped result rather than an input.
    """
    return ColumnRef(input=0, kind=_Kind.GROUP_KEY, index=index, label=f"group key {index}")


def aggregate_ref(index: int) -> ColumnRef:
    """The `index`th aggregate. Only legal inside a grouped result's HAVING."""
    return ColumnRef(input=0, kind=_Kind.AGGREGATE, index=index, label=f"aggregate {index}")


