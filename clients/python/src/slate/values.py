"""Python values on the wire, and the one place a type is guessed at — nowhere.

# The problem this module exists to have an opinion about

`Value` is a `oneof` with an `int64_value` and a `uint64_value`, and the
kernel's ordering is type-first: `U64(5)` and `I64(5)` are not the same value
and comparing them is `ComparisonTypeMismatch`, not equality. So a predicate
`size = 5` must carry the integer width the column was *declared* with, and a
Python `int` does not say which it is.

The protocol publishes no schema — deliberately; the `.proto`'s opening comment
refuses one — so nothing on the wire can tell a client. Three ways out:

1. **Guess.** Default a Python `int` to `int64_value`. This is what a client
   written in an afternoon does, and it is wrong on every `u64` column: the
   predicate does not match a row, it fails a type check or silently selects
   nothing. Rejected — it is the "same query, different answer" shape, arriving
   at whichever column happens to be unsigned.
2. **Make the caller wrap every integer**: `i64(5)`, `u64(5)`. Correct, and
   unbearable.
3. **Let the caller declare the schema once**, and use the declared type of the
   column a value is being compared against. This is what `Table` in
   `schema.py` is for, and it is what this module does.

So: `to_value(x, hint)` takes the declared type of the slot the value is going
into, when there is one, and coerces a bare `int` to it. Where there is no
declared type — a computed value, a group key, an aggregate, anything the
server invents — a bare `int` is **refused** with a message naming `i64` and
`u64`, rather than defaulted. Refusing is the same choice the server makes for
an unset `oneof`, for the same reason.

`bool`, `str`, `float`, `bytes` and `UUID` are unambiguous and need no hint.
`None` is `NULL_VALUE` — an explicit null, which is the only kind the server
accepts.

A decimal is the same problem a third time, and gets the same answer. The wire
carries a count of the column's smallest unit and nothing else; the *scale* is
the column's and lives in the catalog. So `Units(1250)` states a decimal
explicitly, a bare `int` against a column declared `ValueType.DECIMAL` is taken
as units, and a `decimal.Decimal` is **refused** — it carries a scale of its
own, there is no schema on the wire to reconcile it against, and guessing would
be off by a factor of ten.
"""

from __future__ import annotations

import decimal as _decimal
import uuid as _uuid
from collections.abc import Sequence

from ._proto.slate.v1 import records_pb2 as pb
from .types import ValueType

__all__ = [
    "NULL",
    "Array",
    "Null",
    "PyValue",
    "Units",
    "Vector",
    "from_value",
    "i64",
    "to_value",
    "u64",
]


class Null:
    """An explicit null.

    A singleton rather than `None`, in the places where `None` already means
    "not set" — a projection that is absent is not a projection of null. Both
    `None` and `NULL` encode to `NULL_VALUE`; `NULL` exists so that a caller
    can write one without ambiguity in a container.
    """

    _instance: Null | None = None

    def __new__(cls) -> Null:
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __repr__(self) -> str:
        return "NULL"


NULL = Null()


class _Tagged(int):
    """An integer that remembers which of the wire's two integer types it is.

    A subclass of `int` so that it can be used anywhere an `int` can — the
    caller writes `u64(5)` and everything downstream, including their own
    arithmetic, keeps working.
    """

    __slots__ = ()


class i64(_Tagged):
    """A signed 64-bit integer, stated explicitly."""

    __slots__ = ()


class u64(_Tagged):
    """An unsigned 64-bit integer, stated explicitly."""

    __slots__ = ()


