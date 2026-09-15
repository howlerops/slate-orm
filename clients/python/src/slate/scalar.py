"""Computed values: `Scalar`, the wire's per-row expression.

A query's computed values are appended after its table's own columns, and a
filter, a sort key, a GROUP BY key or an aggregate names one with
`ColumnRef.computed` — so none of them has to learn what an expression is.
That is the kernel's arrangement and the wire only names the slots; this module
only builds the expressions.

Two rules the server enforces and this module surfaces rather than duplicates:

- A later computed value may read an earlier one, and reading forwards is not
  legal. The builder in `query.py` numbers them in declaration order, so
  `q.computed(0)` inside the expression for computed value 1 is the natural
  thing to write and the illegal direction is the awkward one.
- An input's computed value has no slot in a *joined* row, because the kernel
  packs a joined row by declared table width. Naming one across inputs is
  refused by the server with that reason. This client does not try to pre-empt
  the refusal: the server's message is better than anything written here, and a
  second copy of the rule is a second place for it to be wrong. (The kernel now
  has `Join::compute`, over the joined row rather than over an input, but the
  gRPC protocol does not carry it yet — so there is nothing here to build.)

Arithmetic is written as arithmetic — `s * 2` rather than `s.mul(2)` — matching
`slate_kernel::Scalar`. A bare Python `int` on either side is refused for the
reason `values.py` gives: a literal has no declared type and the wire has two
integer widths that do not compare equal.
"""

from __future__ import annotations

import dataclasses
import enum
from collections.abc import Iterable, Sequence
from typing import Literal

from ._proto.slate.v1 import records_pb2 as pb
from .expr import ColumnRef, Expr
from .values import PyValue, to_value

__all__ = [
    "CalendarPart",
    "as_scalar",
    "Metric",
    "Scalar",
    "TimeUnit",
    "calendar_part",
    "case",
    "coalesce",
    "concat",
    "date_trunc",
    "day_of_month",
    "day_of_week",
    "distance",
    "extract",
    "length",
    "lit",
    "lower",
    "month",
    "regexp_replace",
    "round_",
    "upper",
    "year",
]


class TimeUnit(enum.Enum):
    """Which part of a timestamp.

    Timestamps are seconds since the epoch in an integer column: there is no
    date type to be more precise about. `UNSPECIFIED` is not offered, because
    the server refuses it and a client that could send it would be sending a
    request it knows will fail.
    """

    SECOND = pb.TIME_UNIT_SECOND
    MINUTE = pb.TIME_UNIT_MINUTE
    HOUR = pb.TIME_UNIT_HOUR
    DAY = pb.TIME_UNIT_DAY


class CalendarPart(enum.Enum):
    """A calendar field of a timestamp, which `TimeUnit` cannot name.

    Separate from `TimeUnit` because that one promises a fixed number of
    seconds — it is how both `extract` and `date_trunc` are defined — and a
    month has none. A `MONTH` member there would give it a length that is a
    lie, and the lie would be silent.
    """

    YEAR = pb.CALENDAR_PART_YEAR
    MONTH = pb.CALENDAR_PART_MONTH
    DAY_OF_MONTH = pb.CALENDAR_PART_DAY_OF_MONTH
    #: Zero for Sunday, matching ClickHouse, MySQL and SQLite rather than ISO.
    DAY_OF_WEEK = pb.CALENDAR_PART_DAY_OF_WEEK


class Metric(enum.Enum):
    """How to measure the distance between two vectors."""

    L2 = pb.METRIC_L2
    L2_SQUARED = pb.METRIC_L2_SQUARED
    COSINE = pb.METRIC_COSINE
    NEGATIVE_INNER_PRODUCT = pb.METRIC_NEGATIVE_INNER_PRODUCT


class Scalar:
    """A value computed from a row."""

    __slots__ = ()

    def to_proto(self) -> pb.Scalar:  # pragma: no cover - overridden
        raise NotImplementedError

    def __add__(self, other: Operand) -> Scalar:
        return _Pair("add", self, as_scalar(other))

    def __sub__(self, other: Operand) -> Scalar:
        return _Pair("sub", self, as_scalar(other))

    def __mul__(self, other: Operand) -> Scalar:
        return _Pair("mul", self, as_scalar(other))

    def __truediv__(self, other: Operand) -> Scalar:
        return _Pair("div", self, as_scalar(other))

    def __radd__(self, other: Operand) -> Scalar:
        return _Pair("add", as_scalar(other), self)

    def __rsub__(self, other: Operand) -> Scalar:
        return _Pair("sub", as_scalar(other), self)

    def __rmul__(self, other: Operand) -> Scalar:
        return _Pair("mul", as_scalar(other), self)

    def __rtruediv__(self, other: Operand) -> Scalar:
        return _Pair("div", as_scalar(other), self)


