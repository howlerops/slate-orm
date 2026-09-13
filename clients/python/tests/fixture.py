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
ALL = (DOCS, USERS, AUTHORS, BOOKS, SALES, SECRETS)