class Units(_Tagged):
    """A count of a decimal column's smallest unit.

    The same type the Rust surface has, and for the same reason: **the scale
    lives in the schema, not in the value.** `Units(1250)` in a column declared
    `scale=2` is 12.50, and the identical value in a `scale=0` column is 1250.
    Nothing on the wire says which, because the protocol publishes no schema.

    So this does not accept a `decimal.Decimal` and does not multiply anything.
    `to_string_with_scale` is how a value becomes a number a person reads, and
    it takes the scale as an argument because a value does not have one.
    """

    __slots__ = ()

    def to_string_with_scale(self, scale: int) -> str:
        """Render against `scale`, as a decimal string.

        Mirrors `slate_orm::Units::to_string_with_scale`, and the conformance
        corpus compares the two.
        """
        if scale < 0:
            raise ValueError(f"a scale is not negative, got {scale}")
        if scale == 0:
            return str(int(self))
        whole, part = divmod(abs(int(self)), 10**scale)
        return f"{'-' if int(self) < 0 else ''}{whole}.{part:0{scale}d}"


class Vector(tuple[float, ...]):
    """A dense f32 vector, for an embedding column.

    A distinct type because a `list[float]` is otherwise indistinguishable from
    a caller passing the wrong thing, and because `Value` has a `double_value`
    that a list of floats is *not*.
    """

    __slots__ = ()

    def __new__(cls, elements: Sequence[float]) -> Vector:
        return super().__new__(cls, (float(e) for e in elements))


class Array(tuple["PyValue", ...]):
    """A homogeneous list, for an array column.

    A distinct type for the reason `Vector` is one: a bare `list` is
    indistinguishable from a caller passing the wrong thing, and a `tuple`
    already means something else here.

    **Homogeneous by the column's declaration, not by this type.** The element
    type lives on the column — the way a decimal's scale does — and the
    protocol publishes no schema, so this client cannot check it. A mixed list
    encodes fine and is refused by the server, naming the element that did not
    match; that is the same place a wrong scale is caught, and for the same
    reason.

    An `Array` may not hold another `Array`: a column's element type is a
    scalar type name and cannot say what an inner list would hold. The server
    refuses one.

    The elements keep their Python types, so `Array(["a", "b"])` is a list of
    strings and `Array([i64(1)])` is a list of `i64`. A bare `int` inside one
    is refused for the same reason a bare `int` anywhere is — the wire has two
    integer widths and nothing here says which — except that the column's hint
    cannot help, because a hint names the *array*, not its elements.
    """

    __slots__ = ()

    def __new__(cls, elements: Sequence[PyValue]) -> Array:
        return super().__new__(cls, elements)


#: Everything this client will encode. `int` is here and is refused without a
#: hint; see the module docstring. `Units` is an `int` subclass, so it needs no
#: arm of its own.
PyValue = None | Null | bool | int | float | str | bytes | _uuid.UUID | Vector | Array


class ValueTypeError(TypeError):
    """A Python value that cannot be encoded, or cannot be encoded unambiguously."""


