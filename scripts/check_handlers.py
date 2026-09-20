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

#: Where the handlers live. Directories rather than one file, because the
#: first version of this check read `service.rs` alone and missed a
#: `fingerprint::check` in `convert.rs` — a file away, reachable only through
#: callers, and invisible to a check whose whole subject is "is this reachable
#: without authorising". That is the caveat this check's own entry named,
#: biting within the hour.
SOURCES = (
    ROOT / "crates" / "slate-server" / "src",
    ROOT / "crates" / "slate-serverd" / "src",
)

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
#: `fingerprint::check` calls whose table was authorised by their *caller*, and
#: why that is safe rather than a hole.
#:
#: A function taking a `&TableDef` cannot authorise: it has no name to resolve
#: and no context to resolve it for. The check it owes is on its callers, and
#: those are the sites rule 2 covers. Listed rather than skipped by shape,
#: because "takes a table so somebody else checked" is exactly the reasoning
#: that was wrong three times today, and writing it down forces it to be
#: re-argued when a caller is added.
FINGERPRINT_BY_CALLER = {
    "query_from_proto_at": (
        "takes an already-resolved `&TableDef`; its callers are `query`, "
        "`explain` and the join and chain handlers, each of which authorises "
        "before converting — which they did not until finding 8 was fixed the "
        "second time, and which rule 2 now holds them to"
    ),
}

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
    sources = [Path(one) for one in argv] if argv else list(SOURCES)

    files: list[Path] = []
    for source in sources:
        if source.is_dir():
            files.extend(sorted(source.rglob("*.rs")))
        elif source.exists():
            files.append(source)

    problems: list[str] = []
    seen: set[str] = set()
    bare = 0
    checks = 0

    for path in files:
        lines = path.read_text().splitlines()
        names = enclosing_functions(lines)
        for at, line in enumerate(lines):
            if BARE.search(line):
                bare += 1
                owner = names[at]
                seen.add(owner)
                if owner not in UNAUTHORIZED:
                    problems.append(
                        f"{path.name}:{at + 1}: `{owner}` resolves a table with the "
                        "bare `self.table(..)`, which does not authorise.\n"
                        "  Use `self.authorized_table(&context, name, action)` with "
                        "the action the kernel will check, or add it to "
                        "UNAUTHORIZED with a reason it is safe."
                    )
            if FINGERPRINT.search(line):
                checks += 1
                owner = names[at]
                seen.add(owner)
                window = lines[max(0, at - REACH) : at]
                if owner in FINGERPRINT_BY_CALLER:
                    continue
                if not any(AUTHORIZED.search(above) for above in window):
                    problems.append(
                        f"{path.name}:{at + 1}: `fingerprint::check` in `{owner}` "
                        f"with no `authorized_table` in the {REACH} lines above "
                        "it.\n"
                        "  Fingerprinting before authorising is security finding "
                        "8: a caller with no grant confirms a guessed schema one "
                        "fingerprint at a time. If the table arrives already "
                        "authorised, add `{owner}` to FINGERPRINT_BY_CALLER with "
                        "the callers that check it."
                    )

    # A check that finds nothing has stopped checking, and reads identically to
    # one that found nothing wrong. `CLAUDE.md`: "a check that never fires is a
    # check nobody has debugged". If the handlers move, this fails rather than
    # going quietly green over an empty tree.
    if not files:
        problems.append(f"no Rust sources under {[str(one) for one in sources]}")
    elif checks == 0:
        problems.append(
            f"no `fingerprint::check` anywhere in {len(files)} file(s). Either "
            "the handlers moved and SOURCES is stale, or the check is no longer "
            "what this guards — both need a person, not a pass."
        )

    # A stale exemption is its own defect: it reads as a live hazard somebody
    # accepted, and the next person weighs a decision nobody is making.
    for listed, where in ((UNAUTHORIZED, "UNAUTHORIZED"), (FINGERPRINT_BY_CALLER, "FINGERPRINT_BY_CALLER")):
        for name, reason in listed.items():
            if name not in seen:
                problems.append(
                    f"{where} lists `{name}`, which no longer does what the entry "
                    f"exempts. Delete it; its reason was: {reason}"
                )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    print(
        f"ok    {len(files)} files, {bare} bare resolutions all accounted for, "
        f"{checks} fingerprint checks all authorised first"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
