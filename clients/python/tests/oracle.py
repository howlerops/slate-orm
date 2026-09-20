"""Comparing a value this client decoded with one the kernel produced.

Both sides are tagged with the type, not just the number. A bare `5` cannot
tell `U64(5)` from `I64(5)`, and the kernel orders values type first — so a
client that sent the wrong integer width would build a predicate that matches
nothing, and an untagged comparison would agree with it perfectly.

Floats are the exception, and are compared as parsed numbers rather than as
text: Rust prints `2007f64` as "2007" and Python prints it as "2007.0", and a
string comparison would fail on a difference that is not a disagreement about
the value.
"""

from __future__ import annotations

import json
import pathlib
import uuid
from collections.abc import Sequence
from typing import Any

from slate import Array, Group, JoinedRow, PyValue, Row, Vector

Tagged = tuple[str, Any]


def tag(value: PyValue) -> Tagged:
    """A decoded Python value in the tagged form the oracle file uses."""
    if value is None:
        return ("null", None)
    # Before `int`: `bool` is an `int` in Python and would tag as a number.
    if isinstance(value, bool):
        return ("bool", value)
    if isinstance(value, int):
        # `from_value` returns `i64`/`u64`, which are `int` subclasses that
        # remember which of the wire's two integer types they came from. That
        # memory is the entire point of the tag.
        return (type(value).__name__, int(value))
    if isinstance(value, float):
        return ("f64", value)
    if isinstance(value, str):
        return ("str", value)
    if isinstance(value, uuid.UUID):
        return ("uuid", str(value))
    if isinstance(value, Vector):
        return ("vector", [float(x) for x in value])
    # Before `bytes`, and before the fall-through: `Array` is a `tuple`
    # subclass and nothing above it matches one. Each element is tagged in
    # turn, so an element's *type* is compared and not only its text — a list
    # of `["1"]` decoded as strings must not agree with one decoded as
    # integers, which is the confusion the tagging exists to stop, one level
    # down.
    if isinstance(value, Array):
        return ("array", [tag(element) for element in value])
    if isinstance(value, bytes):
        return ("bytes", list(value))
    raise TypeError(f"nothing tags a {type(value).__name__}")


def untag(entry: dict[str, Any]) -> Tagged:
    """One value from the oracle file, in the same form."""
    (kind, raw), = entry.items()
    if kind == "f64":
        return ("f64", float(raw))
    if kind == "vector":
        return ("vector", [float(x) for x in raw])
    if kind == "array":
        return ("array", [untag(element) for element in raw])
    if kind == "null":
        return ("null", None)
    return (kind, raw)


def tag_row(row: Row) -> list[Tagged]:
    return [tag(v) for v in row.values]


def untag_row(entry: list[dict[str, Any]]) -> list[Tagged]:
    return [untag(v) for v in entry]


def tag_joined(row: JoinedRow) -> list[list[Tagged] | None]:
    return [None if r is None else tag_row(r) for r in row]


def untag_joined(entry: list[list[dict[str, Any]] | None]) -> list[list[Tagged] | None]:
    return [None if r is None else untag_row(r) for r in entry]


def tag_group(group: Group) -> dict[str, list[Tagged]]:
    return {
        "key": [tag(v) for v in group.key],
        "values": [tag(v) for v in group.aggregates],
    }


def untag_group(entry: dict[str, list[dict[str, Any]]]) -> dict[str, list[Tagged]]:
    return {"key": untag_row(entry["key"]), "values": untag_row(entry["values"])}


def multiset(rows: Sequence[object]) -> list[str]:
    """Rows as a sorted list of strings.

    Sorted because a join has no inherent order and the algorithms genuinely
    produce different ones — demanding an order would test the implementation
    rather than the semantics. Strings so that a mismatch prints the rows
    rather than a length.
    """
    return sorted(repr(r) for r in rows)


class Oracle:
    """The kernel's answers, keyed by case name."""

    def __init__(self, path: pathlib.Path) -> None:
        self._answers: dict[str, Any] = json.loads(path.read_text())
        self._used: set[str] = set()

    def rows(self, name: str) -> list[list[Tagged]]:
        self._used.add(name)
        return [untag_row(r) for r in self._answers[name]]

    def joined(self, name: str) -> list[list[list[Tagged] | None]]:
        self._used.add(name)
        return [untag_joined(r) for r in self._answers[name]]

    def groups(self, name: str) -> list[dict[str, list[Tagged]]]:
        self._used.add(name)
        return [untag_group(g) for g in self._answers[name]]

    @property
    def names(self) -> set[str]:
        return set(self._answers)

    @property
    def used(self) -> set[str]:
        return self._used
