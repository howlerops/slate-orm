#!/usr/bin/env python3
"""Every guard has a real-tree mutation, or a written reason it has none.

`scripts/mutations.json` holds one mutation per guard: a change to a file that
guard watches, which it must refuse. `scripts/run_mutations.py` runs them. This
checks the roster is complete and still points at what it says it does, and it
is the half that belongs in `scripts/check.sh` — it reads files and changes
nothing, where the runner breaks the tree and puts it back.

The reason it exists is the survey that produced the roster. Three guards were
green for reasons unrelated to what they check, and each was found by mutating
the real tree rather than a fixture. A twenty-ninth guard arriving with no
mutation would be in exactly that position and nothing would say so — which is
the shape `check_transport_door.py` and `check_renamed_column.py` already guard
for clients, applied to the guards themselves.

`unmutated` is the escape hatch, and it is one sentence per guard rather than a
list of names, because "we have not got to it yet" is a reason and should be
written as one. An entry there for a guard that *does* have a suite is itself a
failure, so the two cannot both be true.

What this cannot check is whether a mutation is a good one — whether it breaks
the property the guard exists for, rather than something incidental the guard
happens to notice. That is `run_mutations.py`'s job only in the weak sense that
a bad mutation still gets caught; the judgement is a person's, and the `why` on
each suite is where it is written down.

Run directly: `python3 scripts/check_mutations_roster.py`.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROSTER = ROOT / "scripts" / "mutations.json"
GUARDS = "check_*.py"

#: Guards that check the roster itself, and would be checking their own
#: mutation. Kept to the two that are genuinely reflexive rather than used as a
#: general exemption — anything else goes in `unmutated` with a sentence.
REFLEXIVE = ("check_mutations_roster",)


def guards() -> list[str]:
    return sorted(
        path.stem for path in (ROOT / "scripts").glob(GUARDS) if not path.name.startswith("test_")
    )


def main() -> int:
    problems: list[str] = []
    roster = json.loads(ROSTER.read_text())
    # Keyed by (guard, file), not by guard. A guard that watches two trees
    # needs a suite per tree — `mutate.py` takes one `file` per spec — and
    # `check_workspace.py` is the first: its member-list rule mutates the root
    # manifest and its publish rule mutates a member's. Keyed by guard alone,
    # the second suite was invisible *here* while `run_mutations.py` ran it
    # perfectly well, so the collision message below was right about this file
    # and wrong about the runner.
    suites = {(suite["guard"], suite["file"]): suite for suite in roster["suites"]}
    #: The guards a suite exists for, which is what the coverage rules below
    #: ask about. A guard with two suites is covered once.
    covered = {guard for guard, _ in suites}
    excused = roster["unmutated"]
    found = guards()

    if not found:
        # The never-fires case: no guards found means the glob or the tree
        # moved, and every rule below would compare two empty sets.
        print(
            f"no `scripts/{GUARDS}` at all, so this compared nothing.",
            file=sys.stderr,
        )
        return 1

    if len(suites) != len(roster["suites"]):
        problems.append(
            "two suites name the same guard *and* the same file. One of them "
            "is invisible to this check, and which is an accident of file "
            "order. Two suites for one guard are fine when they mutate "
            "different files; two for one file are two names for one thing."
        )

    for guard in found:
        if guard in REFLEXIVE:
            continue
        if guard in covered and guard in excused:
            problems.append(
                f"{guard} has a suite and an `unmutated` reason.\n"
                "  Both cannot be true. Drop the reason."
            )
        elif guard not in covered and guard not in excused:
            problems.append(
                f"scripts/{guard}.py has no real-tree mutation and no reason "
                "for having none.\n"
                "  A guard's fixture tests prove its rules refuse what they "
                "should; only breaking the real tree proves the rules are "
                "pointed at anything. Three guards were green for unrelated "
                "reasons and every one was found this way. Add a suite to "
                "scripts/mutations.json, or a sentence to `unmutated` saying "
                "why there is none."
            )

    for guard in sorted(covered | set(excused)):
        if guard not in found and guard not in REFLEXIVE:
            problems.append(
                f"scripts/mutations.json names {guard}, which is not a guard "
                "under scripts/ any more. Drop it."
            )

    for (guard, _file), suite in sorted(suites.items()):
        path = ROOT / suite["file"]
        if not path.exists():
            problems.append(
                f"{guard}'s mutation targets {suite['file']}, which is not a "
                "file. The suite cannot run; point it at what the guard "
                "watches now."
            )
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        for case in suite["cases"]:
            times = text.count(case["old"])
            if times != 1:
                problems.append(
                    f"{guard}: `{case['name']}` anchors on text occurring "
                    f"{times} times in {suite['file']}.\n"
                    "  `scripts/mutate.py` refuses a patch that is not exactly "
                    "once, so this suite would error rather than run — and the "
                    "usual cause is a reformat moving the anchor, which is the "
                    "first of the six lies that script's docstring lists."
                )
        if not suite.get("why", "").strip():
            problems.append(
                f"{guard}'s suite has no `why`. What a mutation breaks is the "
                "judgement this file cannot make for itself; write it down."
            )

    for guard, reason in sorted(excused.items()):
        if not reason.strip():
            problems.append(f"`unmutated` names {guard} with no reason.")

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    print(
        f"ok    {len(found)} guards, {len(covered)} with a real-tree mutation "
        f"({len(suites)} suites), "
        f"{len(excused)} with a written reason for having none"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
