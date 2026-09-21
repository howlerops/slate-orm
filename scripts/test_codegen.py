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


def test_a_generated_module_with_an_enum_is_valid_python() -> None:
    """The narrowed type has to survive being written into a *string*.

    This exists because it did not. The Python decoder casts to a quoted type
    — `cast("int | None", …)` — to avoid building a union object per row, and a
    `Literal["pending"]` closed that string early and made the whole generated
    module a `SyntaxError`.

    Every check in CI passed on it. `ruff` never saw the file, the codegen
    tests asserted on the *annotation* line rather than the cast, and the one
    test that executes generated Python used a catalog with no enumerated
    column. It was found by starting the demo, which is the only thing that
    had ever imported the result.

    So this compiles the module rather than reading it, and does so for a
    catalog that narrows — the combination none of the three checks had.
    """
    spec = [
        table(
            "posts",
            [column("id", "u64", 0), column("status", "string", 1), nullable("tag", "string", 2)],
            [0],
        )
    ]
    spec[0]["checks"] = [
        check("status_known", "status in ('draft', 'live')"),
        # A nullable enumerated column too: its hint is `Literal[…] | None`,
        # which is the composition of the two spellings and the place a naive
        # fix would put the quotes back.
        check("tag_known", "tag in ('red', 'blue')"),
    ]
    body = codegen.python_module(spec)

    # `compile`, not `ast.parse`: it is the check that actually failed, and a
    # module that parses but does not compile would still be broken.
    compile(body, "<generated>", "exec")

    try:
        import slate  # noqa: F401
    except ImportError:
        print("  (skipped executing it: the `slate` client is not installed)")
        return

    namespace: dict[str, object] = {}
    exec(compile(body, "<generated>", "exec"), namespace)
    row_type: Any = namespace["Posts"]
    decoded = row_type.from_row([1, "live", None])
    assert decoded.status == "live"
    assert decoded.tag is None

    # And the narrowing is a *type*, not a runtime check: the server enforces
    # the rule, so a value outside the set still decodes. Asserted because the
    # opposite is the natural assumption to make about a `Literal`, and a
    # caller who believed it would skip handling a value the server allows
    # after a schema change the client has not regenerated for.
    assert row_type.from_row([2, "archived", None]).status == "archived"


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


def check(name: str, predicate: str, column: str | None = None, message: str | None = None) -> dict:
    return {"name": name, "predicate": predicate, "column": column, "message": message}


def test_a_string_in_check_becomes_a_type_in_the_two_languages_that_have_one() -> None:
    """`status in ('draft', 'live')` is an enumeration, and enumerations are types.

    The note that asked for constraints to be published named exactly this as
    the payoff. The server already refuses anything outside the set, so the
    narrower type states the existing rule rather than inventing one.
    """
    spec = [
        table(
            "posts",
            [column("id", "u64", 0), column("status", "string", 1)],
            [0],
        )
    ]
    spec[0]["checks"] = [check("status_known", "status in ('draft', 'live', 'archived')")]

    body = codegen.python_module(spec)
    assert '    status: Literal["draft", "live", "archived"]' in body
    # And the import is there, which is not automatic: it is emitted only when
    # some table narrows, because an unused one is what ruff deletes out of the
    # generated file and turns into drift.
    assert "from typing import Literal, cast" in body
    assert '  status: "draft" | "live" | "archived";' in codegen.typescript_module(spec)

    # Go has no union of string literals, so the values are data beside the
    # struct and the field stays `string`.
    go = codegen.go_file(spec, "schema")
    assert 'var PostsStatusValues = []string{"draft", "live", "archived"}' in go, go
    assert "\tStatus string" in go, go


def test_a_predicate_the_matcher_does_not_fully_understand_narrows_nothing() -> None:
    """All-or-nothing on purpose.

    A general predicate parser here would be a second implementation of
    `lang/pred.rs`, which this repository has twice now paid to keep in step.
    Anything not recognised in full leaves the field alone, which is visible,
    rather than narrowing to a subset, which is silently wrong.
    """
    for predicate in (
        # A conjunction: the `in` is there but so is something else.
        "status in ('draft') and id > 0",
        # A column, not a literal.
        "status in ('draft', title)",
        # Not an `in` at all.
        "status = 'draft'",
        # Nested parentheses the narrow matcher refuses to guess at.
        "status in (('draft'))",
    ):
        spec = [
            table(
                "posts",
                [column("id", "u64", 0), column("status", "string", 1), column("title", "string", 2)],
                [0],
            )
        ]
        spec[0]["checks"] = [check("c", predicate)]
        assert "    status: str" in codegen.python_module(spec), predicate
        assert "Literal" not in codegen.python_module(spec).split("__all__")[1], predicate


