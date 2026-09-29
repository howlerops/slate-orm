#!/usr/bin/env python3
"""Tests for `check_renamed_column.py`, over a tree this one builds.

Run against the real clients, a rule that had stopped checking would pass for
as long as all three stayed correct — which is the failure mode the guard
exists to prevent, one level up. The case that matters most is the fourth
client: the guard's whole claim is that it sees one arriving.

The fixture is *derived from the roster* rather than written out beside it, so
a rule whose pattern stops matching its own declaration is a failure here
rather than a fixture to update. That costs an unescaping step — the patterns
are regexes over source, so `\\.` and `\\[` do not match themselves — and buys
the property that adding a fourth client to `PROVES` needs no edit here.

Run directly: `python3 scripts/test_check_renamed_column.py`.
"""

from __future__ import annotations

import contextlib
import dataclasses
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_renamed_column as guard

#: Which manifest makes each rostered client a client. Not derived: `MANIFESTS`
#: is a flat tuple with no idea which language owns which name, and a fixture
#: that wrote all four into every directory would stop the no-manifest case
#: from being reachable at all.
MANIFEST = {
    "python": "pyproject.toml",
    "go": "go.mod",
    "typescript": "package.json",
}


def literal(pattern: str) -> str:
    """A line the guard's pattern matches, from the pattern itself."""
    for escaped, plain in (
        ("\\(", "("),
        ("\\)", ")"),
        ("\\{", "{"),
        ("\\}", "}"),
        ("\\[", "["),
        ("\\]", "]"),
        ("\\.", "."),
    ):
        pattern = pattern.replace(escaped, plain)
    return pattern


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as handle:
        handle.write(text)


def tree(root: pathlib.Path) -> None:
    """A miniature of the real layout: three clients, three proofs."""
    for name, proof in guard.PROVES.items():
        (root / "clients" / name).mkdir(parents=True, exist_ok=True)
        (root / "clients" / name / MANIFEST[name]).write_text("")
        write(root / proof.where, literal(proof.test) + "\n")
        write(root / proof.where, literal(proof.declares) + "\n")
        write(root / proof.renamed[0], literal(proof.renamed[1]) + "\n")


def run(root: pathlib.Path) -> tuple[int, str]:
    guard.ROOT, guard.CLIENTS = root, root / "clients"
    out, err = io.StringIO(), io.StringIO()
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = guard.main()
    except Exception as raised:  # noqa: BLE001 - any raise is a failing case
        return 70, f"{out.getvalue()}{err.getvalue()}the guard raised {raised!r}"
    return code, out.getvalue() + err.getvalue()


def clean(root: pathlib.Path) -> None:
    tree(root)


def fourth_client(root: pathlib.Path) -> None:
    """A Rust client arrives with a manifest and no test. The headline case."""
    tree(root)
    (root / "clients" / "rust").mkdir()
    (root / "clients" / "rust" / "Cargo.toml").write_text("")


def no_manifest(root: pathlib.Path) -> None:
    """A directory the discovery cannot classify is a person's problem."""
    tree(root)
    (root / "clients" / "kotlin").mkdir()


def test_file_gone(root: pathlib.Path) -> None:
    """The file holding the test is renamed away, not edited.

    Separate from `test_renamed` because a guard that reports only "no match"
    sends you looking inside a file that is not there, and because nothing
    else here distinguishes the two arms of `missing`.
    """
    tree(root)
    (root / guard.PROVES["go"].where).unlink()


def test_renamed(root: pathlib.Path) -> None:
    proof = guard.PROVES["typescript"]
    tree(root)
    path = root / proof.where
    path.write_text(path.read_text().replace(literal(proof.test), 'test("something else"'))


def test_occurs_twice(root: pathlib.Path) -> None:
    """Two matches and the roster cannot say which test it means.

    Not pedantry: the declaration is looked for in the *body* below, and a
    body is a slice between one test and the next. With two candidates there
    is no such slice, and picking the first would make the rule's answer
    depend on file order.
    """
    proof = guard.PROVES["go"]
    tree(root)
    write(root / proof.where, literal(proof.test) + "\n")


#: How each language starts the *next* test, for the case below. A fourth
#: client needs a line here; the case list is generated from `PROVES`, so one
#: arriving without an entry raises rather than quietly generating no case.
NEXT_TEST = {
    "python": "def test_about_hashing() -> None:",
    "go": "func TestSomethingElse(t *testing.T) {",
    "typescript": 'test("something else", async () => {',
}


def declared_only_in_another_test(client: str):
    """The hole a real-tree mutation found: the declaration is *somewhere*.

    `clients/python/tests/test_fixture.py` declares `Column("comment", …)`
    twice — once in the renamed-column test and once in
    `test_the_two_spellings_hash_differently`, which is about hashing — and
    `clients/typescript/test/schema.test.ts` declares `category` twice, for
    two different tables. So the guard's first draft, which searched the whole
    file, stayed green with the real declaration cut out. This is that shape:
    the test is there, the declaration is there, and they are not the same
    test.

    One case per client rather than one overall, because the three languages
    end a test differently and `BOUNDARY` has a keyword for each. A single
    Python case left `test\\(` unexercised, and the mutation dropping it
    survived.
    """

    def build(root: pathlib.Path) -> None:
        tree(root)
        proof = guard.PROVES[client]
        path = root / proof.where
        path.write_text(path.read_text().replace(literal(proof.declares) + "\n", ""))
        write(path, NEXT_TEST[client] + "\n    " + literal(proof.declares) + "\n")

    return build


