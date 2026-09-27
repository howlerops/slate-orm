#!/usr/bin/env python3
"""Tests for `check_secret_types.py`, over trees this file writes.

Run against the real tree alone, every rule would pass for as long as the tree
stayed correct — which is the failure the guard exists to prevent, one level
up. So each case writes a small `crates/` and the real tree is checked last as
the case that says "and this repository agrees".

Run directly: `python3 scripts/test_check_secret_types.py`.
"""

from __future__ import annotations

import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_secret_types as guard

#: A rostered type, written the way the real ones are: no derived `Debug`, a
#: hand-written one, and nothing else.
SAFE = (
    "/// One accepted token.\n"
    "struct Bearer {\n"
    "    name: String,\n"
    "    secret: Vec<u8>,\n"
    "}\n"
    "\n"
    "impl core::fmt::Debug for Bearer {\n"
    "    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {\n"
    '        f.write_str("Bearer")\n'
    "    }\n"
    "}\n"
)

#: The roster every case starts from.
ROSTER = {"Bearer": "the fixture's own"}


def run(files: dict[str, str], roster: dict[str, str] | None = None) -> list[str]:
    """The real guard over a `crates/` this writes."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory) / "crates"
        for name, body in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        return guard.problems(root, ROSTER if roster is None else roster)


#: name, the files, the roster (`None` for the default), and the text the
#: report must carry. An empty expectation means nothing is reported.
CASES: list[tuple[str, dict[str, str], dict[str, str] | None, str]] = [
    (
        "a rostered type with a hand-written Debug is clean",
        {"a/src/auth.rs": SAFE},
        None,
        "",
    ),
    (
        # The half the caveat asked for: a third type nobody wrote a test for.
        "a secret-holding type that is not rostered is reported",
        {"a/src/auth.rs": SAFE, "a/src/other.rs": "struct Third {\n    password: String,\n}\n"},
        None,
        "`Third` in crates/a/src/other.rs holds a secret and is not in HOLDS_SECRET",
    ),
    (
        "a derived Debug on a secret-holding type is reported",
        {"a/src/auth.rs": "#[derive(Debug)]\nstruct Bearer {\n    secret: Vec<u8>,\n}\n"},
        None,
        "derives `Debug` and holds a secret",
    ),
    (
        # The doors the caveat names beyond `Debug`.
        "a derived Serialize is reported",
        {"a/src/auth.rs": "#[derive(Clone, Serialize)]\nstruct Bearer {\n    secret: Vec<u8>,\n}\n"},
        None,
        "derives `Serialize` and holds a secret",
    ),
    (
        "an implemented Display is reported",
        {
            "a/src/auth.rs": SAFE
            + "\nimpl core::fmt::Display for Bearer {\n"
            "    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) "
            "-> core::fmt::Result { Ok(()) }\n}\n"
        },
        None,
        "implements `Display` and holds a secret",
    ),
    (
        "a derive that belongs to some other item is not read as this one's",
        {
            "a/src/auth.rs": "#[derive(Debug)]\nstruct Unrelated {\n    n: u8,\n}\n\n" + SAFE
        },
        None,
        "",
    ),
    (
        # A derive above something that is *not* a struct — an enum, a type
        # alias, a function — must not carry down to the next struct. A
        # mutation deleting that reset survived the case above, because a
        # `struct` line consumes the derives itself; only a non-struct item
        # between the two reaches the arm.
        "a derive above an enum does not carry down to the next struct",
        {
            "a/src/auth.rs": "#[derive(Debug)]\nenum Kind {\n    One,\n}\n\n" + SAFE
        },
        None,
        "",
    ),
    (
        # The suffix rule, and it is load-bearing: `slate-serverd`'s config
        # carries `secret_env` and `secret_file`, which name an environment
        # variable and a path. Treating those as secrets would put the whole
        # configuration type on the roster and make it meaningless.
        "a field naming where a secret lives is not a secret",
        {
            "a/src/auth.rs": SAFE,
            "a/src/config.rs": "struct Token {\n"
            "    secret_env: Option<String>,\n"
            "    secret_file: Option<String>,\n}\n",
        },
        None,
        "",
    ),
    (
        "a roster entry for a type that no longer holds a secret is reported",
        {"a/src/auth.rs": SAFE},
        {"Bearer": "the fixture's own", "Gone": "a type that was renamed"},
        "HOLDS_SECRET names `Gone`, which no longer holds a field named for a secret",
    ),
    (
        # The never-fires half. A renamed field, or a `SECRET_FIELD` that
        # stopped matching, leaves this printing `ok` over a tree it read and
        # understood none of.
        "a tree with no secret field anywhere is reported, not passed over",
        {"a/src/auth.rs": "struct Plain {\n    n: u8,\n}\n"},
        None,
        "no struct anywhere holds a field named for a secret",
    ),
]


def main() -> int:
    failed = 0
    for name, files, roster, wanted in CASES:
        try:
            found = run(files, roster)
        except Exception as raised:  # noqa: BLE001 - a crash is this case failing
            found = [f"raised {raised!r} instead of reporting a problem"]
        ok = (not found) if not wanted else any(wanted in one for one in found)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    found = guard.problems()
    ok = not found
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  this repository's two secret types are quiet")
    if not ok:
        for one in found:
            print(f"      {one}")

    print()
    print(f"{len(CASES) + 1 - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