def test_an_in_check_over_a_number_narrows_nothing() -> None:
    """`priority in (1, 2, 3)` is a range somebody wrote as a set.

    A union of numeric literals says less about arithmetic than `int` does, so
    the field keeps its plain type. Two spellings, because they fail the
    matcher at different places and only the second reaches the type guard:
    bare numbers are not string literals and are rejected by the value
    pattern, while *quoted* numbers parse fine and are refused only because
    the column is not a `str`.
    """
    for predicate in ("priority in (1, 2, 3)", "priority in ('1', '2', '3')"):
        spec = [table("posts", [column("id", "u64", 0), column("priority", "i64", 1)], [0])]
        spec[0]["checks"] = [check("priority_known", predicate)]
        body = codegen.python_module(spec)
        assert "    priority: int" in body, predicate
        assert "Literal[" not in body.split("__all__")[1], predicate
        assert "  priority: bigint;" in codegen.typescript_module(spec), predicate


def test_the_constraints_are_published_to_every_language() -> None:
    """Name, column, message and predicate, as data a client can read."""
    spec = [table("posts", [column("id", "u64", 0), column("title", "string", 1)], [0])]
    spec[0]["checks"] = [
        check("title_length", "title ~ '^.{1,80}$'", "title", "Title must be 1 to 80 characters.")
    ]

    body = codegen.python_module(spec)
    assert '"title_length": {' in body
    assert '"column": "title",' in body
    assert '"message": "Title must be 1 to 80 characters.",' in body

    go = codegen.go_file(spec, "schema")
    assert "var PostsChecks = map[string]slate.CheckRule{" in go
    assert 'Column: "title"' in go

    typescript = codegen.typescript_module(spec)
    assert "export const PostsChecks: Record<string, CheckRule>" in typescript
    assert 'message: "Title must be 1 to 80 characters."' in typescript


def test_a_check_with_no_column_publishes_the_absence() -> None:
    """Null, not an empty string, for the same reason the wire omits the key."""
    spec = [table("posts", [column("id", "u64", 0), column("title", "string", 1)], [0])]
    spec[0]["checks"] = [check("cross", "id > 0")]
    assert '"column": None,' in codegen.python_module(spec)
    assert "column: null" in codegen.typescript_module(spec)


def test_a_table_with_no_checks_emits_no_check_block() -> None:
    """An empty map in three languages is three pieces of noise."""
    spec = [table("posts", [column("id", "u64", 0)], [0])]
    body = codegen.python_module(spec)
    # The other half of the conditional import: nothing narrows here, so
    # `Literal` must be absent rather than imported and unused.
    assert "from typing import cast" in body
    assert "Literal" not in body
    assert "_CHECKS" not in body
    assert "PostsChecks" not in codegen.go_file(spec, "schema")
    assert "PostsChecks" not in codegen.typescript_module(spec)


def foreign_key(name: str, parent: int, columns: list[int]) -> dict:
    return {"name": name, "parent": parent, "columns": columns, "on_delete": "restrict"}


def two_tables_with_a_key() -> list[dict]:
    """`comments.post_id -> posts`, with the tables at ids 1 and 2.

    Two tables, because a foreign key is the one generated thing that needs a
    *second* table to be right about: its parent is published as an id and the
    generator has to resolve it.
    """
    posts = table("posts", [column("id", "u64", 0)], [0])
    comments = table(
        "comments", [column("id", "u64", 0), column("post_id", "u64", 1)], [0]
    )
    comments["id"] = 2
    comments["foreign_keys"] = [foreign_key("comment_post", 1, [1])]
    return [posts, comments]