def an_indented_declaration_inside_a_body(root: pathlib.Path) -> None:
    """A helper defined inside a test must not end that test's body.

    The one case here that must *pass*, and the only one that pins `BOUNDARY`'s
    `^`: without it the first indented `def ` truncates the body, the
    declaration below falls outside it, and the guard refuses a client that is
    perfectly correct. A nested `def` inside a Python test is ordinary, so this
    is a false refusal waiting rather than a hypothetical.
    """
    tree(root)
    proof = guard.PROVES["python"]
    path = root / proof.where
    path.write_text(
        path.read_text().replace(
            literal(proof.declares),
            "    def helper() -> None:\n        pass\n" + literal(proof.declares),
        )
    )


def declared_by_the_tests_own_name(root: pathlib.Path) -> None:
    """A `declares` pattern the test's own declaration satisfies checks nothing.

    Here because the mutation moving `body`'s start back to the declaration
    survived every other case in this file: no pattern in the real roster
    overlaps a test declaration, so both slices behaved identically and the
    choice between them was unobservable. It is observable now. A roster entry
    that lazily points `declares` at part of the test's name — `RenamedColumn`
    is in `TestARenamedColumnIsAcceptedUnderItsPreviousName` — must not be
    satisfied by the name it was written next to.
    """
    tree(root)
    proof = guard.PROVES["go"]
    path = root / proof.where
    path.write_text(path.read_text().replace(literal(proof.declares) + "\n", ""))
    guard.PROVES = dict(guard.PROVES, go=dataclasses.replace(proof, declares=r"RenamedColumn"))


def declares_nothing_old(root: pathlib.Path) -> None:
    """The test is there and hands the server the *current* spelling.

    The 2026-09-14 claim this guard descends from was exactly this mistake in
    prose: a client recorded as exercising renames while exercising nothing.
    """
    proof = guard.PROVES["python"]
    tree(root)
    path = root / proof.where
    path.write_text(path.read_text().replace(literal(proof.declares), 'Column("note")'))


def the_rename_is_gone(root: pathlib.Path) -> None:
    """The catalog stops calling the name an old one, and the test still passes.

    A spelling that is not a previous name is just the name, so the client
    declares the current column, the server serves it, and a green test named
    for renames proves nothing. Nothing else in this file reaches that rule.
    """
    proof = guard.PROVES["go"]
    tree(root)
    path = root / proof.renamed[0]
    path.write_text(path.read_text().replace(literal(proof.renamed[1]), 'name = "kind"'))


def rostered_client_gone(root: pathlib.Path) -> None:
    tree(root)
    (root / "clients" / "go" / "go.mod").unlink()


def no_clients_at_all(root: pathlib.Path) -> None:
    """The never-fires case: nothing found must not read as nothing wrong."""
    (root / "clients").mkdir()


CASES = [
    (
        "three clients with a renamed-column test pass",
        clean,
        0,
        "3 clients, each with a test that declares",
    ),
    (
        "a fourth client with no test fails and names it",
        fourth_client,
        1,
        "clients/rust is a client and PROVES has no entry",
    ),
    (
        "a directory with no manifest fails rather than being skipped",
        no_manifest,
        1,
        "declares none of",
    ),
    (
        "a renamed-away test file fails and says the file is gone",
        test_file_gone,
        1,
        "clients/go/slate/schema_test.go does not exist",
    ),
    (
        "a renamed test fails",
        test_renamed,
        1,
        "typescript's renamed-column test occurs 0 times",
    ),
    (
        "a test pattern matching twice fails",
        test_occurs_twice,
        1,
        "go's renamed-column test occurs 2 times",
    ),
    *(
        (
            f"a declaration only in some other {client} test fails",
            declared_only_in_another_test(client),
            1,
            f"no `{guard.PROVES[client].declares}` between that test and the next one",
        )
        for client in sorted(guard.PROVES)
    ),
    (
        "a helper defined inside a test does not end its body",
        an_indented_declaration_inside_a_body,
        0,
        "3 clients, each with a test that declares",
    ),
    (
        "a declaration matched by the test's own name fails",
        declared_by_the_tests_own_name,
        1,
        "no `RenamedColumn` between that test and the next one",
    ),
    (
        "a test declaring only the current spelling fails",
        declares_nothing_old,
        1,
        "python's renamed-column test no longer declares an old spelling",
    ),
    (
        "a catalog that no longer renames the column fails",
        the_rename_is_gone,
        1,
        "declares a name the catalog no longer calls an old one",
    ),
    (
        "a rostered client that is gone fails",
        rostered_client_gone,
        1,
        "is not a client directory any more",
    ),
    ("no clients at all fails", no_clients_at_all, 1, "no client directories found"),
]


def main() -> int:
    was = (guard.ROOT, guard.CLIENTS, guard.PROVES)
    failures = []
    try:
        for name, build, wanted, needle in CASES:
            # Per case, not once at the end: a case may patch `PROVES`, and a
            # patch that leaked into the next case would make this file's
            # results depend on its own order.
            guard.ROOT, guard.CLIENTS, guard.PROVES = was
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                try:
                    build(root)
                except Exception as raised:  # noqa: BLE001 - so is a bad fixture
                    code, output = 70, f"the fixture raised {raised!r}"
                else:
                    code, output = run(root)
            if code != wanted:
                failures.append(f"FAIL  {name}: exit {code}, wanted {wanted}\n{output}")
            elif needle not in output:
                failures.append(f"FAIL  {name}: no {needle!r} in\n{output}")
            else:
                print(f"ok    {name}")
    finally:
        guard.ROOT, guard.CLIENTS, guard.PROVES = was

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
