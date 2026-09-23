#!/usr/bin/env python3
"""No client's `Identity` grows a slot for a credential unnoticed.

Security finding 10 was that the Python client's `Identity.__repr__` printed
`extra` — the field its own docstring says carries "a bearer token, a mesh
header" — so a caller's credential reached any traceback, log line or debugger
that formatted it. The server end of the same wire redacts that token in two
places, by hand, for exactly this reason.

Go and TypeScript were spared by an accident of scope: their `Identity` types
carry the caller's principal, tenant and roles and nothing else, so neither can
hold a credential and neither has a formatter to leak one. That is true today
and nothing says it must stay true — which is the shape this repository has
been bitten by five times in one session, each time a fix that covered one path
of several.

So: **an `Identity` in any client may declare the three identity members and no
others.** A fourth is not forbidden; it is a decision, and this makes somebody
make it. What it costs to add one is an entry in `ALLOWED` here with a reason
the member cannot hold a secret, or a redacting formatter and a test like
Python's.

The criterion is *the member exists*, not *the member looks sensitive*. A
name-based rule — flagging `token`, `secret`, `auth` — is a proxy for the
hazard rather than the hazard, and Python's is spelled `extra`.

Run directly: `python3 scripts/check_client_identity.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: The three identity members, per language's spelling. Python's `Identity`
#: takes them as constructor parameters and is checked by its own suite
#: (`clients/python/tests/test_identity_repr.py`), which asserts the roster of
#: keys the `repr` prints in full; it is named here so the set of clients this
#: file accounts for is visible rather than implied.
IDENTITY = {
    "go": ("Principal", "Tenant", "Roles"),
    "typescript": ("principal", "tenant", "roles"),
}

#: Members beyond the three, and why each cannot carry a credential.
#:
#: Empty, and that is the point: Python's `extra` is the only such member in
#: any client, it does carry credentials, and it is handled by redacting its
#: value rather than by an exemption here. A new entry needs a reason a reader
#: can check, not a note that somebody once looked.
ALLOWED: dict[str, str] = {}

GO = ROOT / "clients" / "go" / "slate" / "client.go"
TYPESCRIPT = ROOT / "clients" / "typescript" / "src" / "client.ts"

GO_STRUCT = re.compile(r"^type Identity struct \{(.*?)^\}", re.M | re.S)
GO_FIELD = re.compile(r"^\t([A-Z]\w*)\s", re.M)
TS_INTERFACE = re.compile(r"^export interface Identity \{(.*?)^\}", re.M | re.S)
TS_MEMBER = re.compile(r"^  (?:readonly )?([a-z]\w*)\??:", re.M)

#: A formatter on the Go type would print whatever it holds, the way Python's
#: `repr` did. There is none today and the three members are not secrets, so
#: this is not an error on its own — but a `String()` *and* a fourth member is
#: the combination finding 10 was, and the message says so.
GO_FORMATTER = re.compile(r"^func \(\w+ \*?Identity\) String\(\)", re.M)


def members(path: Path, block: re.Pattern[str], member: re.Pattern[str]) -> list[str] | None:
    if not path.exists():
        return None
    found = block.search(path.read_text())
    return None if found is None else member.findall(found.group(1))


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    go, typescript = (Path(one) for one in argv) if argv else (GO, TYPESCRIPT)

    declared = {
        "go": members(go, GO_STRUCT, GO_FIELD),
        "typescript": members(typescript, TS_INTERFACE, TS_MEMBER),
    }

    problems = []
    for language, found in declared.items():
        if found is None:
            # The never-fires case. `Identity` is the type every one of these
            # clients sends metadata through; if the pattern stops matching,
            # the rule reports nothing and reads as a pass.
            problems.append(
                f"no `Identity` declaration found for {language}. Either it "
                "moved or it is spelled differently now — both need a person, "
                "not a pass."
            )
            continue
        expected = set(IDENTITY[language])
        for name in sorted(set(found) - expected - set(ALLOWED)):
            problems.append(
                f"{language}'s `Identity` declares `{name}`, which is not one "
                f"of {sorted(expected)}.\n"
                "  A new member of this type is a new metadata slot, and a "
                "metadata slot is where a credential goes — security finding "
                "10 was the Python client printing exactly such a member in "
                "its `repr`. Either give it a redacting formatter and a test "
                "like `clients/python/tests/test_identity_repr.py`, or add it "
                "to ALLOWED with a reason it cannot hold a secret."
            )
        for name in sorted(expected - set(found)):
            problems.append(
                f"{language}'s `Identity` no longer declares `{name}`, which "
                "IDENTITY still expects. Fix the list, or this rule is "
                "checking a shape that is gone."
            )

    if go.exists() and GO_FORMATTER.search(go.read_text()) and ALLOWED:
        problems.append(
            "Go's `Identity` has a `String()` and ALLOWED is not empty. That "
            "is finding 10's combination — a formatter and a member that can "
            "hold a secret — and it needs a test asserting the secret does "
            "not print."
        )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    counted = sum(len(found or ()) for found in declared.values())
    print(f"ok    {len(declared)} client identities, {counted} members, none unaccounted for")
    return 0


if __name__ == "__main__":
    sys.exit(main())