#: Anything that can stand where a `Scalar` is wanted. A `ColumnRef` becomes
#: `Scalar::Column` and a Python value becomes a literal, so that
#: `upper(c.kind)` reads the way it would in SQL.
Operand = Scalar | ColumnRef | PyValue


def as_scalar(value: Operand) -> Scalar:
    """Coerce to a `Scalar`."""
    if isinstance(value, Scalar):
        return value
    if isinstance(value, ColumnRef):
        return _Column(value)
    return _Literal(to_value(value, None))


def lit(value: PyValue) -> Scalar:
    """A literal. Explicit form of the coercion, for when it reads better."""
    return _Literal(to_value(value, None))


@dataclasses.dataclass(frozen=True)
class _Column(Scalar):
    ref: ColumnRef

    def to_proto(self) -> pb.Scalar:
        return pb.Scalar(column=self.ref.to_proto())


@dataclasses.dataclass(frozen=True)
class _Literal(Scalar):
    value: pb.Value

    def to_proto(self) -> pb.Scalar:
        return pb.Scalar(literal=self.value)


# Each variant is set by name rather than through `pb.Scalar(**{op: value})`.
# The keyword-splat version is four lines shorter and is unchecked: mypy cannot
# see which field is being set, so a typo in the operator name would be a
# `Scalar` with no node set — which the server refuses at runtime, in a message
# about a client built against a newer schema. Spelling the branches out is
# what makes the operator name a type error instead.

PairOp = Literal["add", "sub", "mul", "div"]
UnaryOp = Literal["length", "lower", "upper", "round"]
ListOp = Literal["concat", "coalesce"]
TimeOp = Literal["extract", "date_trunc"]


@dataclasses.dataclass(frozen=True)
class _Pair(Scalar):
    op: PairOp
    left: Scalar
    right: Scalar

    def to_proto(self) -> pb.Scalar:
        pair = pb.ScalarPair(left=self.left.to_proto(), right=self.right.to_proto())
        if self.op == "add":
            return pb.Scalar(add=pair)
        if self.op == "sub":
            return pb.Scalar(sub=pair)
        if self.op == "mul":
            return pb.Scalar(mul=pair)
        return pb.Scalar(div=pair)


@dataclasses.dataclass(frozen=True)
class _Unary(Scalar):
    op: UnaryOp
    inner: Scalar

    def to_proto(self) -> pb.Scalar:
        inner = self.inner.to_proto()
        if self.op == "length":
            return pb.Scalar(length=inner)
        if self.op == "lower":
            return pb.Scalar(lower=inner)
        if self.op == "upper":
            return pb.Scalar(upper=inner)
        return pb.Scalar(round=inner)


@dataclasses.dataclass(frozen=True)
class _List(Scalar):
    op: ListOp
    parts: Sequence[Scalar]

    def to_proto(self) -> pb.Scalar:
        items = pb.ScalarList(scalars=[p.to_proto() for p in self.parts])
        if self.op == "concat":
            return pb.Scalar(concat=items)
        return pb.Scalar(coalesce=items)


@dataclasses.dataclass(frozen=True)
class _TimePart(Scalar):
    op: TimeOp
    unit: TimeUnit
    value: Scalar

    def to_proto(self) -> pb.Scalar:
        part = pb.TimePart(unit=self.unit.value, value=self.value.to_proto())
        if self.op == "extract":
            return pb.Scalar(extract=part)
        return pb.Scalar(date_trunc=part)


@dataclasses.dataclass(frozen=True)
class _CalendarField(Scalar):
    part: CalendarPart
    value: Scalar

    def to_proto(self) -> pb.Scalar:
        return pb.Scalar(
            calendar_part=pb.CalendarField(
                part=self.part.value, value=self.value.to_proto()
            )
        )


@dataclasses.dataclass(frozen=True)
class _Case(Scalar):
    branches: Sequence[tuple[Expr, Scalar]]
    otherwise: Scalar

    def to_proto(self) -> pb.Scalar:
        return pb.Scalar(
            case=pb.Case(
                branches=[
                    pb.CaseBranch(when=when.to_proto(), then=then.to_proto())
                    for when, then in self.branches
                ],
                otherwise=self.otherwise.to_proto(),
            )
        )


