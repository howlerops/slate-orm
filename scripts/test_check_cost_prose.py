#!/usr/bin/env python3
"""Tests for `check_cost_prose.py`, over trees this one writes.

Run against the real kernel — where every sentence is current — a mutation to
the pattern, the arithmetic or the exemption survives, because a correct tree
distinguishes none of them. That is the same finding two guards written this
week produced, so this one starts with fixtures and keeps the real tree as the
last case.

Run directly: `python3 scripts/test_check_cost_prose.py`.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_cost_prose as guard

#: The constants as the real file declares them: a read costs 1, a scanned row
#: an eight-thousandth, so the crossover is 8,000.
STATS = (
    "pub const SCAN_ROW_COST: f64 = 0.000_125;\n"
    "pub const POINT_READ_COST: f64 = 1.0;\n"
)


def run(prose: str, stats: str = STATS) -> tuple[int, list[str]]:
    """Run the real guard over one source file and one `stats.rs`."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        (root / "a.rs").write_text(prose)
        where = root / "stats.rs"
        where.write_text(stats)
        return guard.check(root, where)


#: name, the comment, how many claims it should hold, how many are stale.
CASES: list[tuple[str, str, int, int]] = [
    ("a current per-read claim is read and accepted",
     "// a point read costs 1 request.\n", 1, 0),
    ("the same claim in words is read too",
     "// a point read costs one request.\n", 1, 0),
    # The defect this exists for: eight files said three when it was one.
    ("a stale per-read claim is reported",
     "// a point read costs about three requests.\n", 1, 1),
    ("and the report names both figures",
     "// point reads cost about three.\n", 1, 1),
    ("a current crossover is accepted",
     "// it saves scanning eight thousand rows per row fetched.\n", 1, 0),
    # The derived figure, which moved by the same factor and in five files.
    ("a stale crossover in words is reported",
     "// it saves scanning some twenty-four thousand rows.\n", 1, 1),
    ("a stale crossover as an inequality is reported",
     "// fetching `k` rows beats scanning `n` only when `n > 24000k`.\n", 1, 1),
    ("and the current inequality is accepted",
     "// fetching `k` rows beats scanning `n` only when `n > 8000k`.\n", 1, 0),
    # `ledger/README.md` asks for a withdrawn figure to stay legible. A line
    # carrying one is history, not a claim about today, and the corrected
    # passages in `chain.rs` and `join.rs` depend on this.
    # Two lines, so this proves the skip is per *line* rather than the file
    # going unread: the struck-through figure is ignored and the current claim
    # beneath it is still counted. Without the exemption this is 2 claims and
    # 1 stale, which is what the corrected passages in `chain.rs` and
    # `join.rs` would report.
    ("a struck-through figure is history, not a stale claim",
     "// ~~a point read costs about three~~ requests.\n"
     "// a point read costs one request.\n", 1, 0),
    # Digits beside the word, which is how `planner.rs` writes the inequality
    # and how somebody would naturally write the prose form. A mutation that
    # read `8 thousand` as 8 survived until this case existed.
    ("a thousands figure written in digits is scaled",
     "// it saves scanning 8 thousand rows per row fetched.\n", 1, 0),
    ("and a stale one in digits is reported",
     "// it saves scanning 24 thousand rows per row fetched.\n", 1, 1),
    ("a comment with no figure in it is not a claim",
     "// a point read costs whatever the constant says.\n", 0, 0),
    # Both directions: the arithmetic has to come from the constants, not from
    # a number written here.
    ("the crossover follows the constants, not a literal",
     "// fetching `k` rows beats scanning `n` only when `n > 4000k`.\n", 1, 0),
]


def main() -> int:
    failed = 0
    for name, prose, claims, stale in CASES:
        # The last case redefines the constants so the crossover is 4,000.
        stats = STATS
        if "4000k" in prose:
            stats = (
                "pub const SCAN_ROW_COST: f64 = 0.000_250;\n"
                "pub const POINT_READ_COST: f64 = 1.0;\n"
            )
        # A crash is a named failure, not a dead suite: mutating a
        # never-fires guard away lets a missing key reach the arithmetic
        # below it, and an unguarded call here killed the run before its
        # summary line — which `mutate.py` can only report as "NOTHING RAN".
        try:
            seen, wrong = run(prose, stats)
        except Exception as raised:  # noqa: BLE001 - any crash is this failure
            seen, wrong = -1, [f"raised {raised!r}"]
        ok = seen == claims and len(wrong) == stale
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        wanted {claims} claim(s) and {stale} stale, "
                  f"got {seen} and {wrong}")

    # The never-fires halves, both of them. `check` returning `(0, [])` for a
    # tree with no claim is the right answer; the judgement that zero means
    # "looking in the wrong place" lives in `main`, so both are exercised.
    seen, wrong = run("// nothing about costs here.\n")
    ok = seen == 0 and not wrong
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a tree with no claim reads none")
    if not ok:
        print(f"        got {seen} and {wrong}")

    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        (root / "a.rs").write_text("// nothing about costs here.\n")
        stats = root / "stats.rs"
        stats.write_text(STATS)
        out = io.StringIO()
        with contextlib.redirect_stderr(out), contextlib.redirect_stdout(out):
            code = guard.main(root, stats)
    ok = code == 1 and "looking in the wrong place" in out.getvalue()
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  and a real run over it fails, rather than passing")
    if not ok:
        print(f"        exit {code}: {out.getvalue()}")

    # Same guard against a crash as the loop above, and it is needed here for
    # the same reason: without the `len(values) != 2` check a missing key
    # reaches the arithmetic and raises, which took the whole suite with it.
    try:
        _, wrong = run("// a point read costs 1.\n", "const SOMETHING_ELSE: u8 = 1;\n")
    except Exception as raised:  # noqa: BLE001 - a crash is this case failing
        wrong = [f"raised {raised!r} instead of reporting the missing constant"]
    ok = any("no longer declares both" in one for one in wrong)
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a stats.rs without the constants fails")
    if not ok:
        print(f"        got {wrong}")

    seen, wrong = guard.check()
    ok = seen > 0 and not wrong
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  every claim in the real kernel is current")
    for one in wrong:
        print(f"        {one}")

    print()
    print(f"{len(CASES) + 4 - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