def test_a_foreign_keys_parent_is_resolved_to_a_name_in_every_language() -> None:
    """The id in `--print-schema` becomes a table name in the generated file.

    This is the whole feature. A client holding a `Relation` knows the child
    and the key; the *parent* is what a `parents` read decodes as, and it is
    published as an id — which means nothing outside the catalog. A generated
    file carrying the id would leave the caller doing the look-up, and a
    caller doing it from memory is how one table's rows get decoded against
    another's ordinals.
    """
    spec = two_tables_with_a_key()
    go = codegen.go_file(spec, "schema")
    assert "CommentsForeignKeys" in go
    assert 'Parent: "posts"' in go, go[go.index("CommentsForeignKeys") :][:400]
    assert "Parent: 1" not in go, "the id leaked into the generated file"

    typescript = codegen.typescript_module(spec)
    assert 'parent: "posts"' in typescript
    assert "parent: 1" not in typescript

    python = codegen.python_module(spec)
    assert "COMMENTS_FOREIGN_KEYS" in python
    assert '"parent": "posts",' in python
    assert '"parent": 1,' not in python


def test_the_child_is_the_table_the_key_is_declared_on() -> None:
    """Not the parent, and not a guess from the column's name.

    `Relation.On` is always the child, either direction, and getting these two
    the wrong way round is a mistake that reads a plausible-looking table.
    """
    go = codegen.go_file(two_tables_with_a_key(), "schema")
    line = next(one for one in go.splitlines() if "comment_post" in one and "Child" in one)
    assert 'Child: "comments"' in line, line
    assert 'Parent: "posts"' in line, line


def test_a_composite_foreign_key_generates_like_any_other() -> None:
    """Two columns, one key, and nothing in the generated file about columns.

    Closes a hypothesis the foreign-key entry labelled: "Nothing in the
    generator looks at the column list, so there is no reason to expect
    trouble." That is testable in ten lines, and a labelled hypothesis in this
    repository is a thing to go and test rather than a thing to leave labelled.

    What it pins is the *shape* of the answer: a composite key generates one
    entry with a name, a child and a parent, exactly as a single-column key
    does, because the referencing columns are deliberately not generated —
    nothing in any client's surface takes them, since the server resolves the
    key by name.
    """
    posts = table("posts", [column("tenant", "u64", 0), column("id", "u64", 1)], [0, 1])
    comments = table(
        "comments",
        [
            column("id", "u64", 0),
            column("post_tenant", "u64", 1),
            column("post_id", "u64", 2),
        ],
        [0],
    )
    comments["id"] = 2
    comments["foreign_keys"] = [foreign_key("comment_post", 1, [1, 2])]
    spec = [posts, comments]

    go = codegen.go_file(spec, "schema")
    line = next(one for one in go.splitlines() if "comment_post" in one and "Child" in one)
    assert 'Child: "comments"' in line, line
    assert 'Parent: "posts"' in line, line
    # One entry, not one per column, and no ordinal anywhere in it.
    assert go.count('"comment_post":') == 1, go
    for spelled in ("1, 2", "[1 2]", "columns"):
        assert spelled not in line, f"the column list leaked into {line}"

    assert 'parent: "posts"' in codegen.typescript_module(spec)
    assert '"parent": "posts",' in codegen.python_module(spec)


def test_a_table_with_no_foreign_keys_emits_no_block() -> None:
    """An empty map in three languages is three pieces of noise."""
    spec = [table("posts", [column("id", "u64", 0)], [0])]
    assert "PostsForeignKeys" not in codegen.go_file(spec, "schema")
    assert "PostsForeignKeys" not in codegen.typescript_module(spec)
    assert "_FOREIGN_KEYS" not in codegen.python_module(spec)


def test_a_key_pointing_at_a_table_that_is_not_here_is_refused() -> None:
    """Rather than generated with a missing parent.

    Unreachable from a catalog the server would serve — it refuses the key at
    build time — so this is a guard against a bug in the generator or in
    `--print-schema`, not against anybody's schema. It matters because the
    quiet alternative is a generated file whose parent is empty, which reads a
    row of no table at all.
    """
    spec = two_tables_with_a_key()
    spec[1]["foreign_keys"][0]["parent"] = 99
    for produce in (
        lambda: codegen.go_file(spec, "schema"),
        lambda: codegen.typescript_module(spec),
        lambda: codegen.python_module(spec),
    ):
        try:
            produce()
        except SystemExit as why:
            assert "comment_post" in str(why), why
        else:
            raise AssertionError("a dangling parent was generated rather than refused")


def array_column(name: str, element: str, ordinal: int, *, null: bool = False) -> dict:
    """An array column of `element`, for the tests below."""
    made = column(name, "array", ordinal)
    made["element_type"] = element
    made["nullable"] = null
    return made


