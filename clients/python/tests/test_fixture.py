"""Is the schema this client declares the schema the server has?

The protocol publishes no schema, so a client's `Table` is a local guess and
every ordinal it produces is a number that came from that guess. `ColumnRef`
removed the *cross-table* arithmetic; this is what is left, and it has the same
failure shape: a `Table` that disagrees with the catalog points every reference
past the disagreement at the wrong column, and the server cannot tell.

What can be checked from a client is the width, by reading a row. That catches
a column added or removed. It does not catch a rename, and it does not catch
two columns of the same type swapped — both of which re-point every reference
between them at something plausible. Stated in a test rather than in a comment
so the limit is as visible as the check.
"""

from __future__ import annotations

import pytest

from slate import Client, InvalidRequest, PermissionDenied, Query, Table

from .fixture import AUTHORS, BOOKS, DOCS, SALES, SECRETS, USERS


@pytest.mark.parametrize("table", [DOCS, USERS, AUTHORS, BOOKS, SALES], ids=lambda t: t.name)
def test_the_declared_width_matches_the_rows_the_server_returns(
    oracle_client: Client, table: Table
) -> None:
    rows = list(oracle_client.query(Query(table).limit(1)))
    assert rows, (
        f"`{table.name}` returned no visible row, so its width was not checked. "
        "A width check against an empty table passes and proves nothing."
    )
    assert len(rows[0].values) == table.width, (
        f"this client declares {table.width} columns for `{table.name}` and the "
        f"server returned {len(rows[0].values)}. Every ordinal past the "
        "disagreement names a different column."
    )


def test_a_table_with_no_grant_is_denied_rather_than_absent(
    oracle_client: Client,
) -> None:
    """The name resolves; the read does not.

    Which is the distinction that makes `PermissionDenied` worth having: an
    unknown table is `NOT_FOUND`, and a table the caller may not read is not.
    """
    with pytest.raises(PermissionDenied):
        list(oracle_client.query(Query(SECRETS)))


def test_a_renamed_column_is_refused_rather_than_silently_answered(
    oracle_client: Client,
) -> None:
    """PROTOCOL FINDING 2, since fixed: the server now checks the client's claim.

    This was pinned the other way round, as the gap it named. `renamed`
    declares the same *shape* as `docs` with one column called something else,
    so the client's only available check — compare the returned row's width
    against the declared width — passes, and every reference built from it is a
    well-formed ordinal naming a different column than the caller wrote. The
    query was answered. Nothing at any layer could see it.

    The fix is a schema fingerprint travelling on every request that names a
    table: the table name, and per ordinal the column's name and declared type,
    plus the primary key and the count. The width check this test was named for
    is now the weaker half of a real one.
    """
    from slate import Column, ValueType

    renamed = Table(
        "docs",
        [
            Column("id", ValueType.U64),
            Column("category", ValueType.STR),  # really `kind`
            Column("size", ValueType.I64),
            Column("note", ValueType.STR),
        ],
        primary_key=["id"],
    )
    with pytest.raises(InvalidRequest):
        list(oracle_client.query(Query(renamed).limit(1)))

    # The control: the same shape spelled correctly is served, so the refusal
    # is about the name and not about the check refusing everything.
    correct = Table(
        "docs",
        [
            Column("id", ValueType.U64),
            Column("kind", ValueType.STR),
            Column("size", ValueType.I64),
            Column("note", ValueType.STR),
        ],
        primary_key=["id"],
    )
    assert list(oracle_client.query(Query(correct).limit(1)))
