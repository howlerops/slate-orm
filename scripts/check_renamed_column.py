#!/usr/bin/env python3
"""Every client proves it accepts a renamed column's previous name.

`ledger/2026-09-14-schema-checks-in-go-and-typescript.md` claimed the opposite:

> Neither client accepts a renamed column's previous spelling, which the server
> does accept … where the Python client would be served.

It was wrong, and wrong in the direction that matters — a client was recorded
as *lacking* a property it had. A client declares the spelling **it** uses; the
server enumerates every spelling the catalog accepts (`fingerprint::accepted`,
a product over each column's renames) and compares. So the old name passes, the
new name passes, and no client models renames at all.

Go and TypeScript withdrew that claim with a test each on 2026-09-28. Python
had no such test until 2026-09-29, and the entry that added it left this:

> Nothing stops the fourth client arriving without this test.

Hence a roster, and hence a roster **derived from `clients/`** rather than
written out: a fourth client is caught by existing, not by being remembered.
The same shape as `check_transport_door.py`, for the same reason — that guard
exists because the TypeScript client shipped for months with no way to pass an
interceptor and three entries said so before anybody checked the other two.

What each client must have is one test that declares a table under a column's
*previous* name and is served. The needle is the test, not the feature: the
feature is the server's, and a client passes it by not getting in the way,
which is exactly the kind of property that is lost silently.

Run directly: `python3 scripts/check_renamed_column.py`.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CLIENTS = ROOT / "clients"

#: A directory under `clients/` is a client when it declares a package. Kept
#: identical to `check_transport_door.py`'s list on purpose: two roster guards
#: over the same directory that disagree about what a client is would let a
#: fourth SDK be caught by one and skipped by the other, and the one that
#: skipped it would stay green. `Cargo.toml` is in the list for the Rust client
#: that does not exist yet; `clients/python/testserver` carries one too, but it
#: is nested and only the top level is read.
MANIFESTS = ("pyproject.toml", "go.mod", "package.json", "Cargo.toml")


@dataclass(frozen=True)
class Proof:
    """Where a client shows it serves a renamed column, and how to see it."""

    #: The file holding the test.
    where: str
    #: A pattern naming the test itself, so renaming the test fails here rather
    #: than quietly leaving the roster pointing at nothing.
    test: str
    #: A pattern proving the test hands the server a table declaring the column
    #: under the *old* name. Without this the roster would accept a test that
    #: merely mentions renaming — which is what the wrong claim in the
    #: 2026-09-14 entry was, in prose. Looked for inside `test`'s body and
    #: nowhere else: two of these three files declare the same old spelling a
    #: second time, in a test about something else, and a file-wide search
    #: stayed green with the real declaration cut out.
    declares: str
    #: Where the catalog says that name is an old one, as (file, pattern).
    #: Separate from `declares` because the two can rot apart, and when they do
    #: the test keeps passing for the wrong reason: drop the rename from the
    #: catalog and `category` is simply the column's name, so the client
    #: declares the current spelling, the server serves it, and a test named
    #: for renames is exercising nothing. It is not the same file for every
    #: client — Go and TypeScript write their catalog inline in the test, and
    #: Python's is the testserver's, shared with the rest of that suite.
    renamed: tuple[str, str]


PROVES: dict[str, Proof] = {
    "go": Proof(
        where="clients/go/slate/schema_test.go",
        test=r"func TestARenamedColumnIsAcceptedUnderItsPreviousName\(",
        declares=r'\{Name: "category", Type: slate\.TypeString\}',
        renamed=(
            "clients/go/slate/schema_test.go",
            r'name = "kind", type = "str", previous_names = \["category"\]',
        ),
    ),
    "python": Proof(
        where="clients/python/tests/test_fixture.py",
        test=r"def test_a_renamed_column_is_accepted_under_its_previous_name\(",
        declares=r'Column\("comment", ValueType\.STR\)',
        renamed=(
            "clients/python/testserver/src/main.rs",
            r'\.renamed_column\("note", "comment"\)',
        ),
    ),
    "typescript": Proof(
        where="clients/typescript/test/schema.test.ts",
        test=r'test\("a renamed column is accepted under its previous name"',
        declares=r'\{ name: "category", type: "string" \}',
        renamed=(
            "clients/typescript/test/schema.test.ts",
            r'name = "kind", type = "str", previous_names = \["category"\]',
        ),
    ),
}


def discovered() -> tuple[list[str], list[str]]:
    """Client directories under `clients/`, and any that declare no package."""
    named, unmanifested = [], []
    for child in sorted(CLIENTS.iterdir()) if CLIENTS.is_dir() else []:
        if not child.is_dir() or child.name.startswith("."):
            continue
        (named if any((child / m).exists() for m in MANIFESTS) else unmanifested).append(child.name)
    return named, unmanifested


#: Where one test ends and the next begins. A heuristic and named as one: the
#: union of the three languages' declaration keywords at column zero, not a
#: parser. It is enough because the roster refuses a `test` pattern that does
#: not occur exactly once, so the slice always starts at a known test.
BOUNDARY = re.compile(r"^(?:async def |def |func |test\(|it\(|describe\()", re.M)


def body(text: str, test: str) -> str:
    """What `test` matched up to the start of the next test, excluding itself.

    The slice starts *after* the declaration rather than at it so that a
    `declares` pattern cannot be satisfied by the test's own name. Nothing in
    today's roster overlaps a declaration, so both boundaries behave
    identically against the real tree — which is exactly why the choice is
    written down: between two answers a run cannot tell apart, keep the safer
    one, and put the case that tells them apart in the test file.
    """
    at = re.search(test, text)
    if at is None:  # pragma: no cover - callers count the matches first
        raise AssertionError(f"body() called with no match for {test!r}")
    following = BOUNDARY.search(text, at.end())
    return text[at.end() : following.start() if following else len(text)]


def missing(relative: str, pattern: str) -> str | None:
    """`None` when `pattern` occurs in the file, else why it does not.

    The two answers are kept apart because a renamed-away file and an edited
    one are different mistakes with the same symptom, and a guard that says
    only "no match" sends you looking in a file that is not there.
    """
    path = ROOT / relative
    if not path.exists():
        return f"{relative} does not exist"
    if re.search(pattern, path.read_text()):
        return None
    return f"{relative} has no `{pattern}`"


def main() -> int:
    problems: list[str] = []
    found, unmanifested = discovered()

    if not found:
        # The never-fires case. `clients/` renamed or emptied and every rule
        # below checks nothing, which reads exactly like a clean run.
        problems.append(
            f"no client directories found under {CLIENTS}. Either the layout "
            "moved or MANIFESTS is stale — both need a person, not a pass."
        )
    for name in unmanifested:
        problems.append(
            f"clients/{name} declares none of {list(MANIFESTS)}, so this rule "
            "cannot tell whether it is a client. Add its manifest name to "
            "MANIFESTS, or say here why the directory is not a client."
        )
    for name in sorted(set(found) - set(PROVES)):
        problems.append(
            f"clients/{name} is a client and PROVES has no entry for it.\n"
            "  A client that never declares a renamed column's previous name "
            "will pass every other test and lose the property silently: the "
            "server accepts the old spelling and the client has to do nothing "
            "at all, so nothing breaks until something does. Two entries "
            "recorded Go and TypeScript as *lacking* this before anybody ran "
            "it. Write the test, then name it here."
        )
    for name in sorted(set(PROVES) - set(found)):
        problems.append(
            f"PROVES has an entry for {name} and clients/{name} is not a "
            "client directory any more. Fix the entry, or this rule is "
            "checking an SDK that is gone."
        )

    for name in sorted(set(PROVES) & set(found)):
        proof = PROVES[name]
        path = ROOT / proof.where
        if not path.exists():
            problems.append(
                f"{name} has no renamed-column test: {proof.where} does not "
                "exist.\n"
                "  The file was renamed, moved or deleted, so this roster "
                "points at nothing. That is the roster failing rather than the "
                "client — but it fails the same way, because neither says so."
            )
            continue
        text = path.read_text()
        times = len(re.findall(proof.test, text))
        if times != 1:
            problems.append(
                f"{name}'s renamed-column test occurs {times} times in "
                f"{proof.where}, matching `{proof.test}`.\n"
                "  Exactly one, because the declaration is looked for inside "
                "that test's body and nowhere else. None means the test was "
                "renamed or deleted; more than one means the pattern is too "
                "loose to say which test it means."
            )
            continue
        if re.search(proof.declares, body(text, proof.test)) is None:
            problems.append(
                f"{name}'s renamed-column test no longer declares an old "
                f"spelling: no `{proof.declares}` between that test and the "
                "next one.\n"
                "  A test that merely mentions renaming proves nothing. It has "
                "to hand the server a table declaring a column under a name "
                "the catalog has since renamed away, and be served. Searched "
                "in the body rather than the file because two of these three "
                "files declare the same old spelling a second time, in a test "
                "about something else — a file-wide search stayed green with "
                "the real declaration cut out, which is how this rule was "
                "found to be checking nothing."
            )
        elif why := missing(*proof.renamed):
            problems.append(
                f"{name}'s renamed-column test declares a name the catalog no "
                f"longer calls an old one: {why}.\n"
                "  The test still passes, and passes for nothing: a spelling "
                "that is not a previous name is just the name. Restore the "
                "rename, or point the test at one that is still a rename."
            )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    print(
        f"ok    {len(found)} clients, each with a test that declares a "
        "renamed column's previous name and is served"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
