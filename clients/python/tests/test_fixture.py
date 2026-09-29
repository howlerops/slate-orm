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

#: The tables whose fingerprints are pinned across all four implementations.
#:
#: At module scope so `test_every_value_type_appears_in_a_pinned_table` can
#: *derive* which types are covered instead of repeating a list — a guard whose
#: expectation is hand-written is the same maintenance problem it guards.
PINNED_DOCS = Table(
    name="docs",
    columns=[
        Column("id", ValueType.U64),
        Column("kind", ValueType.STR),
        Column("size", ValueType.I64),
    ],
    primary_key=["id"],
)
PINNED_SHELVES = Table(
    name="shelves",
    columns=[
        Column("id", ValueType.U64),
        Column("tags", ValueType.ARRAY, element=ValueType.STR),
        Column("price", ValueType.DECIMAL, scale=2),
    ],
    primary_key=["id"],
)
READINGS = Table(
    name="readings",
    columns=[
        Column("id", ValueType.U64),
        Column("ok", ValueType.BOOL),
        Column("raw", ValueType.BYTES),
        Column("weight", ValueType.F64),
        Column("tag", ValueType.UUID),
        Column("point", ValueType.VECTOR),
    ],
    primary_key=["id"],
)


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


def test_every_column_type_contributes_to_the_fingerprint() -> None:
    """The five types no pinned table carried.

    `DOCS` is u64/str/i64 and `SHELVES` is u64/array/decimal, so between them a
    port could misspell `bool`, `bytes`, `f64`, `uuid` or `vector` in its hash
    and every pinned value stayed green. That is the residual
    `ledger/2026-09-20-an-array-on-the-wire-and-in-three-clients.md` recorded:
    two tables were pinned, not the type surface.

    Each of the five is swapped for `STR` in turn and must move the number.
    That is what makes one table a pin for five types rather than for the first
    one that happens to differ -- and it is checked here rather than asserted,
    because "the hash reads every column's type" is exactly the kind of claim
    that is true of the code somebody read and false of the code somebody
    wrote.
    """
    base = list(READINGS.columns)
    readings = READINGS

    # This client is the port the other three pin against; `review_fingerprint.rs`,
    # `schema_test.go` and `schema.test.ts` each write this number down
    # independently, which is what makes it evidence rather than self-agreement.
    assert fingerprint_of(readings) == 0x9EB9CC433C353EB1

    for index in range(1, len(base)):
        swapped = list(base)
        swapped[index] = Column(base[index].name, ValueType.STR)
        assert fingerprint_of(
            Table(name="readings", columns=swapped, primary_key=["id"])
        ) != fingerprint_of(readings), (
            f"`{base[index].name}` is declared {base[index].type.value} and the "
            "fingerprint does not read it"
        )


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


def _unpinned_types(*tables: Table) -> list[str]:
    """The `ValueType` members no column of `tables` declares, by name.

    A function rather than an expression inside the test, so the *never-fires*
    case can call it with a short list. A guard whose computation exists only
    where it is asserted cannot be shown to fire.
    """
    covered = {column.type for table in tables for column in table.columns}
    return sorted(t.value for t in ValueType if t not in covered)


def test_every_value_type_appears_in_a_pinned_table() -> None:
    """An eleventh column type must not arrive with no pinned fingerprint.

    Five of the ten were in that position until
    `ledger/2026-09-28-five-column-types-no-pinned-table-carried.md`: `docs`
    and `shelves` between them covered neither `bool`, `bytes`, `f64`, `uuid`
    nor `vector`, so a port misspelling one of those produced a fingerprint the
    server refuses for exactly the tables that use it.

    Closing those five does not close the shape, which is what this is for. The
    covered set is *derived* from the pinned tables rather than listed, so
    adding a type to `ValueType` fails here until some pinned table carries it
    — and the three constants beside them are what make that pin evidence.
    """
    missing = _unpinned_types(PINNED_DOCS, PINNED_SHELVES, READINGS)
    assert not missing, (
        f"{missing} appear in no pinned table, so a port could misspell one in "
        "its fingerprint and every pinned value would stay green. Add a column "
        "of that type to one of the pinned tables, recompute its fingerprint "
        "here, and write the new number down in review_fingerprint.rs, "
        "schema_test.go and schema.test.ts."
    )


def test_the_coverage_guard_fires_when_a_type_is_unpinned() -> None:
    """The never-fires half, without which the guard above proves nothing.

    A guard that computes an empty answer passes for ever and reads exactly
    like a guard that is satisfied -- `scripts/mutate.py` found this one by
    replacing the computation with `[]` and watching the suite stay green. So
    the computation is exercised against a set that is deliberately short: with
    only `docs` pinned, the seven types it does not carry must be named.
    """
    short = _unpinned_types(PINNED_DOCS)
    assert "vector" in short and "bool" in short, short
    assert "u64" not in short, "docs carries u64, so it is not missing"
    # And the real answer is not simply everything: `docs` covers three.
    assert len(short) == len(list(ValueType)) - 3, short


def test_a_renamed_column_is_accepted_under_its_previous_name(client: Client) -> None:
    """The check Go and TypeScript had and this client did not.

    `ledger/2026-09-14-withdrawing-the-rename-caveat.md` withdrew the claim
    that only Python would be served here — the server enumerates every
    spelling the catalog accepts, so no client models renames at all — and
    added the test to Go and TypeScript. It left this:

        The Python client has no equivalent test. Its fingerprint is the
        reference the other two were ported from and is pinned against them,
        so the property follows -- but "follows" is not "shown".

    `docs.note` was once `comment`, declared in the testserver's schema. Both
    spellings are served; a third the column never had is refused. It is `note`
    rather than `kind` because
    `test_a_renamed_column_is_refused_rather_than_silently_answered` above
    needs `kind` spelled `category` to stay illegal.
    """
    from .fixture import DOCS as CURRENT

    previous = Table(
        "docs",
        [
            Column("id", ValueType.U64),
            Column("kind", ValueType.STR),
            Column("size", ValueType.I64),
            Column("comment", ValueType.STR),  # `note`, before it was renamed
        ],
        primary_key=["id"],
    )
    for name, table in (("previous", previous), ("current", CURRENT)):
        rows = list(client.query(Query(table).limit(1)))
        assert rows, f"the {name} spelling returned nothing to check"

    # The control, and the half that makes the pair mean something: a spelling
    # the column never had is refused, so the acceptance above is about the
    # recorded rename and not about the check having stopped running.
    never = Table(
        "docs",
        [
            Column("id", ValueType.U64),
            Column("kind", ValueType.STR),
            Column("size", ValueType.I64),
            Column("remark", ValueType.STR),
        ],
        primary_key=["id"],
    )
    with pytest.raises(InvalidRequest):
        list(client.query(Query(never).limit(1)))


def test_the_two_spellings_hash_differently(client: Client) -> None:
    """Which is why the server has to enumerate rather than compare one hash.

    The client sends one fingerprint. If the two spellings hashed the same the
    test above would pass for a reason that has nothing to do with renames —
    the server would be comparing a number that cannot tell them apart. They
    do not, so the acceptance is the enumeration working.
    """
    from .fixture import DOCS as CURRENT

    previous = Table(
        "docs",
        [
            Column("id", ValueType.U64),
            Column("kind", ValueType.STR),
            Column("size", ValueType.I64),
            Column("comment", ValueType.STR),
        ],
        primary_key=["id"],
    )
    assert fingerprint_of(previous) != fingerprint_of(CURRENT)