def to_value(value: PyValue, hint: ValueType | None = None) -> pb.Value:
    """`value` in its wire form, using `hint` to disambiguate an integer.

    `hint` is the *declared* type of the column or slot the value is going
    into, or `None` where the slot has no declared type.
    """
    if value is None or isinstance(value, Null):
        return pb.Value(null_value=pb.NULL_VALUE)

    # Before `int`: `bool` is a subclass of `int` in Python, and a `bool`
    # reaching the integer branch would be silently encoded as 0 or 1 into a
    # numeric column. That is a type confusion the server cannot catch, because
    # `Int64Value(1)` is a perfectly good integer.
    if isinstance(value, bool):
        return pb.Value(bool_value=value)

    if isinstance(value, i64):
        return pb.Value(int64_value=int(value))
    if isinstance(value, u64):
        return pb.Value(uint64_value=int(value))
    if isinstance(value, Units):
        return pb.Value(decimal_value=int(value))

    if isinstance(value, int):
        if hint is ValueType.I64:
            return pb.Value(int64_value=value)
        if hint is ValueType.U64:
            return pb.Value(uint64_value=value)
        if hint is ValueType.DECIMAL:
            # The units, not the number: an `int` going into a decimal column
            # is already a count of the column's smallest unit, exactly as
            # `Units` would be. Coercing here rather than refusing keeps the
            # rule the module docstring states — a declared slot disambiguates
            # a bare `int` — and a caller who finds that surprising is a caller
            # who has not read the column's scale, which no client can fix.
            return pb.Value(decimal_value=value)
        raise ValueTypeError(
            f"the integer {value} is going into a slot with no declared type, and the "
            "wire has both `int64_value` and `uint64_value`. The kernel orders values "
            "type first, so the wrong one does not compare equal — it is a type "
            "mismatch or an empty result. Write `i64(...)` or `u64(...)`."
            + ("" if hint is None else f" (the slot is declared {hint.name.lower()})")
        )

    if isinstance(value, float):
        return pb.Value(double_value=value)
    if isinstance(value, str):
        return pb.Value(string_value=value)
    if isinstance(value, _uuid.UUID):
        # Exactly sixteen bytes, big-endian, as `Uuid::as_bytes` gives them.
        return pb.Value(uuid_value=value.bytes)
    if isinstance(value, Vector):
        return pb.Value(vector_value=pb.Vector(elements=list(value)))
    if isinstance(value, Array):
        # No hint is passed down. A hint is the declared type of the slot, and
        # the slot here is the array — the element type is on the column and
        # this protocol publishes no schema, so there is nothing truthful to
        # pass. A bare `int` element is therefore refused, the way a bare `int`
        # in a slot with no declared type is, and the message says to write
        # `i64(...)` or `u64(...)`. Guessing one would be the type confusion
        # this whole module is arranged to prevent.
        return pb.Value(
            array_value=pb.ArrayValue(elements=[to_value(element) for element in value])
        )
    if isinstance(value, bytes):
        return pb.Value(bytes_value=value)

    if isinstance(value, _decimal.Decimal):
        raise ValueTypeError(
            "a `decimal.Decimal` has a scale of its own and a decimal column has "
            "one too, and nothing on the wire reconciles them: the protocol "
            "publishes no schema, so this client cannot know that 12.50 into a "
            "`scale=2` column is 1250 and into a `scale=4` column is 125000. "
            "Scale it yourself and send `Units(...)` — the same thing the Rust "
            "surface requires."
        )

    raise ValueTypeError(f"cannot put {type(value).__name__} on the wire")


def from_value(value: pb.Value) -> PyValue:
    """A wire value as Python.

    Integers come back as `i64` or `u64` rather than as a bare `int`. That
    looks like pedantry until a caller takes a value out of a row and puts it
    back into a filter: a bare `int` would have lost which width it was and
    would be refused by `to_value`, or worse, coerced to the other one. Both
    are `int` subclasses, so `row[0] == 5` and `row[0] + 1` behave normally.
    """
    kind = value.WhichOneof("kind")
    if kind is None:
        # The server refuses this on the way in for the reason its comment
        # gives, and the same reasoning applies coming out: an unset `kind` is
        # a message from a schema this client does not have, and reading it as
        # a null would put a wrong value in a row.
        raise ValueTypeError(
            "a value arrived from the server with no kind set; this client was "
            "built against an older schema than the server is speaking"
        )
    if kind == "null_value":
        return None
    if kind == "bool_value":
        return value.bool_value
    if kind == "int64_value":
        return i64(value.int64_value)
    if kind == "uint64_value":
        return u64(value.uint64_value)
    if kind == "decimal_value":
        return Units(value.decimal_value)
    if kind == "double_value":
        return value.double_value
    if kind == "string_value":
        return value.string_value
    if kind == "uuid_value":
        return _uuid.UUID(bytes=value.uuid_value)
    if kind == "vector_value":
        return Vector(value.vector_value.elements)
    if kind == "array_value":
        # Recursing rather than matching the scalar kinds again, so the two
        # cannot disagree about what a `uuid_value` decodes to; an element the
        # server should never send raises the same error any other unreadable
        # value would.
        return Array([from_value(element) for element in value.array_value.elements])
    if kind == "bytes_value":
        return value.bytes_value
    raise ValueTypeError(f"unknown value kind `{kind}`")