def test_an_array_column_declares_its_element_type_in_every_language() -> None:
    """The element type reaches the declaration, which is what the hash needs.

    The fingerprint hashes an array's element type, so a declaration carrying
    only `ARRAY` is refused by the server on the first request. This was the
    reason the generator used to refuse arrays outright: the type tables are
    keyed by the column's own type and cannot say what its elements are. The
    resolution is one more level of indirection, per column rather than per
    type, and the tables are untouched.
    """
    spec = [
        table(
            "posts",
            [column("id", "u64", 0), array_column("tags", "string", 1)],
            [0],
        )
    ]
    python = codegen.python_module(spec)
    assert 'Column("tags", ValueType.ARRAY, element=ValueType.STR)' in python, python

    go = codegen.go_file(spec, "schema")
    assert '{Name: "tags", Type: slate.TypeArray, Element: slate.TypeString}' in go, go

    typescript = codegen.typescript_module(spec)
    assert '{ name: "tags", type: "array", element: "string" }' in typescript, typescript


def test_the_go_and_typescript_decoders_check_an_array_element_by_element() -> None:
    """The element check reaches Go and TypeScript, not only Python.

    Asserted on the emitted source rather than by running it, and that is the
    weakness worth naming: the Python case above is executed, these two are
    read. The suites that *execute* generated Go and TypeScript are the demo's
    own, and the demo's schema has no array column, so nothing runs this yet.
    Mutation testing is what made the gap visible — removing the element check
    from both emitters broke nothing until these assertions existed.
    """
    spec = [
        table(
            "posts",
            [column("id", "u64", 0), array_column("tags", "string", 1)],
            [0],
        )
    ]

    go = codegen.go_file(spec, "schema")
    assert "Tags []string" in go.replace("  ", " "), go
    # The element is asserted to its own wire type, not to `slate.Value`: a
    # `slate.Array` is a `[]slate.Value`, so asserting the interface always
    # succeeds and the cast that follows would be wrong for every element.
    assert "ev, ok := e.(slate.String)" in go, go
    # And the error names the position, because `tags[2]` is findable.
    assert 'posts.tags[%d]: expected slate.String' in go, go
    # The write side wraps each element in its own type.
    assert "append(tagsElements, slate.String(e))" in go, go

    typescript = codegen.typescript_module(spec)
    assert "tags: string[]" in typescript, typescript
    assert 'elements(row, 1, "posts", "tags", "string", false) as string[]' in typescript, (
        typescript
    )
    assert 'row.tags.map((e) => ({ kind: "string", value: e }))' in typescript, typescript


def test_a_generated_array_column_decodes_and_encodes_element_by_element() -> None:
    """The generated Python runs, and the elements survive the round trip.

    Executed rather than pattern-matched, for the reason the transposition
    test above gives. The element check is the part worth running: an array's
    elements arrive already decoded to native Python, so a wrong element type
    in the declaration produces a list that *looks* right at the call site and
    is refused by the server instead.
    """
    spec = [
        table(
            "posts",
            [
                column("id", "u64", 0),
                array_column("tags", "string", 1),
                array_column("sizes", "i64", 2),
            ],
            [0],
        )
    ]
    body = codegen.python_module(spec)

    try:
        from slate import i64
        from slate.values import Array
    except ImportError:
        print("  (skipped: the `slate` client is not installed)")
        return

    namespace: dict[str, object] = {}
    exec(compile(body, "<generated>", "exec"), namespace)
    row_type: Any = namespace["Posts"]

    decoded = row_type.from_row([1, Array(("a", "b")), Array((i64(2), i64(3)))])
    assert decoded.tags == ("a", "b"), decoded.tags
    assert decoded.sizes == (2, 3), decoded.sizes
    # Empty is a value, not a null — the distinction `docs/arrays.md` makes
    # load-bearing in the kernel, and the one a generated decoder could lose.
    assert row_type.from_row([1, Array(()), Array(())]).tags == ()

    # The encoder wraps each element in its own tag. `i64` and `u64` are both
    # a plain `int` after decoding, so a list of them carries nothing to tell
    # them apart and the server would be the first to notice.
    encoded = row_type(id=1, tags=("a",), sizes=(7,)).to_row()
    assert isinstance(encoded[1], Array), encoded
    assert isinstance(encoded[2][0], i64), f"the element lost its tag: {encoded[2]!r}"

    # And an element of the wrong type is named with its position.
    try:
        row_type.from_row([1, Array((1,)), Array((i64(2),))])
    except TypeError as why:
        assert "posts.tags[0]" in str(why), why
    else:
        raise AssertionError("a wrongly typed element decoded without complaint")


