"""The generated Python decoders, run rather than only imported.

Hand-written beside a generated file, and it must stay that way: a test the
generator emitted would agree with the generator by construction.

The Go and TypeScript adapters have had this since the decoders shipped
compiled-but-never-run. Python did not, and the gap was not that Python was
untested — `scripts/test_codegen.py` executes a decoder generated from a
*synthetic* catalog — but that nothing ran *this* file, the one the demo
imports, against the ordinals *this* catalog declares. A generator that is
right about a catalog it invented and wrong about the real one passes that
suite. Two of three languages caught that; the third is this.

Run with `python3 -m pytest examples/explorer/backends/python/adapter`.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from pathlib import Path

import pytest
from slate.values import Null, Units

from .schema import (
    BOOKS_CHECKS,
    EDITIONS_FOREIGN_KEYS,
    SALES_FOREIGN_KEYS,
    SHIPMENTS_CHECKS,
    SHIPMENTS_FOREIGN_KEYS,
    Authors,
    Books,
    Editions,
    Sales,
    Shipments,
)

SCHEMA_PY = Path(__file__).with_name("schema.py")


def book_row() -> list[object]:
    """A well-formed `books` row: the ordinals the catalog declares, in order."""
    return [
        7,
        3,
        "A Book In Flight",
        2026,
        4.5,
        1_700_000_000,
        (0.1, 0.2, 0.3, 0.4),
        Units(1250),
    ]


def test_books_decodes_every_column() -> None:
    book = Books.from_row(book_row())
    assert book.id == 7
    assert book.author_id == 3
    assert book.title == "A Book In Flight"
    assert book.year == 2026
    assert book.rating == 4.5
    assert book.released == 1_700_000_000
    assert len(book.embedding) == 4
    # A decimal is a count of the smallest unit; the scale lives in the schema,
    # so Units(1250) at scale 2 is 12.50 and this type does not know that.
    assert book.price == Units(1250)


def test_books_refuses_a_transposed_row() -> None:
    row = book_row()
    row[2], row[3] = row[3], row[2]  # title <-> year
    with pytest.raises(TypeError, match=r"books\.title"):
        Books.from_row(row)


def test_books_cannot_see_a_same_typed_swap() -> None:
    # Asserted rather than left implicit, the same as the other two languages.
    # "The decoder catches transposition" grows in the retelling: it catches a
    # *type* mismatch. Same-typed neighbours are what the ordinals in the
    # generated declaration are for, and what the fingerprint protects.
    row = book_row()
    row[0], row[1] = row[1], row[0]  # id <-> author_id, both U64
    book = Books.from_row(row)
    assert book.id == 3
    assert book.author_id == 7


def test_books_refuses_a_short_row() -> None:
    with pytest.raises(ValueError, match="8 columns"):
        Books.from_row(book_row()[:4])


def test_books_refuses_a_null_in_a_non_nullable_column() -> None:
    row = book_row()
    row[2] = Null()
    with pytest.raises(ValueError, match="not nullable"):
        Books.from_row(row)


def test_a_bare_none_is_refused_like_an_explicit_null() -> None:
    # `_field` treats `None` and `Null` alike, which is a decision worth a test:
    # the wire sends `NULL_VALUE` and the client may hand back either, and a
    # decoder that accepted one and crashed on the other would fail in
    # production on whichever the demo does not happen to produce.
    row = book_row()
    row[2] = None
    with pytest.raises(ValueError, match="not nullable"):
        Books.from_row(row)


# --- every other table -------------------------------------------------------
#
# One case each rather than the six `books` gets. What differs between tables
# is the column list, not the decoder's shape — the generator emits one shape —
# so what each case asserts is that *this table's* ordinals are the catalog's.

DECODERS: dict[str, Callable[[], None]] = {
    "Authors": lambda: _authors(),
    "Books": lambda: _books(),
    "Sales": lambda: _sales(),
    "Editions": lambda: _editions(),
    "Shipments": lambda: _shipments(),
}


def _authors() -> None:
    author = Authors.from_row([3, "Ursula", "US", 1929])
    # `name` and `country` are both strings and adjacent, so the values differ
    # on purpose: a decoder reading ordinal 2 for `name` would pass a test that
    # used the same string for both.
    assert author == Authors(id=3, name="Ursula", country="US", born=1929)


def _books() -> None:
    assert Books.from_row(book_row()).title == "A Book In Flight"


def _sales() -> None:
    assert Sales.from_row([11, 7, 430]) == Sales(id=11, book_id=7, units=430)


def _editions() -> None:
    assert Editions.from_row([5, 7, "paperback"]) == Editions(
        id=5, book_id=7, format="paperback"
    )


def _shipments() -> None:
    # The only generated decoder here with a nullable column, so this is the
    # only place that branch runs. Both ways round: "null becomes None" and "a
    # value comes through" are two branches, and a test of one says nothing
    # about the other.
    live = Shipments.from_row([9, 7, "shipped", Null()])
    assert live == Shipments(id=9, book_id=7, status="shipped", deleted_at=None)

    retired = Shipments.from_row([9, 7, "shipped", 1_700_000_042])
    assert retired.deleted_at == 1_700_000_042


@pytest.mark.parametrize("name", sorted(DECODERS))
def test_each_decoder_reads_its_own_ordinals(name: str) -> None:
    DECODERS[name]()


def test_every_generated_decoder_is_exercised() -> None:
    # The table above is hand-written, which is the point — and a hand-written
    # list beside a generated file is the thing this repository has watched
    # drift five times. Reading the source turns "somebody remembers" into a
    # failure with the missing name in it.
    source = SCHEMA_PY.read_text(encoding="utf-8")
    declared = re.findall(r"^class (\w+):$", source, re.MULTILINE)
    assert len(declared) >= 5, f"found {declared}; the pattern is not matching"
    for name in declared:
        assert name in DECODERS, (
            f"{name} is generated and nothing runs it; add it to DECODERS"
        )
    for name in DECODERS:
        assert name in declared, (
            f"DECODERS names {name}, which schema.py no longer declares"
        )


# --- the published checks and foreign keys -----------------------------------


def test_books_checks_carry_the_published_rule() -> None:
    rule = BOOKS_CHECKS["year_is_positive"]
    assert rule["column"] == "year"
    assert rule["predicate"] == "year > 0"
    assert rule["message"], "the check should carry the sentence a form shows"


def test_shipments_checks_carry_the_enumeration_codegen_narrows_from() -> None:
    # The rule a *type* is generated from: `status` above is a `Literal` of
    # three strings rather than `str`, and this is what says so.
    rule = SHIPMENTS_CHECKS["status_known"]
    assert rule["column"] == "status"
    predicate = rule["predicate"]
    for value in ("pending", "shipped", "delivered"):
        assert value in predicate, f"the predicate {predicate} omits {value}"


def test_the_sales_foreign_key_carries_its_parent() -> None:
    # The whole reason foreign keys are generated: `parent` is the table a
    # `parents` read answers with, and it is the one fact a client holding a
    # relation cannot derive.
    assert SALES_FOREIGN_KEYS["sale_book"] == {
        "name": "sale_book",
        "child": "sales",
        "parent": "books",
        "on_delete": "restrict",
    }


def test_every_declared_foreign_key_is_generated() -> None:
    # Weaker than the decoder check above — the *set*, not each key's parent —
    # and here for the same reason: a fourth key added to `head.toml` should not
    # be generated into a file nothing looks at.
    generated = {
        **SALES_FOREIGN_KEYS,
        **EDITIONS_FOREIGN_KEYS,
        **SHIPMENTS_FOREIGN_KEYS,
    }
    source = SCHEMA_PY.read_text(encoding="utf-8")
    declared = re.findall(r"^(\w+)_FOREIGN_KEYS = \{$", source, re.MULTILINE)
    assert len(declared) == 3, f"three tables declare a key, found {declared}"
    assert len(generated) == 3
    # All three point at `books`, which is what lets the demo show a *path* —
    # two steps in opposite directions through one parent.
    for name, key in generated.items():
        assert key["parent"] == "books", f"{name} points at {key['parent']}"
