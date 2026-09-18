"""The schema the test server serves, declared again on this side.

This is the third statement of one schema — the server's catalog, the test
server binary's `TableDef`s, and this. That is not a design anybody would
choose; it is what a protocol with no `Describe` costs a client in another
language, and it is the finding this file exists to demonstrate as much as to
serve.

`test_fixture.py` checks what can be checked: that each declared width matches
the row the server actually returns. That catches a column added or removed. It
does not catch a rename or a reorder that preserves the width, and nothing
available to a client can.
"""

from __future__ import annotations

from slate import Column, Table, ValueType

I64 = ValueType.I64
U64 = ValueType.U64
STR = ValueType.STR
DECIMAL = ValueType.DECIMAL

DOCS = Table(
    "docs",
    [
        Column("id", U64),
        Column("kind", STR),
        Column("size", I64),
        Column("note", STR),
    ],
    primary_key=["id"],
)

USERS = Table(
    "users",
    [
        Column("tenant_id", U64),
        Column("id", U64),
        Column("owner", U64),
        Column("email", STR),
    ],
    primary_key=["tenant_id", "id"],
)

AUTHORS = Table(
    "authors",
    [
        Column("tenant_id", U64),
        Column("id", U64),
        Column("owner", U64),
        Column("name", STR),
        Column("country", STR),
        Column("born", I64),
    ],
    primary_key=["tenant_id", "id"],
)

BOOKS = Table(
    "books",
    [
        Column("tenant_id", U64),
        Column("id", U64),
        Column("author_id", U64),
        Column("title", STR),
        Column("year", I64),
    ],
    primary_key=["tenant_id", "id"],
)

SALES = Table(
    "sales",
    [
        Column("tenant_id", U64),
        Column("id", U64),
        Column("book_id", U64),
        Column("units", I64),
    ],
    primary_key=["tenant_id", "id"],
)

#: A table the `app` role has no grant on. Reading it is PERMISSION_DENIED,
#: which is a different thing from a policy quietly returning fewer rows.
SECRETS = Table(
    "secrets",
    [Column("id", U64), Column("value", STR)],
    primary_key=["id"],
)

#: Every table the server serves, for the width check.
LIBRARIES = Table(
    "libraries",
    [
        Column("tenant_id", U64),
        Column("id", U64),
        Column("name", STR),
    ],
    primary_key=["tenant_id", "id"],
)

# The child of a real foreign key, `shelf_library`, which is what makes a
# relationship nameable — see the note beside these two in the testserver.
SHELVES = Table(
    "shelves",
    [
        Column("tenant_id", U64),
        Column("id", U64),
        Column("library_id", U64),
        Column("label", STR),
    ],
    primary_key=["tenant_id", "id"],
)

# A second foreign key, one level further down, so a relationship *path* has
# somewhere to go: `libraries → shelves → copies`.
COPIES = Table(
    "copies",
    [
        Column("tenant_id", U64),
        Column("id", U64),
        Column("shelf_id", U64),
        Column("barcode", STR),
    ],
    primary_key=["tenant_id", "id"],
)

# A decimal column, at a scale that is neither 0 nor 1 — at scale 0 a decimal
# is indistinguishable from an `i64` in every assertion, which is the one scale
# a test of decimals must not use. `1250` here is 12.50.
#
# The scale is declared and *is* part of the fingerprint, which is what makes
# this suite an end-to-end check of that: the testserver declares `amount` at
# scale 2 too, so every request here that carries a schema check is one the
# server accepts only because the two agree. Change this 2 and the whole file
# is refused — which was the point of hashing it. It is the one property in
# the fingerprint that addresses no column, and it is there because a wrong
# scale reaches the *right* column and renders every value a power of ten out,
# with the wire carrying units and never the scale to notice by.
PRICES = Table(
    "prices",
    [
        Column("id", U64),
        Column("label", STR),
        Column("amount", DECIMAL, scale=2),
    ],
    primary_key=["id"],
)

ALL = (
    DOCS,
    USERS,
    AUTHORS,
    BOOKS,
    SALES,
    SECRETS,
    LIBRARIES,
    SHELVES,
    COPIES,
    PRICES,
)
