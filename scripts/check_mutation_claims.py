#!/usr/bin/env python3
"""A ledger entry that counts mutations must cite the run that produced them.

#287 made every `mutate.py` run write a record under `ledger/mutations/`, and
closed by naming what it had *not* done: "an entry can still say 'four caught,
none survived' with no run behind it, or cite a run whose record says
otherwise. The records make that checkable and nothing checks it."

This is that check, and it is narrower than it first looks.

WHY IT IS DATE-SCOPED, AND WHY THAT IS NOT A LOOPHOLE

245 entries mention mutation; 73 state a count. Four records exist, because
records began on 2026-09-22. A rule applied to all of them would fail 241
entries for predating the mechanism that would have satisfied them, which is
not a finding about anything.

So the rule starts on `FIRST_DAY` and the exemption is by construction rather
than by a list somebody prunes — the same shape as the frozen measurement-table
roster in `check_table_provenance.py`. An entry dated before it cannot be made
to comply; an entry dated after it has no excuse.

WHY CITATION AND NOT INFERENCE

The tempting version pairs entries with records by date and filename: an entry
written today claiming a mutation of `x.py` should have a record from today
touching `x.py`. That is wrong in both directions. An entry legitimately cites
a run from the day before, several entries can share one run, and one task
often runs the same file three times — so "a record exists nearby" is not
evidence that *this* claim came from it, and demanding a same-day record would
fail honest entries.

An explicit citation says which run. That also matches how this repository
already treats the ledger: `check_cited_docs.py` deliberately refuses to
validate `docs/…` paths cited *from* `ledger/`, because an entry records what
was true on its day and rewriting it destroys the record. A citation to a
mutation record is exempt from that worry for a reason worth stating — records
are themselves dated, append-only, and never rewritten, so a link between two
dated records cannot rot the way a link into a moving tree does.

WHAT IT VERIFIES

1. The cited record exists.
2. If the entry claims every mutation was caught, the record holds no survivor.

The second is the one with teeth. "All caught" beside a record showing a
survivor is precisely the discrepancy #287 said nothing could see.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LEDGER = ROOT / "ledger"
RECORDS = LEDGER / "mutations"

#: The first day an entry could have cited a record.
#:
#: `mutate.py` began writing records on 2026-09-22, partway through the day, so
#: entries dated that day may predate the first record. The rule starts the day
#: after: the first date on which every run of the whole day left a trace.
FIRST_DAY = "2026-09-23"

#: `ledger/2026-09-23-a-slug.md`
DATED = re.compile(r"^(\d{4}-\d{2}-\d{2})-")

#: Three ways an entry says it ran mutations.
#:
#: A count, or a table, is required. Without one this matches the ambient prose
#: every entry in this repository carries — "a surviving mutation is a missing
#: test" is a statement of policy, not a claim to have run one.
#:
#: The first version required the number *before* the noun, and on 2026-09-26
#: three entries in one session slipped past it: "A mutation, run twice, caught
#: both times", "**Mutations**, six run, six caught", and "Six were run". All
#: three had run real mutations, all three had records sitting in
#: `ledger/mutations/`, and none cited one — which is exactly the failure this
#: check exists to catch, going uncaught because the claim was phrased in a
#: word order the pattern did not have. A guard that only recognises one way of
#: saying a thing is a guard on a phrasing, not on a practice.
NUMBER = r"(?:one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|\d+)"

COUNT = re.compile(
    # "Five mutations", "4 mutations", "three mutations against the recording".
    rf"\b{NUMBER}\s+mutations?\b"
    # "Mutations: six run, six caught", "mutations, five run". The number
    # after the noun rather than before it, which is how an evidence heading
    # reads when the word comes first.
    rf"|\bmutations?\b[^.\n]{{0,30}}\b{NUMBER}\s+(?:run|caught|survived)\b"
    # The evidence table every such claim in this repository carries:
    # `| mutation | caught by |`. The most reliable signal of the three,
    # because the table is a fixed shape and the prose above it is not.
    r"|^\|\s*mutation\s*\|",
    re.IGNORECASE | re.MULTILINE,
)

#: A citation of a specific run.
CITATION = re.compile(r"ledger/mutations/([0-9A-Za-z._-]+\.json)")

#: "all caught", "none survived", "every one caught".
#:
#: Deliberately not "no survivors", which appears in this repository as a
#: description of what a clean run *means* rather than a claim about one.
CLEAN_CLAIM = re.compile(
    r"\ball caught\b|\bnone survived\b|\bevery one caught\b|\bno survivor\b",
    re.IGNORECASE,
)


def entries(ledger: Path = LEDGER) -> list[Path]:
    """Dated entries, not `README.md`, `TEMPLATE.md` or the records."""
    return [
        path for path in sorted(ledger.glob("*.md")) if path.is_file() and DATED.match(path.name)
    ]


def survivors(record: Path) -> list[str]:
    """Case names the record scored as surviving, expectedly or not.

    `survived-as-recorded` is a survivor too. An entry claiming "all caught"
    beside a run where one survived on purpose is still a sentence that does
    not describe the run, and the fix is to say so rather than to widen what
    "caught" means.
    """
    try:
        held = json.loads(record.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    return [
        case.get("name", "?")
        for case in held.get("cases", [])
        if str(case.get("verdict", "")).startswith("survived")
    ]


def check(ledger: Path = LEDGER, records: Path = RECORDS) -> tuple[int, list[str]]:
    """Returns how many claims were read, and which are unsupported."""
    seen = 0
    wrong: list[str] = []
    for path in entries(ledger):
        stamped = DATED.match(path.name)
        if not stamped or stamped.group(1) < FIRST_DAY:
            continue
        text = path.read_text(encoding="utf-8")
        if not COUNT.search(text):
            continue
        seen += 1
        cited = CITATION.findall(text)
        if not cited:
            wrong.append(
                f"ledger/{path.name} counts mutations and cites no run.\n"
                f"  `mutate.py` records every run under `ledger/mutations/`; "
                f"name the one behind the claim,\n"
                f"  as `ledger/mutations/<file>.json`. Entries before "
                f"{FIRST_DAY} are exempt — records did not exist."
            )
            continue
        claims_clean = bool(CLEAN_CLAIM.search(text))
        for name in cited:
            record = records / name
            if not record.is_file():
                wrong.append(
                    f"ledger/{path.name} cites ledger/mutations/{name}, which "
                    f"is not there.\n"
                    f"  Records are append-only; a citation to one that is "
                    f"missing means the wrong name was written."
                )
                continue
            left = survivors(record)
            if claims_clean and left:
                wrong.append(
                    f"ledger/{path.name} says every mutation was caught, but "
                    f"ledger/mutations/{name} records\n"
                    f"  {len(left)} survivor(s): {', '.join(left[:3])}.\n"
                    f"  Either the entry is describing a different run, or it "
                    f"is describing this one wrongly."
                )
    return seen, wrong


def main(ledger: Path = LEDGER, records: Path = RECORDS) -> int:
    seen, wrong = check(ledger, records)
    for problem in wrong:
        print(problem, file=sys.stderr)
    if wrong:
        print(f"\n{len(wrong)} problem(s) of {seen} counted claims", file=sys.stderr)
        return 1
    # No never-fires guard here, unlike the other checkers in this directory,
    # and the reason is the date scope: zero claims in scope is the *correct*
    # answer on any day when no entry since FIRST_DAY counted a mutation. A
    # guard that demanded a non-zero count would fail every quiet week.
    print(f"ok    {seen} entries count mutations, all citing a recorded run")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
