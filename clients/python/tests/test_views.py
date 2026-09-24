"""`Table.as_view` — a view's declaration is its base table's, renamed.

A pure test: no server, because the construction is local and the thing worth
asserting is that ordinals survive and nothing is shared. `docs/views.md`
refuses a projection precisely so that a view's ordinals *are* its base
table's; a view that quietly renumbered them would mis-decode a row rather
than raise, which is the failure this method exists to make unavailable.
"""

from __future__ import annotations

from slate.schema import Column, Table, ValueType

BOOKS = Table(
    "books",
    [
        Column("id", ValueType.U64),
        Column("title", ValueType.STR),
        Column("year", ValueType.I64),
    ],
    ["id"],
)


def test_a_view_is_its_base_tables_columns_under_another_name() -> None:
    view = BOOKS.as_view("recent_books")
    assert view.name == "recent_books"
    assert view.column_names == BOOKS.column_names
    assert view.primary_key == BOOKS.primary_key
    assert view.width == BOOKS.width


def test_every_ordinal_survives_the_rename() -> None:
    view = BOOKS.as_view("recent_books")
    for name in BOOKS.column_names:
        assert view.ordinal_of(name) == BOOKS.ordinal_of(name)
        assert view.type_of(name) == BOOKS.type_of(name)


def test_the_view_does_not_alias_the_base_tables_columns() -> None:
    """Both hold tuples, so this cannot go wrong today.

    Asserted anyway: `columns` becoming a list is a plausible change, and it
    would silently give two declarations one backing store. The Go and
    TypeScript versions copy explicitly for exactly this reason, and a test
    that only exists in two of three languages is how the third drifts.
    """
    view = BOOKS.as_view("recent_books")
    assert view.columns is not BOOKS.columns or isinstance(BOOKS.columns, tuple)


def test_a_view_of_a_view_is_just_another_rename() -> None:
    """The client cannot refuse this; the server does.

    `docs/views.md` refuses a view whose `FROM` names another view, and that
    refusal lives in the catalog because only the catalog knows what is a view.
    Renaming twice here produces a declaration the server will reject by name,
    which is the correct division: the client does not hold the registry.
    """
    twice = BOOKS.as_view("recent_books").as_view("newest_books")
    assert twice.name == "newest_books"
    assert twice.column_names == BOOKS.column_names
