#!/usr/bin/env python3
"""Cases for `check_table_provenance.py`.

The suite this guard most resembles is `test_check_build_stamp.py`, and it
inherits that file's hard-won fixture rule: **the roster is always replaced,
never inherited**. #281 found that patching a module global *as well as*
passing a parameter made a fixture agree with a bug it should have exposed, so
every case here passes its roster in explicitly and the real one is never
consulted.
"""

from __future__ import annotations

import contextlib
import io
import json
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_table_provenance as guard

STAMP = (
    "build: slate-slatedb 0.0.1 | features aws, cache | off dhat-heap | "
    "release (opt-level 3, debug false) | x86_64-unknown-linux-gnu | "
    "slatedb 0.16.0, foyer 0.22.3, object_store 0.14.1, tokio 1.53.1"
)

TIMED = "| query | wall |\n|---|---:|\n| point get | 16 ms |\n"
#: The unit lives in the header and the cells are bare numbers — the shape
#: that cell-only matching missed, including the two scan-calibration tables.
HEADER_UNIT = "| | GETs | rows/GET |\n|---|---:|---:|\n| read nothing | 53 | 3,774 |\n"
#: No unit anywhere: a design table, which needs no provenance.
PLAIN = "| shape | answer | why |\n|---|---|---|\n| `WITH RECURSIVE` | Refused | a fixpoint |\n"


def run(body: str, roster: dict[str, str] | None = None) -> tuple[int, int, list[str]]:
    """One table, in a tree of its own.

    `readmes` is passed as `None`, never left to default. #288 widened this
    guard past `docs/` and gave the new parameter a live default; every case
    here then read the repository's own README tables on top of its one
    fixture table, and 13 of 14 failed at once.

    That default is still the right one, and this is the cheap half of the
    trade. Defaulting it to `None` instead would make a fixture that forgets
    silently correct and a *production* run that forgets silently check less —
    and neither guard's never-fires case would notice, because `docs/` alone
    always has tables. A loud failure in 13 fixtures beats a quiet hole in the
    real run.
    """
    with tempfile.TemporaryDirectory() as directory:
        docs = pathlib.Path(directory)
        (docs / "perf.md").write_text(body, encoding="utf-8")
        return guard.check(docs, roster or {}, None)


CASES: list[tuple[str, str, int, int]] = [
    ("a timed table with no build line is a problem", f"# H\n\n{TIMED}", 1, 1),
    ("the same table under a build line is fine", f"# H\n\n{STAMP}\n\n{TIMED}", 1, 0),
    (
        "the excuse comment accounts for it",
        f"# H\n\n{guard.EXCUSE}\n{TIMED}",
        1,
        0,
    ),
    # The section scope. A stamp cannot vouch across a heading, because one
    # line at the top of a 1,800-line page would otherwise cover every table
    # below it — measured on other days, by other binaries.
    (
        "a build line under an earlier heading does not vouch",
        f"# H\n\n{STAMP}\n\n## Later\n\n{TIMED}",
        1,
        1,
    ),
    # The false-negative that cell-only matching had.
    ("a header-borne unit is still a measurement", f"# H\n\n{HEADER_UNIT}", 1, 1),
    ("a table with no measurement in it is not asked for one", f"# H\n\n{PLAIN}", 0, 0),
    # Pasted benchmark output is full of `|` and is not a table.
    (
        "a table inside a fence is not read",
        f"# H\n\n```\n{TIMED}```\n",
        0,
        0,
    ),
    (
        "the excuse does not reach from too far above",
        f"# H\n\n{guard.EXCUSE}\n\n\n\n{TIMED}",
        1,
        1,
    ),
]


