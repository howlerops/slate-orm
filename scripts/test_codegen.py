#!/usr/bin/env python3
"""What `codegen.py` does that no catalog in this repository exercises.

The generator's real test is the three-SDK conformance suite: the declarations
it writes are what every adapter sends, so a wrong one refuses ninety-two cases
and the run says so. What that cannot reach is the code paths no *existing*
catalog goes down — a dropped column, an ordinal gap, a type nobody has added a
spelling for — because the demo's schema has none of them. Those are here, on
synthetic catalogs.

Run directly (`python3 scripts/test_codegen.py`) or under pytest.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

import codegen


def column(name: str, kind: str, ordinal: int, scale: int | None = None) -> dict:
    return {
        "name": name,
        "type": kind,
        "ordinal": ordinal,
        "scale": scale,
        "nullable": False,
        "added_in": 0,
        "dropped_in": None,
        "previous_names": [],
    }


def table(name: str, columns: list[dict], key: list[int]) -> dict:
    return {
        "name": name,
        "id": 1,
        "schema_version": 0,
        "primary_key": key,
        "tenant_column": None,
        "columns": columns,
        "indexes": [],
    }


def test_a_dropped_column_keeps_its_ordinal() -> None:
    """A dropped column is declared, not skipped.

    This is the failure the whole generator exists to prevent, arriving by the
    one route a generator could introduce it: `dropped_in` looks like a reason
    to leave a column out, and leaving it out shifts every ordinal after it by
    one. The server would accept the resulting requests — the references are
    legitimate — and answer every one of them with the wrong column.
    """
    gone = column("removed", "string", 1)
    gone["dropped_in"] = 3
    one = table("t", [column("id", "u64", 0), gone, column("kept", "i64", 2)], [0])
    body = codegen.python_module([one])
    assert '"removed"' in body, body
    assert body.index('"removed"') < body.index('"kept"'), body


def test_an_ordinal_gap_is_refused_rather_than_papered_over() -> None:
    """A catalog that cannot be right stops the run.

    Unreachable through `--print-schema` today, which enumerates columns in
    order. Asserted because the alternative when it stops being unreachable is
    a file that generates cleanly and is wrong, and a generator that silently
    renumbers is worse than one that does not run.
    """
    gappy = table("t", [column("id", "u64", 0), column("far", "i64", 4)], [0])
    try:
        codegen.python_module([gappy])
    except SystemExit as stopped:
        assert "gap at ordinal 1" in str(stopped), stopped
    else:
        raise AssertionError("a gap at ordinal 1 should have stopped the run")


def test_an_unknown_type_is_refused_in_every_language() -> None:
    """A type with no spelling is an error, never a guess.

    A guess would compile and hash to the wrong fingerprint, which is exactly
    the silent failure being generated away from — so this is the one place a
    crash is the good outcome.
    """
    exotic = table("t", [column("id", "u64", 0), column("odd", "geography", 1)], [0])
    for produce in (
        lambda: codegen.python_module([exotic]),
        lambda: codegen.go_file([exotic], "schema"),
        lambda: codegen.typescript_module([exotic]),
    ):
        try:
            produce()
        except codegen.Unknown as refused:
            assert "geography" in str(refused), refused
        else:
            raise AssertionError("an unknown type should have been refused")


def test_a_decimal_carries_its_scale_and_nothing_else_does() -> None:
    """The scale reaches the declaration, and only for a decimal.

    `scale=0` on a non-decimal is not harmless noise: the Python client refuses
    it outright, and the fingerprint hashes a decimal's scale, so emitting one
    where the server has none is a declaration the server will reject.
    """
    money = table(
        "t",
        [column("id", "u64", 0), column("price", "decimal", 1, scale=2)],
        [0],
    )
    python = codegen.python_module([money])
    assert 'Column("price", ValueType.DECIMAL, scale=2)' in python, python
    assert "scale=" not in python.split('Column("id"')[1].split("\n")[0], python

    go = codegen.go_file([money], "schema")
    assert "Scale: 2" in go, go
    assert go.count("Scale:") == 1, go

    typescript = codegen.typescript_module([money])
    assert "scale: 2" in typescript, typescript
    assert typescript.count("scale:") == 1, typescript


def test_the_primary_key_is_named_not_numbered() -> None:
    """A composite key comes out as names, in key order.

    Key order is not column order — `primary_key = ["b", "a"]` is a different
    key from `["a", "b"]` and hashes differently — so this checks the order
    survives rather than only the membership.
    """
    pair = table(
        "t",
        [column("a", "u64", 0), column("b", "u64", 1), column("c", "i64", 2)],
        [1, 0],
    )
    assert 'primary_key=["b", "a"]' in codegen.python_module([pair])
    assert 'PrimaryKey: []string{"b", "a"}' in codegen.go_file([pair], "schema")
    assert 'primaryKey: ["b", "a"]' in codegen.typescript_module([pair])


def nullable(name: str, kind: str, ordinal: int) -> dict:
    field = column(name, kind, ordinal)
    field["nullable"] = True
    return field


def test_a_row_type_skips_a_dropped_column_but_not_its_ordinal() -> None:
    """The declaration keeps a dropped column; the row type drops the field.

    These pull in opposite directions and both are right. The declaration must
    keep it because the fingerprint hashes which columns are dropped. The row
    type must not, because a field nobody can read is noise — but the columns
    after it must still decode from their *real* ordinals, which is the bug
    this asserts against.
    """
    gone = column("old", "string", 1)
    gone["dropped_in"] = 2
    spec = [table("t", [column("id", "u64", 0), gone, column("name", "string", 2)], [0])]

    body = codegen.python_module(spec)
    assert "    old:" not in body, "a dropped column is not a field"
    assert '"t", "name"' in body
    # The point: `name` decodes from ordinal 2, not from 1.
    assert 'values, 2, "t", "name"' in body, body

    go = codegen.go_file(spec, "schema")
    assert "\tOld " not in go
    assert "row[2].(slate.String)" in go, go

    typescript = codegen.typescript_module(spec)
    assert "  old:" not in typescript
    assert 'field(row, 2, "t", "name"' in typescript, typescript


def test_a_nullable_column_is_optional_in_every_language() -> None:
    """Three languages, three spellings of absent, none of them a zero."""
    spec = [table("t", [column("id", "u64", 0), nullable("note", "string", 1)], [0])]

    assert "    note: str | None" in codegen.python_module(spec)
    # A pointer, because Go's zero string is a real value a caller cannot tell
    # from an absent one.
    assert "\tNote *string" in codegen.go_file(spec, "schema")
    assert "  note: string | null;" in codegen.typescript_module(spec)


def test_a_table_name_is_not_de_pluralised() -> None:
    """`books` becomes `Books`. Guessing the singular is guessing about English."""
    spec = [table("series", [column("id", "u64", 0)], [0])]
    assert "class Series:" in codegen.python_module(spec)
    assert "type Series struct" in codegen.go_file(spec, "schema")
    assert "export interface Series {" in codegen.typescript_module(spec)


def test_a_generated_python_row_decodes_and_refuses_a_transposed_one() -> None:
    """The generated Python actually runs, and catches the failure it exists for.

    Two columns of different types, transposed. Without the per-column check
    this returns a `Books` holding a title where the id goes and an id where
    the title goes — which is exactly the silent, plausible wrongness the whole
    generator is against, and it is invisible at the call site because both
    fields are populated.
    """
    spec = [
        table(
            "t",
            [column("id", "u64", 0), column("title", "string", 1), nullable("note", "string", 2)],
            [0],
        )
    ]
    body = codegen.python_module(spec)

    # Executed, not pattern-matched. The generated module imports from `slate`,
    # so this runs only where the client is installed — skipped rather than
    # failed elsewhere, because the generator is testable without it and CI
    # installs it.
    try:
        import slate  # noqa: F401
    except ImportError:
        print("  (skipped: the `slate` client is not installed)")
        return

    namespace: dict[str, object] = {}
    exec(compile(body, "<generated>", "exec"), namespace)
    # `Any`, because the type is built at run time by the generator under
    # test; a checker cannot know it and the alternative is four ignores.
    row_type: Any = namespace["T"]

    decoded = row_type.from_row([1, "a title", None])
    assert decoded.id == 1
    assert decoded.title == "a title"
    assert decoded.note is None, "a null nullable column decodes to None"

    transposed = ["a title", 1, None]
    try:
        row_type.from_row(transposed)
    except TypeError as why:
        assert "t.id" in str(why), why
    else:
        raise AssertionError("a transposed row decoded without complaint")

    try:
        row_type.from_row([1, "a title"])
    except ValueError as why:
        assert "3 columns" in str(why), why
    else:
        raise AssertionError("a short row decoded without complaint")


def test_a_non_nullable_column_that_comes_back_null_is_refused() -> None:
    """Null into a column declared not-null is a server or schema bug, not a None."""
    spec = [table("t", [column("id", "u64", 0)], [0])]
    body = codegen.python_module(spec)
    try:
        import slate  # noqa: F401
    except ImportError:
        print("  (skipped: the `slate` client is not installed)")
        return
    namespace: dict[str, object] = {}
    exec(compile(body, "<generated>", "exec"), namespace)
    row_type: Any = namespace["T"]
    try:
        row_type.from_row([None])
    except ValueError as why:
        assert "not nullable" in str(why), why
    else:
        raise AssertionError("a null in a non-nullable column decoded without complaint")


def main() -> int:
    failed = 0
    for name, test in sorted(globals().items()):
        if not name.startswith("test_") or not callable(test):
            continue
        try:
            test()
        except AssertionError as why:
            failed += 1
            print(f"FAIL  {name}: {why}")
        else:
            print(f"ok    {name}")
    print(f"\n{'all pass' if not failed else f'{failed} failed'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
