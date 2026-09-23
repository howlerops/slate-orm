"""The column types the kernel has.

Mirrors `slate_tuple::ValueType`. Declared here rather than imported from
anywhere, because there is nowhere to import it from: the protocol publishes no
schema, so a client's idea of a column's type is a local declaration and this
enum is the vocabulary for making one.
"""

from __future__ import annotations

import enum

__all__ = ["ValueType"]


class ValueType(enum.Enum):
    """A column's declared type."""

    BOOL = "bool"
    BYTES = "bytes"
    STR = "string"
    I64 = "i64"
    U64 = "u64"
    F64 = "f64"
    UUID = "uuid"
    VECTOR = "vector"
    #: An exact decimal. The *scale* is not part of the type — see
    #: `Column.scale`, and `values.Units` for what a decimal value is.
    DECIMAL = "decimal"
    #: A homogeneous list. The *element type* is not part of the type — see
    #: `Column.element`, and `values.Array` for what an array value is.
    ARRAY = "array"