@dataclasses.dataclass(frozen=True)
class _Distance(Scalar):
    left: Scalar
    right: Scalar
    metric: Metric

    def to_proto(self) -> pb.Scalar:
        return pb.Scalar(
            distance=pb.Distance(
                left=self.left.to_proto(),
                right=self.right.to_proto(),
                metric=self.metric.value,
            )
        )


@dataclasses.dataclass(frozen=True)
class _RegexpReplace(Scalar):
    value: Scalar
    pattern: str
    replacement: str

    def to_proto(self) -> pb.Scalar:
        return pb.Scalar(
            regexp_replace=pb.RegexpReplace(
                value=self.value.to_proto(),
                pattern=self.pattern,
                replacement=self.replacement,
            )
        )


def length(value: Operand) -> Scalar:
    """The length of a string or a byte string."""
    return _Unary("length", as_scalar(value))


def lower(value: Operand) -> Scalar:
    """Lowercase."""
    return _Unary("lower", as_scalar(value))


def upper(value: Operand) -> Scalar:
    """Uppercase."""
    return _Unary("upper", as_scalar(value))


def concat(*parts: Operand) -> Scalar:
    """String concatenation."""
    return _List("concat", [as_scalar(p) for p in parts])


def coalesce(*parts: Operand) -> Scalar:
    """The first part that is not null."""
    return _List("coalesce", [as_scalar(p) for p in parts])


def extract(unit: TimeUnit, value: Operand) -> Scalar:
    """One part of a timestamp, as a number."""
    return _TimePart("extract", unit, as_scalar(value))


def date_trunc(unit: TimeUnit, value: Operand) -> Scalar:
    """A timestamp truncated to `unit`."""
    return _TimePart("date_trunc", unit, as_scalar(value))


def calendar_part(part: CalendarPart, value: Operand) -> Scalar:
    """A calendar field of a timestamp: the year, the day of the week.

    Timestamps are seconds since the epoch in an integer column, read in UTC.
    There is no timezone here and no date type to carry one; shifting to
    another fixed offset is `calendar_part(part, column + 3600 * hours)`, which
    is what such a conversion is.
    """
    return _CalendarField(part, as_scalar(value))


def year(value: Operand) -> Scalar:
    """The year. Proleptic Gregorian, negative before 1 CE."""
    return calendar_part(CalendarPart.YEAR, value)


def month(value: Operand) -> Scalar:
    """The month, 1 to 12."""
    return calendar_part(CalendarPart.MONTH, value)


def day_of_month(value: Operand) -> Scalar:
    """The day of the month, 1 to 31.

    Named in full rather than `day`, because `TimeUnit.DAY` counts days since
    the epoch and the two are both integers that look plausible in a column.
    SQL's `EXTRACT(DAY FROM t)` means this one; `extract(TimeUnit.DAY, t)`
    means the other.
    """
    return calendar_part(CalendarPart.DAY_OF_MONTH, value)


def day_of_week(value: Operand) -> Scalar:
    """The day of the week, 0 for Sunday through 6 for Saturday."""
    return calendar_part(CalendarPart.DAY_OF_WEEK, value)


def round_(value: Operand) -> Scalar:
    """A number rounded to the nearest integer, halves away from zero.

    Returns an integer, so it can be a group key without the grouping
    depending on float equality. Trailing underscore because `round` is a
    builtin, and shadowing it in a module people do `from slate.scalar import
    *` on would be a trap.
    """
    return _Unary("round", as_scalar(value))


def case(branches: Iterable[tuple[Expr, Operand]], otherwise: Operand) -> Scalar:
    """SQL's `CASE WHEN`.

    `otherwise` is required, because SQL's `CASE` with no `ELSE` produces null
    and sending an explicit null literal says so — where an absent field would
    be a client that forgot. The wire makes the same demand; this signature
    just makes it impossible to omit.
    """
    return _Case(
        [(when, as_scalar(then)) for when, then in branches], as_scalar(otherwise)
    )


def distance(left: Operand, right: Operand, metric: Metric) -> Scalar:
    """The distance between two vectors."""
    return _Distance(as_scalar(left), as_scalar(right), metric)


def regexp_replace(value: Operand, pattern: str, replacement: str) -> Scalar:
    """Replace every match of `pattern` in `value`."""
    return _RegexpReplace(as_scalar(value), pattern, replacement)
