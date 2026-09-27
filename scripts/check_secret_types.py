#!/usr/bin/env python3
"""A type holding a secret does not print it, serialise it, or derive `Debug`.

`ledger/2026-09-20-the-secret-that-debug-prints-as-numbers.md` strengthened two
credential redactions and then recorded the two things it could not close:

> **It covers `Debug`, and a secret can leave by other doors.** Neither test
> says anything about `Display`, about `serde`, about a secret reaching an
> error message, or about one being written to a trace span's fields.

> **Two types, asserted to be the whole surface by enumeration.** [...] A third
> secret-holding type added tomorrow gets no test and nothing will say so. I
> argued above against building a script for it; that argument is a judgement
> about cost, not a claim that the gap is closed.

This is the script that argument was against, and the reason to write it now is
that the enumeration has to be redone by hand every time anyone wonders. Three
doors are checkable from the text — a derived `Debug`, a `Display`, a
`Serialize` — and the fourth, a secret formatted into an error, is not; that
one is below.

# What counts as holding a secret

A struct field whose name contains `secret`, `password`, `passphrase` or
`private_key`, and does **not** end in `_env`, `_file` or `_name`. That suffix
rule is not cosmetic: `slate-serverd`'s config carries `secret_env` and
`secret_file`, which are the *name of an environment variable* and the *path to
a file*. Treating those as secrets would put the daemon's whole configuration
type on the roster and make the roster meaningless, which is the failure mode
of every guard in this directory that matched too much.

`token` is deliberately absent. It is a read token in the reader pool, a lexer
token in `slate-sql`, and a bearer secret in `auth.rs`, and only the third is a
secret. A roster keyed on a word with three meanings is a roster of false
positives.

# The rules

1. **Every struct with a secret field is in `HOLDS_SECRET`.** The half that
   catches the third type nobody wrote a test for.
2. **A rostered struct does not derive `Debug`.** A derived one prints every
   field, which is the defect the two hand-written impls exist to prevent.
3. **A rostered struct does not implement or derive `Display` or `Serialize`.**
   The other doors the caveat names. `Display` is the one that reaches a log
   line through `{}`; `Serialize` is the one that reaches a response body.
4. **Every name in the roster is still a struct that still holds a secret.**
   The reverse half, so a renamed field leaves a roster entry to delete rather
   than a rule that silently guards nothing.

Run directly: `python3 scripts/check_secret_types.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Where to look. Every workspace crate's sources; tests are excluded because a
#: fixture holding a fake secret is not a type this is about.
CRATES = ROOT / "crates"

#: A field that holds a secret, not the name of a place one is kept.
SECRET_FIELD = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?"
    r"([a-z_]*(?:secret|password|passphrase|private_key)[a-z_]*)\s*:",
)

#: `secret_env` is the name of an environment variable; `secret_file` is a
#: path. Neither is a secret, and both are in `slate-serverd`'s config.
NOT_A_SECRET = ("_env", "_file", "_name", "_path", "_var")

#: `struct Name {`, with whatever attributes and doc comments precede it.
STRUCT = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?struct\s+(\w+)")

#: `#[derive(...)]` on the lines above a struct.
DERIVE = re.compile(r"#\[derive\(([^)]*)\)\]")

#: `impl core::fmt::Display for Credentials`, `impl Serialize for Bearer`.
IMPLEMENTS = re.compile(r"^impl(?:<[^>]*>)?\s+(?:[\w:]*::)?(\w+)(?:<[^>]*>)?\s+for\s+(\w+)")

#: The two types that hold a resolved secret in memory, and why each is safe.
#:
#: A roster rather than a shape, for the `EXPECTED_REFUSALS` reason this
#: repository uses everywhere: a list you are forced to edit is a list that
#: stays true. A third type added tomorrow fails rule 1 until somebody writes a
#: line here saying what it is and what keeps it quiet.
HOLDS_SECRET = {
    "Credentials": (
        "slate-slatedb's resolved S3 key pair. A hand-written `Debug` prints "
        "the access key id and `<redacted>` for the other two; nothing "
        "implements Display or Serialize."
    ),
    "Bearer": (
        "slate-serverd's one accepted token. It has no `Debug` at all — the "
        "hand-written one is on `TokenIdentity`, which holds a `Vec<Bearer>` "
        "and prints only the names."
    ),
}


def structs(source: str) -> dict[str, tuple[int, list[str]]]:
    """`{name: (line, derives)}` for every struct in one file."""
    found: dict[str, tuple[int, list[str]]] = {}
    derives: list[str] = []
    for at, line in enumerate(source.splitlines(), start=1):
        matched = DERIVE.search(line)
        if matched:
            derives += [one.strip() for one in matched.group(1).split(",")]
            continue
        named = STRUCT.match(line)
        if named:
            found[named.group(1)] = (at, derives)
            derives = []
            continue
        # A blank line or anything else between a derive and a struct means
        # the derive was for something else. Doc comments and other attributes
        # do not reset it, because they legitimately sit in between.
        if not line.strip().startswith(("///", "//", "#[")) and line.strip():
            derives = []
    return found


def holders(root: Path = CRATES) -> dict[str, Path]:
    """Every struct that has a secret field, by name."""
    found: dict[str, Path] = {}
    for path in sorted(root.rglob("*.rs")):
        if "/tests/" in str(path) or path.name.startswith("test_"):
            continue
        source = path.read_text(encoding="utf-8", errors="replace")
        names = structs(source)
        if not names:
            continue
        # Which struct a field belongs to: the last one opened above it. Good
        # enough because every struct here is a flat block, and a nested one
        # would be reported against its outer type, which is a name a reader
        # can still find.
        order = sorted(names.items(), key=lambda pair: pair[1][0])
        for at, line in enumerate(source.splitlines(), start=1):
            field = SECRET_FIELD.match(line)
            if not field or field.group(1).endswith(NOT_A_SECRET):
                continue
            owner = None
            for name, (opened, _) in order:
                if opened < at:
                    owner = name
                else:
                    break
            if owner:
                found[owner] = path
    return found


def problems(root: Path = CRATES, roster: dict[str, str] | None = None) -> list[str]:
    """Every rule above, against a tree."""
    roster = HOLDS_SECRET if roster is None else roster
    said = []
    found = holders(root)

    if not found:
        said.append(
            "no struct anywhere holds a field named for a secret, which was "
            "false for two of them when this was written. Either the fields "
            "were renamed or SECRET_FIELD no longer matches them — both need "
            "a person, not a pass."
        )
        return said

    for name in sorted(found):
        if name not in roster:
            said.append(
                f"`{name}` in {found[name].relative_to(root.parent)} holds a "
                "secret and is not in HOLDS_SECRET.\n"
                "  Add a line saying what it is and what keeps the secret out "
                "of a log line, or rename the field if it is\n"
                "  the name of a place rather than the thing itself."
            )

    for name in sorted(roster):
        if name not in found:
            said.append(
                f"HOLDS_SECRET names `{name}`, which no longer holds a field "
                "named for a secret. Delete the line, or the roster\n"
                "  is describing something that is not there."
            )

    for name, path in sorted(found.items()):
        source = path.read_text(encoding="utf-8", errors="replace")
        at, derives = structs(source)[name]
        for door in ("Debug", "Display", "Serialize"):
            if door in derives:
                said.append(
                    f"`{name}` at {path.relative_to(root.parent)}:{at} derives "
                    f"`{door}` and holds a secret.\n"
                    f"  A derived `{door}` prints every field. Write it by "
                    "hand and redact, or do not implement it."
                )
        for line in source.splitlines():
            implemented = IMPLEMENTS.match(line)
            if not implemented:
                continue
            trait, subject = implemented.group(1), implemented.group(2)
            if subject == name and trait in ("Display", "Serialize"):
                said.append(
                    f"`{name}` implements `{trait}` and holds a secret.\n"
                    f"  `Display` reaches a log line through `{{}}`; "
                    "`Serialize` reaches a response body. Neither should "
                    "carry it."
                )
    return said


def main() -> int:
    said = problems()
    if said:
        for one in said:
            print(one, file=sys.stderr)
        print(f"\n{len(said)} problem(s)", file=sys.stderr)
        return 1
    found = holders()
    print(
        f"ok    {len(found)} type(s) hold a secret, all rostered, none "
        "deriving Debug or reaching Display or Serialize"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