def test_an_array_of_arrays_and_an_array_with_no_element_type_are_refused() -> None:
    """Two shapes no catalog can hold, refused rather than half-generated.

    Neither reaches here through a catalog `slate-serverd` printed — the schema
    layer refuses an array column with no element type at build time, and there
    is no array-of-arrays column at all. They are checked anyway for the reason
    the unreachable refusals elsewhere in this repository are: the guarantee is
    one edit away from weakening, and the failure then would be generated code
    that compiles and decodes every element as the wrong type.
    """
    missing = column("tags", "array", 1)
    missing.pop("element_type", None)
    for bad, expected in [
        (missing, "element type"),
        (array_column("tags", "array", 1), "array of arrays"),
    ]:
        spec = [table("posts", [column("id", "u64", 0), bad], [0])]
        try:
            codegen.python_module(spec)
        except codegen.Unknown as why:
            assert "tags" in str(why), why
            assert expected in str(why), why
        else:
            raise AssertionError(f"`{expected}` was generated rather than refused")


def test_refuse_unsupported_fires_and_does_not_over_fire() -> None:
    """`UNSUPPORTED` is empty, and the mechanism is still exercised.

    Every type has a generator now, so the dict has nothing in it — and an
    empty roster means the code that reads it never runs, which is the "a
    check that never fires is a check nobody has debugged" shape this
    repository keeps meeting. So the test puts an entry in, checks the refusal
    names the table, the column and the reason, and takes it out again.

    The mechanism is kept rather than deleted because the next type that
    cannot be generated should be one line and a sentence, not a rediscovery
    of why the declaration emitters and the row emitters used to fail
    differently on the same catalog.
    """
    spec = [table("posts", [column("id", "u64", 0), column("when", "instant", 1)], [0])]
    codegen.UNSUPPORTED["instant"] = "there is no such type; this entry is a test fixture"
    try:
        codegen.refuse_unsupported(spec)
    except codegen.Unknown as why:
        assert "posts" in str(why), why
        assert "when" in str(why), why
        assert "test fixture" in str(why), why
    else:
        raise AssertionError("a listed type was accepted")
    finally:
        del codegen.UNSUPPORTED["instant"]


def test_an_ordinary_catalog_is_not_refused() -> None:
    """The never-fires half.

    A guard that refused everything would pass the case above and break every
    real catalog, so the negative is asserted too — the same reason the
    handler checks in this repository carry one.
    """
    codegen.refuse_unsupported(
        [
            {
                "name": "docs",
                "columns": [
                    {"name": "id", "type": "u64", "nullable": False, "scale": None},
                    {"name": "amount", "type": "decimal", "nullable": False, "scale": 2},
                ],
                "primary_key": ["id"],
            }
        ]
    )


def main() -> int:
    passed = 0
    failed = 0
    for name, test in sorted(globals().items()):
        if not name.startswith("test_") or not callable(test):
            continue
        try:
            test()
        # `Exception`, not `AssertionError`. This file is the only `test_*.py`
        # here that calls functions directly rather than running a subprocess,
        # so it is the only one where a test raising something *unexpected*
        # took the whole runner down — no summary line, no `FAIL`, and
        # `scripts/mutate.py` reporting "no test results at all" rather than
        # scoring the mutation that caused it. An aborted runner reads exactly
        # like a clean one, which is the shape of every lie this repository
        # keeps finding. A test that raises for a reason it did not predict is
        # a failing test, and is reported as one.
        except Exception as why:  # noqa: BLE001
            failed += 1
            print(f"FAIL  {name}: {type(why).__name__}: {why}")
        else:
            passed += 1
            print(f"ok    {name}")
    # `N passed, M failed`, which is the house style every other `test_*.py`
    # here prints and `scripts/mutate.py`'s `python` dialect reads. This file
    # printed `all pass` instead, which matched neither — so pointing the
    # mutation harness at this generator reported "no test results at all"
    # rather than scoring anything. That is the harness refusing to lie, and
    # the fix is the summary line rather than a fourth dialect.
    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
