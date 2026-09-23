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
    """Run the real guard over one source file and one `stats.rs`.

    `docs` and `readmes` are both passed as `None`, never left to default.
    #283 gave `check` a docs tree with a real default, and a fixture that let
    it default would read the repository's own `docs/` alongside its one-line
    temporary file — every case would then carry 24 extra claims it did not
    write, which is the fixture-reaching-past-itself failure #281 was about.

    #288 added `readmes` and proved the point by forgetting it: the parameter
    landed with a real default, the fixtures still passed three positional
    arguments, and 36 of 39 cases failed at once — each one carrying the
    repository's own `README.md` claim on top of its fixture. The case named
    "a tree with no claim reads none" is what says so out loud, and it is
    worth keeping for exactly this.
    """
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        (root / "a.rs").write_text(prose)
        where = root / "stats.rs"
        where.write_text(stats)
        return guard.check(root, where, None, None)


def run_markdown(page: str, stats: str = STATS) -> tuple[int, list[str]]:
    """The same, over one `docs/*.md` page and no Rust at all."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        docs = root / "docs"
        docs.mkdir()
        (docs / "perf.md").write_text(page)
        where = root / "stats.rs"
        where.write_text(stats)
        return guard.check(root, where, docs, None)


def run_readme(
    page: str, at: str = "README.md", stats: str = STATS
) -> tuple[int, list[str]]:
    """The same, over a README outside `docs/` and no docs tree at all.

    `at` places the file, so one case can cover `clients/*/README.md` rather
    than leaving the glob tuple's first entry as the only one exercised. That
    is not hypothetical: with no case here at all, shrinking `README_GLOBS` to
    just `("README.md",)` survived mutation.
    """
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        target = root / at
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(page)
        where = root / "stats.rs"
        where.write_text(stats)
        return guard.check(root, where, None, root)


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
    # Two lines, so this proves the exemption is per *span* rather than the
    # file going unread: the struck-through figure is ignored and the current
    # claim beneath it is still counted. Without the exemption this is 2
    # claims and 1 stale, which is what the corrected passages in `chain.rs`
    # and `join.rs` would report. It was per line until #279 joined a comment
    # run into one chunk; per span is strictly finer, and `stats.rs` needs it
    # — it strikes two sentences through and states the current figure in the
    # third, all in one paragraph.
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
    # #279. A claim that wraps across a doc-comment line break. `rustfmt` and
    # a 100-column margin produce this constantly, and reading line by line
    # sees neither half: "costs about" has no figure, "**three requests**"
    # has no subject. This is how the stale bullet in `cost_at_scale`'s
    # module doc survived #278's widening, which was aimed at that very file.
    ("a claim wrapped across a comment line break is one claim",
     "//! - `POINT_READ_COST`, which is the claim that a point read costs\n"
     "//!   about **three requests**.\n", 1, 1),
    ("and the same claim, current, is accepted",
     "//! a point read costs\n//!   about **one request**.\n", 1, 0),
    # Two comment runs with code between them do not merge: joining those
    # would invent a sentence nobody wrote, and the figure would be scored
    # against the wrong subject.
    ("a run is broken by a line of code",
     "// a point read costs one request.\nlet x = 1;\n"
     "// it saves scanning eight thousand rows.\n", 2, 0),
    # #279. The form that actually went stale. `cost_at_scale` printed
    # `POINT_READ_COST = 3.0` in its header for nine tasks; the English
    # clause beside it is what the guard caught, and a restatement with no
    # clause — a bullet, a header, a table cell — was invisible.
    ("a literal restatement of a constant is a claim",
     "// - `POINT_READ_COST = 3.0`, measured at 200,000 rows.\n", 1, 1),
    ("and a current restatement is accepted",
     "// - `SCAN_ROW_COST = 0.000125`, measured at 200,000 rows.\n", 1, 0),
    # The declaration is the one copy that is not a restatement, and what
    # separates it is the type annotation: `NAME: f64 = 1.0` against the
    # `NAME = 1.0` the pattern wants. An explicit `pub const` skip stood here
    # for an hour and a mutation deleting it survived, because the pattern
    # had already excluded the form. This case now covers the real mechanism.
    ("the declaration in stats.rs is not a restatement",
     "pub const POINT_READ_COST: f64 = 1.0;\n", 0, 0),
    # A joined run flushed by the *code* after it, not by end of file. Every
    # other fixture here ends on a comment, which exercised only the second
    # of two flush paths — a mutation to the first survived all of them.
    ("a wrapped claim is joined when code follows it",
     "// a point read costs\n// about three requests.\nlet x = 1;\n", 1, 1),
    # #279. The third derived figure: 1 / SCAN_ROW_COST. Nothing checked it,
    # and `stats.rs` records that this constant has open headroom (#277), so
    # it is the next one that will move under prose that quotes it.
    ("a rows-per-request claim is read against SCAN_ROW_COST",
     "// a scan returns about eight thousand rows per request.\n", 1, 0),
    ("a stale rows-per-request claim is reported",
     "// a scan returns about 100 rows per object-store request.\n", 1, 1),
    # #281. A measured *total* is not a per-read claim. `join.rs` and
    # `chain.rs` both say "four hundred point reads cost 1,221 requests";
    # this read that as "a point read costs 1" and passed only because 1,221
    # begins with a 1. Mutate the total and the guard accuses two correct
    # files of saying a point read costs 2.
    ("a measured total is not a per-read claim",
     "// four hundred point reads cost 1,221 requests.\n", 0, 0),
    ("and neither is one whose first digit happens to be wrong",
     "// four hundred point reads cost 2,442 requests.\n", 0, 0),
    # #281. A ratio is a third kind of sentence. `stamp.rs` says a cacheless
    # build "costs ~3x the object-store requests"; dropping the word boundary
    # in front of the figure makes that a claim that a point read costs 3.
    ("a ratio is not a per-read claim",
     "// without it, point reads cost ~3x the object-store requests.\n", 0, 0),
    # And the plain forms still read, so neither guard above ate them.
    ("a bare per-read claim still reads",
     "// a point read costs 1 request.\n", 1, 0),
    ("a comment with no figure in it is not a claim",
     "// a point read costs whatever the constant says.\n", 0, 0),
    # Both directions: the arithmetic has to come from the constants, not from
    # a number written here.
    ("the crossover follows the constants, not a literal",
     "// fetching `k` rows beats scanning `n` only when `n > 4000k`.\n", 1, 0),
]


def reports_the_range_of_a_joined_run() -> tuple[bool, str]:
    """A violation in a doc-comment run names the lines, not just the first.

    #281. A module doc becomes one chunk once its lines are joined, so every
    violation in it was reported at the run's first line — `file:1` for a
    `//!` block, which reads as a guard that cannot locate anything. The
    stale figure here is on the *second* line of a two-line run, so a report
    naming only line 1 is visibly not pointing at it.
    """
    _, wrong = run("//! a point read\n//!   costs about three requests.\n")
    if len(wrong) != 1:
        return False, f"wanted one problem, got {wrong}"
    if ":1-2 " not in wrong[0]:
        return False, f"wanted the range 1-2 in {wrong[0]!r}"
    return True, ""


def main() -> int:
    failed = 0
    ran = 0
    for name, prose, claims, stale in CASES:
        ran += 1
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

    # #283. The markdown half: a claim in `docs/` is read, wrapping is joined
    # the way a doc comment's is, and the three kinds of prose that trip these
    # patterns without claiming anything current are each let through.
    MARKDOWN: list[tuple[str, str, int, int]] = [
        ("a current claim in a doc is read",
         "A point read costs 1 request.\n", 1, 0),
        ("a stale claim in a doc is reported",
         "A point read costs about three requests.\n", 1, 1),
        # The reason line-by-line matching is not enough, in the tree where
        # every sentence wraps at 80 columns.
        ("a claim wrapped across two lines is still one claim",
         "A point read\ncosts about three requests.\n", 1, 1),
        # Pasted benchmark output is a record of what a run printed.
        ("a figure inside a fence is not a claim",
         "```\na point read costs about three requests\n```\n", 0, 0),
        ("the marker excuses the paragraph it sits in",
         f"{guard.NOT_A_CLAIM}\nA point read costs about three requests.\n", 0, 0),
        ("and the paragraph after it, when it stands alone",
         f"{guard.NOT_A_CLAIM}\n\nA point read costs about three requests.\n", 0, 0),
        # But not the one after that: an excuse that runs to the end of the
        # file would quietly cover every claim below it.
        ("but not the one after that",
         f"{guard.NOT_A_CLAIM}\n\nHistory.\n\nA point read costs three.\n", 1, 1),
        # Strikethrough still works in markdown, and is preferred where a
        # paragraph states the current figure beside the old one — it keeps
        # the live claim under the guard, which the marker cannot.
        ("a struck figure beside a live one keeps the live one checked",
         "It is now 1 request, rather than ~~3 requests~~. "
         "A point read costs 1 request.\n", 1, 0),
    ]
    for name, page, claims, stale in MARKDOWN:
        ran += 1
        try:
            seen, wrong = run_markdown(page)
        except Exception as raised:  # noqa: BLE001 - a crash is this failure
            seen, wrong = -1, [f"raised {raised!r}"]
        ok = seen == claims and len(wrong) == stale
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        wanted {claims} claim(s) and {stale} stale, "
                  f"got {seen} and {wrong}")

    ok, why = reports_the_range_of_a_joined_run()
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a joined run reports the lines it spans")
    if not ok:
        print(f"        {why}")

    # The never-fires halves, both of them. `check` returning `(0, [])` for a
    # tree with no claim is the right answer; the judgement that zero means
    # "looking in the wrong place" lives in `main`, so both are exercised.
    seen, wrong = run("// nothing about costs here.\n")
    ok = seen == 0 and not wrong
    ran += 1
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
            code = guard.main(root, stats, None, None)
    ok = code == 1 and "looking in the wrong place" in out.getvalue()
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  and a real run over it fails, rather than passing")
    if not ok:
        print(f"        exit {code}: {out.getvalue()}")

    # An error-claim about a constant is refused, not verified (#291).
    for name, page, want_seen, want_wrong in [
        ("an error-claim about a constant is refused",
         "`POINT_READ_COST` is 13-19% low at 200k.\n", 0, 1),
        ("an en dash in the range reads the same",
         "`POINT_READ_COST` is 13\u201319% low at 200k.\n", 0, 1),
        ("so does a single figure, and the other constant",
         "SCAN_ROW_COST sits 20% high on this fixture.\n", 0, 1),
        # The escape hatch, which is the point: the class becomes checkable
        # by construction or explicitly exempt, never silently unchecked.
        ("a marked one is history and passes",
         "<!-- not a cost-model claim -->\n\n"
         "`POINT_READ_COST` was 13-19% low on a build with no cache.\n", 0, 0),
        ("a struck one passes too, like any other history",
         "~~`POINT_READ_COST` is 13-19% low at 200k.~~\n", 0, 0),
        # Not this pattern's business: a measurement that happens to be a
        # percentage, which is most of this repository's prose.
        ("a percentage that is not about a constant is left alone",
         "A 5,000-row `update_many` came out ~7% off.\n", 0, 0),
        # The `[^.\n]` in the pattern is what stops this: a constant stated
        # checkably in one sentence and an unrelated percentage in the next
        # must read as one claim and no refusal, not as an error-claim
        # spanning the full stop.
        ("a full stop separates a real claim from a later percentage",
         "`POINT_READ_COST = 1.0`. Some other thing is 40% off.\n", 1, 0),
    ]:
        seen, wrong = run_markdown(page)
        ok = seen == want_seen and len(wrong) == want_wrong
        ran += 1
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        wanted {want_seen} seen and {want_wrong} refused, "
                  f"got {seen} and {wrong}")

    # Counting the refusal is not enough: dropping the en dash from the range
    # still fires, on the `19%` alone, and only the text the message quotes
    # back changes. A message that misquotes the sentence it is complaining
    # about sends the author looking for something they did not write.
    _, wrong = run_markdown("`POINT_READ_COST` is 13\u201319% low at 200k.\n")
    ok = len(wrong) == 1 and "13\u201319%" in wrong[0]
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  the refusal quotes the whole range it saw")
    if not ok:
        print(f"        got {wrong}")

    # The README tree (#288). Without these the whole `readmes` path was
    # dead weight as far as the suite was concerned: disabling it outright,
    # and dropping `main`'s threading of it, both survived mutation.
    for name, page, at, want_seen, want_stale in [
        ("a current claim in the root README is read",
         "A point read costs 1 request.\n", "README.md", 1, 0),
        ("a stale one there is reported",
         "A point read costs three requests.\n", "README.md", 1, 1),
        ("and a client README is read too, not just the root",
         "A point read costs three requests.\n", "clients/go/README.md", 1, 1),
        ("a struck claim in a README is history, as in docs/",
         "~~A point read costs three requests.~~\n", "README.md", 0, 0),
    ]:
        seen, wrong = run_readme(page, at)
        ok = seen == want_seen and len(wrong) == want_stale
        ran += 1
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        wanted {want_seen} claim(s) and {want_stale} stale, "
                  f"got {seen} and {wrong}")

    # `main` must actually hand the tree on. It did not, for one commit.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        (root / "README.md").write_text("A point read costs three requests.\n")
        stats = root / "stats.rs"
        stats.write_text(STATS)
        out = io.StringIO()
        with contextlib.redirect_stderr(out), contextlib.redirect_stdout(out):
            code = guard.main(root, stats, None, root)
    ok = code == 1 and "costs three" in out.getvalue()
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a stale README fails a real run, through main")
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
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  a stats.rs without the constants fails")
    if not ok:
        print(f"        got {wrong}")

    seen, wrong = guard.check()
    ok = seen > 0 and not wrong
    ran += 1
    failed += not ok
    print(f"{'ok  ' if ok else 'FAIL'}  every claim in the real crates is current")
    for one in wrong:
        print(f"        {one}")

    print()
    print(f"{ran - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
