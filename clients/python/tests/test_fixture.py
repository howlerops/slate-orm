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

from slate import Client, Column, InvalidRequest, PermissionDenied, Query, Table, ValueType
from slate.schema import fingerprint_of

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


def test_a_decimals_scale_is_part_of_the_fingerprint() -> None:
    """The one property in the hash that addresses no column.

    Every other excluded property -- nullability, `DEFAULT`, `CHECK`, an index
    -- is excluded because getting it wrong does not make a client read the
    wrong column. A scale fails that test too and is included anyway, because
    the failure it prevents is worse: a wrong ordinal reads the wrong column
    and usually shows, a wrong scale reads the *right* column and renders every
    value a power of ten out, for ever, with nothing anywhere reporting it. The
    wire carries units and never the scale, so this hash is the only place it
    can be caught.

    Hashing it is safe in the one way that matters: a scale cannot change under
    a running client, because changing one is a refused migration. So it cannot
    do what hashing a `CHECK` would -- invalidate a fleet on an unrelated
    schema change -- since there is no such change to make.
    """

    def priced(scale: int) -> Table:
        return Table(
            name="prices",
            columns=[
                Column("id", ValueType.U64),
                Column("label", ValueType.STR),
                Column("amount", ValueType.DECIMAL, scale=scale),
            ],
            primary_key=["id"],
        )

    # This client is the port the other three pin against, so these are its own
    # numbers -- and the agreement is asserted by `review_fingerprint.rs`,
    # `schema_test.go` and `schema.test.ts` writing them down independently.
    assert fingerprint_of(priced(2)) == 0xDAB8856481BC4A6D
    assert fingerprint_of(priced(4)) == 0xDABA08FBB666133F
    assert fingerprint_of(priced(2)) != fingerprint_of(priced(4))


def test_only_a_decimal_contributes_a_scale() -> None:
    """A table with no decimal hashes exactly as it did.

    Which is why the scale is hashed where it exists rather than as a zero on
    every column: no client of a table without one needs rebuilding. The `DOCS`
    constant pinned in the Go and TypeScript suites is that claim, and it is
    unchanged.
    """
    plain = Table(
        name="prices",
        columns=[
            Column("id", ValueType.U64),
            Column("label", ValueType.STR),
            # `scale` is set and meaningless on a non-decimal column, and must
            # not reach the hash. `Column.__post_init__` refuses a non-zero one
            # on a non-decimal, so the only way to write this case is zero --
            # which is also the value a decimal at scale 0 has, and those two
            # must still hash apart because their *types* differ.
            Column("amount", ValueType.I64),
        ],
        primary_key=["id"],
    )
    at_zero = Table(
        name="prices",
        columns=[
            Column("id", ValueType.U64),
            Column("label", ValueType.STR),
            Column("amount", ValueType.DECIMAL, scale=0),
        ],
        primary_key=["id"],
    )
    assert fingerprint_of(plain) != fingerprint_of(at_zero)
