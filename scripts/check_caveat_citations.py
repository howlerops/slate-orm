#!/usr/bin/env python3
"""Every path a caveat verdict cites in `by` must resolve.

    python3 scripts/check_caveat_citations.py

`docs/caveat-status.json` records, per caveat, a verdict and a `by` naming
what closed or decided it. `scripts/caveats.py` verifies only that `by` is
*non-empty*, which
`ledger/2026-09-24-the-ledger-records-762-caveats-and-tracked-none-of-them.md`
wrote down as a caveat of its own:

  > **A verdict is a judgement, and nothing checks it.** `closed` must name
  > something, and that name is not verified to exist or to say what the
  > verdict claims — `check_cited_docs.py` would catch a dead path in a source
  > file but does not read this JSON. A wrong `closed` is invisible.

That caveat names the exact hole this fills: `check_cited_docs.py` reads
`.rs`, `.py`, `.go` and `.ts`, and this file is JSON, so no guard in this
directory had ever opened it.

# It was not hypothetical

Run against the file on the day it was written, this reported one defect
immediately: a verdict written four hours earlier cited
`scripts/check_unverifiable_claims.py`, a file that has never existed. The
guard it meant is `scripts/check_cost_prose.py`. That is the fourth
invented-citation of this kind in two days — four `ledger/…` filenames in one
session, then two `ledger/mutations/…` timestamps, then this — and the first
three were each caught by a *different* guard that happened to read the tree
the claim was in. Nothing read this one.

Two more were prose rather than defects: `ledger/2026-09-22 #285`, meaning the
2026-09-22 entry for task #285. It resolves for a person and not for a tool,
and it is now spelled as the filename, because a citation a tool cannot follow
is one nobody checks.

# What this checks, and what it cannot

It checks **existence**, exactly as `check_cited_docs.py` does, and inherits
that guard's limitation word for word: a `by` naming an entry that exists and
does not say what the verdict claims passes. Reading the entry and judging
whether it closed the caveat is the work the verdict *is*, and no pattern
does it.

It also cannot see a `by` that names nothing path-shaped. "this entry: the
alternative is guessing" cites the caveat's own paragraph, which is honest and
unfollowable; 38 of the verdicts written on 2026-09-26 are of that form and
every one of them is invisible here. That is recorded as a caveat rather than
fixed, because the alternative — requiring every `by` to name a file — would
turn an honest citation of the caveat itself into a fabricated file reference,
which is the failure this guard exists to catch.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STATUS = ROOT / "docs" / "caveat-status.json"

#: The trees a `by` may name. Rooted rather than free-form, so that a sentence
#: mentioning `site/` prose or a bare `foo.py` is not read as a citation — the
#: same narrowness `check_cited_docs.py` uses, and for the same reason: a
#: pattern wide enough to catch every careless reference also catches every
#: ordinary noun.
TREES = (
    "ledger",
    "crates",
    "scripts",
    "site",
    "clients",
    "docs",
    "examples",
    "proto",
    ".github",
)

#: A path-shaped run of characters beginning at one of `TREES`.
#:
#: The leading guard stops a word that merely *ends* in a tree name, and a
#: relative path climbing out of the repository, from matching. The trailing
#: `[\w/]` stops a sentence-final full stop or comma being eaten into the path,
#: which is how a citation ending a sentence would otherwise fail to resolve.
#: Both shapes have a case in `scripts/test_check_caveat_citations.py`, written
#: there rather than spelled here, because a docstring showing an invented
#: `ledger/…` path is itself a citation `check_cited_docs.py` will follow.
CITATION = re.compile(
    r"(?<![\w/.\-])((?:" + "|".join(re.escape(t) for t in TREES) + r")/[\w./\-]*[\w/])"
)


def citations(by: str) -> list[str]:
    """Every path-shaped citation in one `by` string, in order, deduplicated."""
    seen: list[str] = []
    for hit in CITATION.findall(by):
        if hit not in seen:
            seen.append(hit)
    return seen


def check(root: Path = ROOT) -> list[tuple[str, bool, str]]:
    """`(what, ok, detail)` per check, in the shape the other guards use."""
    out: list[tuple[str, bool, str]] = []

    def record(what: str, ok: bool, detail: str = "") -> None:
        out.append((what, ok, detail))

    path = root / "docs" / "caveat-status.json"
    if not path.is_file():
        record("docs/caveat-status.json exists", False, str(path))
        return out
    try:
        verdicts = json.loads(path.read_text(encoding="utf-8")).get("verdicts", [])
    except json.JSONDecodeError as exc:
        record("docs/caveat-status.json parses", False, str(exc))
        return out

    record("docs/caveat-status.json parses", True, f"{len(verdicts)} verdicts")

    dead: list[str] = []
    counted = 0
    for verdict in verdicts:
        by = verdict.get("by") or ""
        for cited in citations(by):
            counted += 1
            if not (root / cited).exists():
                dead.append(f"{verdict.get('entry', '?')}: {cited}")

    # The never-fires guard every check in this directory carries. A renamed
    # field, or a file whose verdicts all cite prose, finds nothing and reads
    # exactly like a file whose every citation resolves.
    record(
        "some verdict cites a path at all",
        counted > 0,
        f"{counted} citations across {len(verdicts)} verdicts",
    )
    record("every cited path resolves", not dead, "\n      ".join(dead))
    return out


def main() -> int:
    failed = 0
    for what, ok, detail in check():
        if ok:
            print(f"ok    {what}" + (f"  ({detail})" if detail else ""))
        else:
            failed += 1
            print(f"FAIL  {what}" + (f"\n      {detail}" if detail else ""))
    print()
    if failed:
        print(f"{failed} failed")
        return 1
    print("every path a caveat verdict cites can be opened")
    return 0


if __name__ == "__main__":
    sys.exit(main())
