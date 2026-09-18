"""Narrowing a read-back column to the type this harness wants.

`Row.get` and `Group.aggregates` hand back the whole `PyValue` union, because a
column outside a projection comes back as `None` and the column's declared type
is not something a row carries. Every comparison in `check.py` wants a concrete
`int` or `float`, and a type checker is right to refuse the union.

A `cast` would satisfy the checker and check nothing. These assert, so a column
that comes back as the wrong type — a decoder bug, which is exactly what this
harness exists to catch on a *real* deployment rather than against an in-memory
store — fails here, with the value in the message, instead of surfacing as a
confusing comparison a hundred lines later.

The same three helpers exist in `clients/python/tests/conftest.py` and are
deliberately not shared: this directory is an example, it runs against an
installed `slate-client` rather than inside the package, and importing out of
another project's test suite to get three assertions would be a worse
dependency than restating them.
"""

from __future__ import annotations

from slate import PyValue


def as_int(value: PyValue) -> int:
    """`value` as an `int`, asserting rather than casting.

    `bool` is excluded on purpose: it is an `int` to Python and is never the
    answer to "how many trips are there".
    """
    assert isinstance(value, int) and not isinstance(value, bool), (
        f"expected an integer column, got {value!r}"
    )
    return value


def as_float(value: PyValue) -> float:
    """`value` as a `float`.

    An `int` is accepted and widened: an `AVG` over an integer column is a
    double on the wire, but a `SUM` is not, and a caller averaging fares should
    not have to know which of the two it is holding.
    """
    assert isinstance(value, (int, float)) and not isinstance(value, bool), (
        f"expected a numeric column, got {value!r}"
    )
    return float(value)


def as_str(value: PyValue) -> str:
    """`value` as a `str`.

    Not only for the checker: `f"{value}"` on a `bytes` column produces
    `b'...'` rather than the text, so a check comparing an interpolated column
    against a server-computed string would fail with a message that looks like
    a computation bug and is really a decoding one.
    """
    assert isinstance(value, str), f"expected a string column, got {value!r}"
    return value
