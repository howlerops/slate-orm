#!/usr/bin/env python3
"""A handler must authorise before it touches a table's shape.

Security finding 8 of `docs/security-review.md` was that a handler resolved its
table and *used* it — fingerprinting it, converting a request against it —
before anything checked the caller's grant, so a caller with no grant read the
schema out of the refusals. It was fixed twice, months apart, because the first
fix covered the handlers the finding was written about and the read paths went
on doing it. `query` gave up a table's exact column count, in one request, to a
role granted nothing.

That is the third time in one session a fix covered one path of several — the
cross-tenant catalog refusal covered one constructor of two, and the duplicate
identity header one `Authenticator` of two. Each time the remaining paths
"looked the same" from reading. This is the check that makes the next one fail
loudly instead:

1. **Every bare `self.table(..)`** — the resolver that does *not* authorise —
   is either inside `authorized_table` or listed in `UNAUTHORIZED` with a
   reason it is safe.
2. **Every `fingerprint::check(..)`** has an `authorized_table` above it in the
   same function, within `REACH` lines. The fingerprint is the original
   channel and the ordering is the whole fix.

Neither is a proof. A handler can hold a `&TableDef` from one of the accounted
sites and pass it along, and this will not see it. What it does is make adding
a *new* unauthorised resolution a failure rather than a silence, which is the
`EXPECTED_REFUSALS` idiom this repository already uses in three places: a list
you are forced to edit is a list that stays true.

Run directly: `python3 scripts/check_handlers.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERVICE = ROOT / "crates" / "slate-server" / "src" / "service.rs"

#: How far above a `fingerprint::check` its authorisation may sit.
#:
#: Four, because in every current site they are adjacent or one line apart, and
#: a check that allowed twenty would accept a handler that authorised one table
#: and fingerprinted another. If a legitimate site ever needs more, widening
#: this is a deliberate edit — which is the point.
REACH = 4

#: Bare `self.table(..)` calls that are *not* a disclosure, and why.
#:
#: Keyed on the enclosing function. The reason is the entry's whole value: a
#: reader deciding whether their new call belongs here needs to know what makes
#: these safe, not that somebody once said so.
UNAUTHORIZED = {
    "authorized_table": (
        "this is the authorising resolver itself; it calls the bare one and "
        "then authorises"
    ),
    "resolve_relation": (
        "reached only from `related`, which authorises `relation.table` "
        "immediately before calling it — see the comment at that call site. "
        "Resolving discloses the child's foreign keys, so the grant is checked "
        "before, not here, because this function does not have the context"
    ),
}

FUNCTION = re.compile(r"^\s*(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)")
BARE = re.compile(r"self\.table\(")
FINGERPRINT = re.compile(r"fingerprint::check\(")
AUTHORIZED = re.compile(r"authorized_table\(")


def enclosing_functions(lines: list[str]) -> list[str]:
    """The innermost `fn` name in scope at each line.

    Indentation rather than brace counting: a nested `fn` inside a handler —
    `resolve_relation` has one — is the name a reader would give that line, and
    both are found by taking the most recent `fn` above it.
    """
    names: list[str] = []
    current = "<file>"
    for line in lines:
        found = FUNCTION.match(line)
        if found:
            current = found.group(1)
        names.append(current)
    return names


def main(argv: list[str] | None = None) -> int:
    # The subject is an argument so the tests beside this file can run the real
    # checks over a file they wrote, rather than against `service.rs` — where a
    # broken check passes for whatever `service.rs` happens to contain.
    argv = sys.argv[1:] if argv is None else argv
    service = Path(argv[0]) if argv else SERVICE

    lines = service.read_text().splitlines()
    names = enclosing_functions(lines)
    problems: list[str] = []
    seen: set[str] = set()

    for at, line in enumerate(lines):
        if BARE.search(line):
            owner = names[at]
            seen.add(owner)
            if owner not in UNAUTHORIZED:
                problems.append(
                    f"{service.name}:{at + 1}: `{owner}` resolves a table with the "
                    "bare `self.table(..)`, which does not authorise.\n"
                    "  Use `self.authorized_table(&context, name, action)` with the "
                    "action the kernel will check, or add it to "
                    "UNAUTHORIZED with a reason it is safe."
                )
        if FINGERPRINT.search(line):
            window = lines[max(0, at - REACH) : at]
            if not any(AUTHORIZED.search(above) for above in window):
                problems.append(
                    f"{service.name}:{at + 1}: `fingerprint::check` with no "
                    f"`authorized_table` in the {REACH} lines above it.\n"
                    "  Fingerprinting before authorising is security finding 8: "
                    "a caller with no grant confirms a guessed schema one "
                    "fingerprint at a time."
                )

    # A stale exemption is its own defect: it reads as a live hazard somebody
    # accepted, and the next person weighs a decision nobody is making.
    for name, reason in UNAUTHORIZED.items():
        if name not in seen:
            problems.append(
                f"UNAUTHORIZED lists `{name}`, which no longer resolves a table "
                f"that way. Delete the entry; its reason was: {reason}"
            )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    bare = sum(1 for line in lines if BARE.search(line))
    checks = sum(1 for line in lines if FINGERPRINT.search(line))
    print(
        f"ok    {bare} bare resolutions, all accounted for; "
        f"{checks} fingerprint checks, all authorised first"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
