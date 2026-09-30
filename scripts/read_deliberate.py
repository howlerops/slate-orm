#!/usr/bin/env python3
"""Assemble the evidence for reading `deliberate` caveats against the tree.

    python3 scripts/read_deliberate.py <draw-id> [start] [count]
    python3 scripts/read_deliberate.py --frame [start] [count]

# Why this exists

`scripts/caveats.py --unchecked` is a worklist of the `deliberate` verdicts
nobody has read against the tree. On 2026-09-30 it held 1013 rows, and five
campaigns had read 246 at roughly thirty a session — which
`ledger/2026-09-30-a-sample-that-pools-with-the-next-one.md` costed at
thirty-four more sessions and recorded as a frame rather than a plan.

Thirty a session was never a throughput limit. It was a *sampling* rate,
chosen so a pass could put a confidence interval on how many verdicts are
false. The cost per row is not reading the sentence, which takes seconds;
it is deciding what to check and then checking it. The entry that read the
last thirty says so in as many words: **"Twenty of the twenty-eight that held
were checked mechanically, not re-argued."**

So this does the mechanical half for every row at once, and leaves the
judgement — which is the part a script cannot have — to a person reading the
output. For each caveat it prints the claim, the verdict's own reasoning, and:

  * every backticked token in the claim, and whether it resolves in the tree
    as a tracked path or as a symbol `git grep` finds, with where;
  * whether the claim is a **negative existential** (`nothing`, `no`, `never`,
    `does not`), which is the shape that goes false when somebody later does
    the thing;
  * whether its subject is **outside this tree** — a registry, a GitHub
    setting, another project — which is the shape both false verdicts in the
    2026-09-30 draw turned out to have, and which `check_outside_premises.py`
    is the standing answer to;
  * how many ledger entries *later than this one* mention the claim's most
    distinctive words, which is where "true when written, overtaken later"
    leaves its trace.

# What it does not decide

Nothing here scores a caveat. A token that resolves says the claim's subject
exists, not that the claim about it holds; a later entry that shares words is
a place to look, not a refutation. The output is a reading aid, and the stamp
it leads to is still a claim about somebody's attention —
`ledger/2026-09-25-the-open-caveats-nobody-re-reads.md` measures that at right
about six times in seven, and this moves the cost of a read, not its accuracy.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import caveats

ROOT = Path(__file__).resolve().parent.parent

BACKTICK = re.compile(r"`([^`\n]{2,60})`")
#: A token worth resolving: a path, or an identifier a grep can find. A SQL
#: keyword or an English word in backticks is neither, and resolving it would
#: fill the output with `WHERE` found in four hundred files.
PATHY = re.compile(r"^[A-Za-z0-9_./-]+\.[a-z]{1,5}$")
SYMBOL = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*$")
#: Held in a name rather than inlined: `frozenset("…".split())` is SIM905,
#: and the list literal ruff offers instead is forty-five quoted strings on one
#: line. A name costs one line and keeps the words readable.
_KEYWORDS = """WHERE SELECT FROM JOIN GROUP ORDER LIMIT OFFSET HAVING DISTINCT UNION
    EXISTS LIKE ILIKE IN NOT NULL AND OR BY ON AS CHECK DEFAULT CASCADE
    RESTRICT EXPLAIN CONTAINS true false None open closed deliberate narrowed
    moment untriaged checked reviewed by residual draw n/a ok yes no"""
KEYWORDS = frozenset(_KEYWORDS.split())

#: The shape that goes false when somebody later does the thing. Both the
#: 216-row entry and the thirty-row entry asked for a tally of these.
NEGATIVE = re.compile(
    r"\b(nothing|no|none|never|cannot|can't|neither|not|un(?:checked|tested|read))\b",
    re.I,
)
#: A premise whose subject is not in this tree, which is the shape both false
#: verdicts of 2026-09-30 had. `scripts/check_outside_premises.py` is the
#: standing guard; this only flags the row for a reader.
OUTSIDE = re.compile(
    r"\b(github|npm|pypi|registry|crates\.io|pages|upstream|browser|"
    r"slatedb|clickhouse|postgres|docker|container|workflow|actions?)\b",
    re.I,
)

_STOP = """this that with from have here does not only they them their than then when
    what which were will been being into more most some such also very over under
    about after before each other because both same much many able cannot never
    nothing still would could should there these those where while whose thing
    things done make made take takes line lines file files test tests case cases
    name names read reads write writes work works need needs want wants know
    first second third whole part parts left right just like even well back down
    entry entries caveat caveats claim claims verdict verdicts ledger scripts
    says said tell tells find finds keep keeps look looks"""
STOP = frozenset(_STOP.split())
WORD = re.compile(r"[a-z_][a-z_0-9]{3,}")


def tracked(root: Path = ROOT) -> set[str]:
    out = subprocess.run(
        ["git", "ls-files"], cwd=root, capture_output=True, text=True, check=False
    )
    return set(out.stdout.split())


def resolve(token: str, files: set[str], root: Path) -> str:
    """Where `token` is in the tree, or why it was not looked for."""
    token = token.strip()
    if token in KEYWORDS or " " in token:
        return ""
    if PATHY.match(token):
        hits = [f for f in files if f == token or f.endswith("/" + token)]
        return f"path: {hits[0]}" if hits else "path: NOT IN THE TREE"
    if not SYMBOL.match(token) or len(token) < 4:
        return ""
    needle = token.split("::")[-1]
    out = subprocess.run(
        ["git", "grep", "-l", "-F", "-w", "--", needle],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    where = [line for line in out.stdout.split() if not line.startswith("ledger/")]
    if not where:
        return "symbol: NOWHERE outside ledger/"
    return f"symbol: {len(where)} files, e.g. {', '.join(sorted(where)[:2])}"


def plain(text: str) -> str:
    """Lowercased, with emphasis and code marks dropped and spaces collapsed."""
    return " ".join(text.replace("*", "").replace("`", "").lower().split())


def later_entries(claim: str, entry: str, bodies: dict[str, str]) -> list[str]:
    """Entries dated after `entry` that share the claim's distinctive words.

    Distinctive means the word appears in fewer than a sixth of all entries —
    a word every entry uses matches everything and says nothing.
    """
    words = {w for w in WORD.findall(claim.lower()) if w not in STOP}
    if not words:
        return []
    rare = {w for w in words if sum(w in b for b in bodies.values()) < len(bodies) // 6}
    if len(rare) < 2:
        return []
    # An entry that quotes the claim verbatim is an audit entry writing the
    # caveat down, not a later change that overtook it — and it matches every
    # rare word by construction, so it crowds out the entry that did. Measured:
    # of the two false verdicts this found on 2026-09-30, one had the entry
    # that closed it as its single hit and the other had that entry pushed off
    # the list by the audit entry quoting it.
    # Compared with emphasis and backticks removed from both sides: an audit
    # entry quoting a caveat in a table cell drops the `*` and the backticks,
    # so a literal comparison misses exactly the case this is for.
    quoted = plain(claim)[:60]
    hits = []
    for name, body in bodies.items():
        if name <= entry or quoted in plain(body):
            continue
        got = sum(w in body for w in rare)
        if got >= max(2, len(rare) * 0.6):
            hits.append((got, name))
    hits.sort(reverse=True)
    return [f"{n} ({g}/{len(rare)})" for g, n in hits[:SHOWN]]


#: How many candidates `later_entries` prints.
#:
#: A cut, not a threshold: everything below it scored well enough to be worth
#: looking at and is dropped anyway, because a reader given nineteen
#: suggestions reads none. `scripts/test_read_deliberate.py` measures what
#: that costs — on one labelled row the entry that actually closed the caveat
#: ranks 16th of 19 — and deliberately does not widen it, because tuning a
#: cut against a two-row labelled set is fitting the parameter to the test.
SHOWN = 3


def ranked(claim: str, entry: str, bodies: dict[str, str]) -> list[str]:
    """[`later_entries`] without the cut, best first.

    Exists for `scripts/test_read_deliberate.py`, which measures *where* in
    this list the answer falls rather than whether it survived `SHOWN` — a
    score that only asks "did it make the cut" cannot tell a signal that got
    worse from one whose answer moved from third to fourth, and the labelled
    set is far too small to spend that resolution.
    """
    words = {w for w in WORD.findall(claim.lower()) if w not in STOP}
    if not words:
        return []
    rare = {w for w in words if sum(w in b for b in bodies.values()) < len(bodies) // 6}
    if len(rare) < 2:
        return []
    quoted = plain(claim)[:60]
    hits = []
    for name, body in bodies.items():
        if name <= entry or quoted in plain(body):
            continue
        got = sum(w in body for w in rare)
        if got >= max(2, len(rare) * 0.6):
            hits.append((got, name))
    hits.sort(reverse=True)
    return [n for _, n in hits]


def main() -> int:
    argv = sys.argv[1:]
    if not argv:
        print(__doc__.splitlines()[2].strip())
        print(__doc__.splitlines()[3].strip())
        return 2

    status = caveats.load(ROOT)
    rows = {f"{c['entry']}::{caveats.key(c['claim'])}": c for c in caveats.caveats(ROOT)}
    if argv[0] == "--frame":
        keys, rest = caveats.frame(ROOT), argv[1:]
    else:
        import json

        record = json.loads((ROOT / caveats.DRAWS / f"{argv[0]}.json").read_text())
        keys, rest = record["keys"], argv[1:]
    start = int(rest[0]) if rest else 0
    count = int(rest[1]) if len(rest) > 1 else len(keys)
    keys = keys[start : start + count]

    files = tracked(ROOT)
    bodies = {
        p.name: p.read_text(encoding="utf-8", errors="replace").lower()
        for p in sorted((ROOT / "ledger").glob("*.md"))
        if p.name not in caveats.SKIP
    }

    for n, k in enumerate(keys, start=start):
        caveat = rows.get(k)
        if caveat is None:
            print(f"[{n}] {k}\n    ORPHANED: no such caveat any more\n")
            continue
        row = status.get(k, {})
        print(f"[{n}] {caveat['entry']}")
        print(f"    claim: {caveat['claim']}")
        print(f"    by:    {row.get('by', '')}")
        marks = []
        if NEGATIVE.search(caveat["claim"]):
            marks.append("negative-existential")
        if OUTSIDE.search(caveat["claim"]):
            marks.append("premise-outside-the-tree")
        if marks:
            print(f"    shape: {', '.join(marks)}")
        for token in dict.fromkeys(BACKTICK.findall(caveat["claim"])):
            found = resolve(token, files, ROOT)
            if found:
                print(f"    `{token}` -> {found}")
        after = later_entries(caveat["claim"], caveat["entry"], bodies)
        if after:
            print(f"    later: {'; '.join(after)}")
        print()
    print(f"{len(keys)} rows, {start}..{start + len(keys) - 1} of the list")
    return 0


if __name__ == "__main__":
    sys.exit(main())
