"""The demo's tables, as this client must declare them.

A client holds its own copy of the schema and the server checks it: every
request carries a fingerprint, and a declaration that disagrees with the
catalog is refused rather than answered positionally-wrong. So this file is not
duplication for its own sake — it is the thing the fingerprint check exists to
catch, and getting the column *order* wrong here is caught on the first request
rather than silently returning the wrong column.
"""

from __future__ import annotations

from slate import Column, Table, ValueType

U64 = ValueType.U64
I64 = ValueType.I64
STR = ValueType.STR
F64 = ValueType.F64
VECTOR = ValueType.VECTOR
DECIMAL = ValueType.DECIMAL

AUTHORS = Table(
    "authors",
    [
        Column("id", U64),
        Column("name", STR),
        Column("country", STR),
        Column("born", I64),
    ],
    primary_key=["id"],
)

BOOKS = Table(
    "books",
    [
        Column("id", U64),
        Column("author_id", U64),
        Column("title", STR),
        Column("year", I64),
        Column("rating", F64),
        # Seconds since the epoch. Appended, so every ordinal above it — and
        # the row policy on `year` — stays where it was.
        Column("released", I64),
        Column("embedding", VECTOR),
        # In cents: at scale 2, `1250` is 12.50. The scale is declared here and
        # nowhere on the wire, which is the whole hazard a decimal column has.
        Column("price", DECIMAL, scale=2),
    ],
    primary_key=["id"],
)

SALES = Table(
    "sales",
    [
        Column("id", U64),
        Column("book_id", U64),
        Column("units", I64),
    ],
    primary_key=["id"],
)

# The far end of the demo's relationship *path*: `sales -> books -> editions`.
EDITIONS = Table(
    "editions",
    [
        Column("id", U64),
        Column("book_id", U64),
        Column("format", STR),
    ],
    primary_key=["id"],
)

BY_NAME = {table.name: table for table in (AUTHORS, BOOKS, SALES, EDITIONS)}