def main() -> int:
    ran = 0
    failed = 0
    for name, body, seen_want, problems in CASES:
        try:
            seen, _, wrong = run(body)
        except Exception as raised:  # noqa: BLE001 - a crash is this failure
            seen, wrong = -1, [f"raised {raised!r}"]
        ok = seen == seen_want and len(wrong) == problems
        ran += 1
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        wanted {seen_want} seen and {problems} problem(s), "
                  f"got {seen} and {wrong}")

    # Freezing. A roster entry accounts for a table, and stops accounting for
    # it the moment a number in that table changes — which is the whole reason
    # the digest covers contents rather than position.
    frozen = guard.digest(TIMED.splitlines())
    _, used, wrong = run(f"# H\n\n{TIMED}", {frozen: "perf.md"})
    ok = used == 1 and not wrong
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a frozen table is accounted for")
    if not ok:
        print(f"        got used={used} {wrong}")

    edited = TIMED.replace("16 ms", "17 ms")
    _, used, wrong = run(f"# H\n\n{edited}", {frozen: "perf.md"})
    ok = used == 0 and len(wrong) == 2  # the edited table, and the stale entry
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  editing a frozen table's number unfreezes it")
    if not ok:
        print(f"        got used={used} {wrong}")

    # Reflow must not unfreeze: the digest collapses whitespace on purpose,
    # so an alignment pass over forty tables is not forty findings.
    reflowed = "|  query  |  wall  |\n|---|---:|\n|  point get  |  16 ms  |\n"
    _, used, wrong = run(f"# H\n\n{reflowed}", {frozen: "perf.md"})
    ok = used == 1 and not wrong
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  but realigning its columns does not")
    if not ok:
        print(f"        got used={used} {wrong}")

    _, _, wrong = run(f"# H\n\n{STAMP}\n\n{TIMED}", {"deadbeefdeadbeef": "perf.md"})
    ok = len(wrong) == 1 and "no longer there" in wrong[0]
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a stale roster entry is reported")
    if not ok:
        print(f"        got {wrong}")

    # The never-fires half, run through `main` so the judgement is exercised
    # and not just the counting under it.
    with tempfile.TemporaryDirectory() as directory:
        docs = pathlib.Path(directory)
        (docs / "a.md").write_text(PLAIN, encoding="utf-8")
        out = io.StringIO()
        with contextlib.redirect_stderr(out), contextlib.redirect_stdout(out):
            code = guard.main(docs, None)
    ok = code == 1 and "looking in the wrong place" in out.getvalue()
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a tree with no measurement fails, not passes")
    if not ok:
        print(f"        exit {code}: {out.getvalue()}")

    # `freeze` writes the roster the rest of this depends on, and had no test
    # at all until #288 — three mutations to it survived: the page sweep, the
    # naming, and the duplicate rule.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        docs = root / "docs"
        docs.mkdir()
        (docs / "perf.md").write_text(TIMED, encoding="utf-8")
        (root / "README.md").write_text(HEADER_UNIT, encoding="utf-8")
        out = root / "roster.json"
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            guard.freeze(docs, root, out)
        frozen = json.loads(out.read_text())["frozen"]
    files = sorted(one["file"] for one in frozen)
    ok = files == ["README.md", "perf.md"]
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  freeze sweeps the READMEs too, not only docs/")
    if not ok:
        print(f"        froze {files}")

    # The same body in two files is one entry: the roster answers "does this
    # table predate the stamp", and that is a property of the table.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        docs = root / "docs"
        docs.mkdir()
        (docs / "perf.md").write_text(TIMED, encoding="utf-8")
        (root / "README.md").write_text(TIMED, encoding="utf-8")
        out = root / "roster.json"
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            guard.freeze(docs, root, out)
        frozen = json.loads(out.read_text())["frozen"]
    ok = len(frozen) == 1
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  one table body in two files is frozen once")
    if not ok:
        print(f"        froze {len(frozen)}: {[one['file'] for one in frozen]}")

    # A page under the repository root is named by its path, not its basename:
    # once two trees are read, `performance.md` stops being unique.
    named = guard.shown(guard.ROOT / "docs" / "performance.md")
    ok = named == "docs/performance.md"
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a page is named by its path from the root")
    if not ok:
        print(f"        got {named!r}")

    # And the real tree, which is what CI actually runs.
    out = io.StringIO()
    with contextlib.redirect_stderr(out), contextlib.redirect_stdout(out):
        code = guard.main()
    ok = code == 0
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  the real docs/ tree is accounted for")
    if not ok:
        print(f"        exit {code}: {out.getvalue()}")

    # Counted, not asserted. This was `len(CASES) + 6`, a hand-kept constant
    # one task after #268 found the same shape in the conformance runner: it
    # cannot notice a case that stopped running, and it silently under-reported
    # the three cases #288 added right beside it.
    print(f"\n{ran - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
