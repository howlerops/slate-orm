#!/usr/bin/env python3
"""Every measurement table in `docs/` says what build produced it.

#280 made nineteen programs print a `build:` line before their first number.
It did not make anything check that a table *recorded from* one of those runs
kept the line — and a recorded table is where a number is actually read. The
prose in `docs/performance.md` already draws the boundary ("tables on this page
that predate 2026-09-22 have no build line"); until this existed, nothing held
it.

The shape is an inverted roster, the same as `check_build_stamp.py`: rather
than listing the tables that need provenance, this finds every table in
`docs/`, decides which ones record a measurement, and requires each of those to
be *accounted for* — stamped, excused, or frozen as predating the rule. A new
timing table added tomorrow is caught because it is new, not because somebody
remembered to add it to a list.

A table is accounted for when one of three things is true:

* **Stamped** — a `build:` line appears between the nearest heading above the
  table and the table itself. This is the answer for anything measured from
  now on.
* **Excused** — `<!-- not a measurement -->` sits within three lines above it.
  The classifier below is deliberately over-eager (see `is_measurement`), so
  there has to be a way to say "this table of line counts is not a benchmark".
  It is a sentence an author writes on purpose, which is the point.
* **Frozen** — its contents match `FROZEN`, the roster of tables that predate
  the rule. Those runs are gone and a build line cannot be reconstructed for
  them; what is recorded about them is their own prose.

The roster pins table *contents*, not just position, so editing the numbers in
a legacy table takes it out of the roster and demands a stamp. That is the
intent: re-measuring is exactly when provenance becomes recordable again.
"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from collections.abc import Sequence
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
ROSTER = ROOT / "scripts" / "frozen_tables.json"

#: The separator row under a markdown table's header: `|---|---:|`.
#:
#: The `-` sits at the end of the character class on purpose. Written
#: `[\s:-|]` it is a *range* from `:` (58) to `|` (124), which excludes `-`
#: (45) and matches no separator at all — an exploratory version of this
#: function reported 3 tables in a tree that has 82, and the count was wrong
#: in the direction that looks like "there is nothing to check here".
SEPARATOR = re.compile(r"^\|[\s:|-]+\|\s*$")

#: A unit attached to a number, as it appears in a cell: `231 ms`, `1,221
#: requests`, `53 GETs`.
# U+00D7 MULTIPLICATION SIGN is the character this page actually uses to
# write a speedup, so it has to be matched literally. `noqa` because ruff
# reasonably calls it confusable with ASCII `x` — which is matched too, right
# beside it, since both spellings occur.
UNIT = (
    r"(ms|µs|us|ns|s|sec|secs|MB|KB|GB|B|requests?|GETs?|PUTs?|LISTs?"
    r"|rows?|%|×|x|ops/s|/s|bytes?)"  # noqa: RUF001
)
IN_CELL = re.compile(r"(?<![\w.])\d[\d,]*(?:\.\d+)?\s*" + UNIT + r"\b")

#: The same idea one row up. Half of this page's measurement tables put the
#: unit in the header and a bare number in the cell — `| | GETs | rows/GET |`
#: over `| read nothing at all | 53 | 3,774 |`. Matching cells alone missed
#: every one of those, including the two tables that carry the scan-cost
#: calibration, so the header is read too.
IN_HEADER = re.compile(
    r"\b(ms|µs|ns|sec|MB|KB|GB|bytes?|requests?|GETs?|PUTs?|LISTs?|rows?"
    r"|ops/s|wall|time|latency|cost|throughput|speedup|measured|per row"
    r"|per call|before|after|p50|p95|p99|lines)\b",
    re.IGNORECASE,
)

#: Written above a table to say the classifier is wrong about it.
EXCUSE = "<!-- not a measurement -->"

#: How far above a table the excuse may sit. Three lines allows a blank line
#: and a lead-in sentence between the two without letting one excuse silently
#: cover a second table further down.
EXCUSE_REACH = 3


def tables(text: str) -> list[tuple[int, list[str]]]:
    """Every markdown table outside a fenced block, as (1-based line, rows).

    Fenced blocks are skipped because this page pastes benchmark output
    verbatim, and pasted output is full of `|` characters that are not tables.
    """
    lines = text.splitlines()
    found: list[tuple[int, list[str]]] = []
    fenced = False
    i = 0
    while i < len(lines):
        if lines[i].startswith("```"):
            fenced = not fenced
            i += 1
            continue
        if (
            not fenced
            and lines[i].startswith("|")
            and i + 1 < len(lines)
            and SEPARATOR.match(lines[i + 1])
        ):
            start = i + 1
            body: list[str] = []
            while i < len(lines) and lines[i].startswith("|"):
                body.append(lines[i])
                i += 1
            found.append((start, body))
            continue
        i += 1
    return found


def is_measurement(body: Sequence[str]) -> bool:
    """Whether this table records something that was run.

    Deliberately over-eager. A table of line counts or a `before | after`
    column of plan names will trip it, and the cost of that is one
    `<!-- not a measurement -->` comment. The cost of the opposite error is a
    timing table entering the docs with no record of the build behind it,
    which is the nine-task defect this whole line of work came from — so when
    the two errors are not equally bad, the classifier leans toward the
    cheaper one.
    """
    return bool(IN_HEADER.search(body[0]) or any(IN_CELL.search(row) for row in body))


def stamped(lines: Sequence[str], start: int) -> bool:
    """Whether a `build:` line appears between the nearest heading and here.

    Scoped to the section rather than the whole file: one stamp at the top of
    `performance.md` must not vouch for a table eleven hundred lines below it,
    measured on a different day by a different binary. A heading is the
    boundary an author already thinks in.
    """
    for i in range(start - 2, -1, -1):
        line = lines[i]
        if line.startswith("#"):
            return False
        if "build: " in line:
            return True
    return False


def excused(lines: Sequence[str], start: int) -> bool:
    """Whether an author wrote the escape hatch directly above this table."""
    first = max(0, start - 1 - EXCUSE_REACH)
    return any(EXCUSE in line for line in lines[first : start - 1])


def digest(body: Sequence[str]) -> str:
    """A content fingerprint for the roster.

    Whitespace is collapsed so that a reflow or an alignment pass does not
    unfreeze forty tables, but the cell *values* are part of the key: editing
    a number in a legacy table takes it off the roster and asks for a stamp,
    which is right, because changing a recorded number means a new run
    happened and that run could say what built it.
    """
    joined = "\n".join(" ".join(row.split()) for row in body)
    return hashlib.sha256(joined.encode("utf-8")).hexdigest()[:16]


def load_roster() -> dict[str, str]:
    if not ROSTER.exists():
        return {}
    stored = json.loads(ROSTER.read_text(encoding="utf-8"))
    return {entry["digest"]: entry["file"] for entry in stored["frozen"]}


def check(docs: Path, roster: dict[str, str]) -> tuple[int, int, list[str]]:
    """Returns (measurement tables seen, frozen ones used, problems)."""
    seen = 0
    used: set[str] = set()
    wrong: list[str] = []
    for path in sorted(docs.glob("*.md")):
        text = path.read_text(encoding="utf-8")
        lines = text.splitlines()
        for start, body in tables(text):
            if not is_measurement(body):
                continue
            seen += 1
            if stamped(lines, start) or excused(lines, start):
                continue
            key = digest(body)
            if key in roster:
                used.add(key)
                continue
            wrong.append(
                f"docs/{path.name}:{start} records a measurement with no build "
                f"line in its section.\n"
                f"  Paste the `build:` line the run printed above the table, or "
                f"write `{EXCUSE}`\n"
                f"  above it if it is not one. If this is an edit to a table "
                f"that predates 2026-09-22,\n"
                f"  the edit is a new measurement and wants the line."
            )
    for key, name in sorted(roster.items(), key=lambda kv: (kv[1], kv[0])):
        if key not in used:
            wrong.append(
                f"the roster freezes a table in docs/{name} that is no longer "
                f"there unstamped ({key}).\n"
                f"  It was stamped, excused, edited or deleted — all good news. "
                f"Run `--freeze` to drop it."
            )
    return seen, len(used), wrong


def freeze(docs: Path) -> int:
    """Rewrite the roster from the tree as it stands.

    The hazard here is the `--no-verify` hazard: an escape hatch reached for
    reflexively stops being an escape hatch. It is mitigated by the roster
    being a checked-in file whose diff says exactly which tables were excused
    and by this printing the count, not by anything the script can enforce.
    """
    entries = []
    for path in sorted(docs.glob("*.md")):
        text = path.read_text(encoding="utf-8")
        lines = text.splitlines()
        for start, body in tables(text):
            if not is_measurement(body):
                continue
            if stamped(lines, start) or excused(lines, start):
                continue
            entries.append(
                {
                    "file": path.name,
                    "line_when_frozen": start,
                    "header": " ".join(body[0].split()),
                    "digest": digest(body),
                }
            )
    ROSTER.write_text(
        json.dumps(
            {
                "note": (
                    "Measurement tables in docs/ that predate the build stamp "
                    "(#280, 2026-09-22). The runs behind them are gone and a "
                    "build line cannot be reconstructed. Entries leave this "
                    "list when a table is re-measured and stamped; nothing "
                    "should be added to it. See scripts/check_table_provenance.py."
                ),
                "frozen": sorted(entries, key=lambda e: (e["file"], e["digest"])),
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"froze {len(entries)} unstamped measurement tables into {ROSTER.name}")
    return 0


def main(docs: Path = DOCS) -> int:
    roster = load_roster()
    seen, used, wrong = check(docs, roster)
    if not seen:
        # The never-fires half. A run that finds no measurement table at all
        # is looking in the wrong place, and every other guard here has been
        # bitten by exactly that.
        print(
            f"no measurement table found under {docs}/ at all, which is not "
            "possible for this repository — the guard is looking in the wrong "
            "place.",
            file=sys.stderr,
        )
        return 1
    for problem in wrong:
        print(problem, file=sys.stderr)
    if wrong:
        print(f"\n{len(wrong)} problem(s) of {seen} measurement tables", file=sys.stderr)
        return 1
    print(f"ok    {seen} measurement tables in docs/, {used} predating the stamp")
    return 0


if __name__ == "__main__":
    if "--freeze" in sys.argv:
        raise SystemExit(freeze(DOCS))
    raise SystemExit(main())
