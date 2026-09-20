#!/usr/bin/env python3
"""Every write path in the kernel is accounted for by the disclosure probe.

Security finding 2 was that `write_many` read storage *before* it decided the
row policy, so a caller could send a row carrying another tenant's key and read
the tenant boundary off which error came back. It was fixed there, and
single-row `insert` was quoted in the review as the control — the path that
already did it right.

The single-row **upsert** was neither the subject nor the control, and it went
on disclosing. That is the fourth time in one session a fix covered one path of
several, and every time the remaining paths were judged equivalent by reading.

So the probe now asks the question of every write path at once, and this holds
its roster to the source:

- a `pub` method in `record.rs` that can modify storage must be named in
  `WRITE_PATHS`;
- a name in `WRITE_PATHS` that no longer modifies storage must be removed.

**"Can modify storage" is the criterion, not "takes a context" or "authorises a
write action".** Those are both proxies and both are wrong at the edges: every
read takes a context, and `insert_many` authorises nothing itself — it hands
off to `write_many`. What makes a method a write path is that it reaches one of
the primitives that actually writes, and that is one grep with no call graph.

Run directly: `python3 scripts/check_write_paths.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

SOURCE = ROOT / "crates" / "slate-kernel" / "src" / "record.rs"
PROBE = ROOT / "crates" / "slate-kernel" / "tests" / "security_probe_cascade.rs"

#: The primitives that put bytes in the store or take them out. Everything that
#: writes goes through one of these, so a method that calls none of them —
#: directly or through a private helper that does — writes nothing.
#:
#: `write_many` and `remove_row` are in the list although they are private,
#: because they are the hop between a public method and the real primitive:
#: `insert_many` calls `write_many` calls `write_row_with`. One hop is all the
#: current code needs and adding the intermediate names costs nothing; a second
#: hop would need a call graph, which is the caveat this file's entry carries.
PRIMITIVES = ("write_row", "write_row_with", "erase_row", "remove_row", "write_many")

FUNCTION = re.compile(r"^    (pub )?(?:async )?fn ([a-z_][a-z0-9_]*)")
MUTATES = re.compile(r"self\.(" + "|".join(PRIMITIVES) + r")\(")
ROSTER = re.compile(r"const WRITE_PATHS: \[&str; \d+\] = \[([^\]]*)\]")


def write_paths(text: str) -> set[str]:
    """Public methods that reach a storage primitive."""
    found: set[str] = set()
    name, public = None, False
    for line in text.splitlines():
        heading = FUNCTION.match(line)
        if heading:
            name, public = heading.group(2), bool(heading.group(1))
        # A primitive calling a primitive is the machinery, not a path.
        if public and name and name not in PRIMITIVES and MUTATES.search(line):
            found.add(name)
    return found


def rostered(text: str) -> set[str] | None:
    found = ROSTER.search(text)
    return None if found is None else set(re.findall(r'"(\w+)"', found.group(1)))


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    source, probe = (Path(one) for one in argv) if argv else (SOURCE, PROBE)

    if not source.exists() or not probe.exists():
        print(f"missing {source} or {probe}", file=sys.stderr)
        return 1

    paths = write_paths(source.read_text())
    listed = rostered(probe.read_text())

    problems = []
    if not paths:
        # The never-fires case, and the one this rule is most exposed to: the
        # criterion is a set of private method names, so renaming `write_row`
        # would leave this matching nothing and printing `ok`.
        problems.append(
            f"no write path in {source.name} reaches any of {list(PRIMITIVES)}, "
            "so this checked nothing. Either the storage primitives were "
            "renamed or the writes moved — both need a person, not a pass."
        )
    elif listed is None:
        problems.append(
            f"{len(paths)} write paths in {source.name} and no WRITE_PATHS "
            f"roster in {probe.name} to account for them"
        )
    else:
        for name in sorted(paths - listed):
            problems.append(
                f"`{name}` can modify storage and is not in WRITE_PATHS.\n"
                "  Add it, and put it in one of the probe's tables — finding 2 "
                "was a write path that answered differently for a key taken in "
                "another tenant, and it survived in the path nobody was asking "
                "about."
            )
        for name in sorted(listed - paths):
            problems.append(
                f"WRITE_PATHS names `{name}`, which no longer modifies storage. "
                "Delete it and its case, or the probe covers one path fewer "
                "than its name claims."
            )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    print(f"ok    {len(paths)} write paths, all in the disclosure probe's roster")
    return 0


if __name__ == "__main__":
    sys.exit(main())
