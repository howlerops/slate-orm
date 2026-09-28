#!/usr/bin/env python3
"""`check_cited_docs.py`, over trees this file writes.

Against a written tree rather than against the repository, for the reason the
other guards here give: a check tested only by running it over `slate-orm` can
only assert that today's tree is clean, which is also what a check that does
nothing asserts.
"""

from __future__ import annotations

import contextlib
import io
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_cited_docs as guard


def tree(root: Path, files: dict[str, str]) -> None:
    for name, body in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")


def case(
    name: str,
    files: dict[str, str],
    seen: int,
    broken: int,
    roster: dict[tuple[str, str], str] | None = None,
    fixtures: dict[str, str] | None = None,
) -> bool:
    """One tree, one expectation. `roster` defaults to empty, not to the real one.

    `guard.NOT_A_FILE` names files in `slate-orm`, none of which a fixture
    tree has, so passing it here would make every case fail on nine stale-row
    reports about the repository. Empty is the fixture's own answer; the cases
    that are about the roster pass the rows they mean.
    """
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        tree(root, files)
        got_seen, got_broken = guard.check(root, roster or {}, fixtures or {})
        problems = []
        if got_seen != seen:
            problems.append(f"saw {got_seen} citations, expected {seen}")
        if len(got_broken) != broken:
            problems.append(f"{len(got_broken)} broken, expected {broken}: {got_broken}")
        if problems:
            print(f"FAIL  {name}")
            for problem in problems:
                print(f"        {problem}")
            return False
    print(f"ok    {name}")
    return True


def never_fires() -> bool:
    """A tree with no citation at all fails, rather than reading as clean.

    The guard is in `main`, not `check`, because `check` returning `(0, [])`
    is the correct answer for a tree that cites nothing — the judgement that
    zero means "looking in the wrong place" belongs one level up. Nothing
    exercised it until a mutation deleting it survived.
    """
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "src").mkdir()
        (root / "src/a.rs").write_text("// nothing cited here\n")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = guard.main(root)
    ok = code == 1 and "looking in the wrong place" in out.getvalue()
    print(f"{'ok  ' if ok else 'FAIL'}  a tree citing nothing fails, rather than passing")
    if not ok:
        print(f"        exit {code}: {out.getvalue()}")
    return ok


def the_tree() -> bool:
    """Every citation in this repository's own source resolves.

    The cases above are all fixtures, which is right — a guard whose only
    subject is a correct tree tests almost nothing. The converse is also
    true and was missing: with no real-tree case, a broken citation *here*
    was reported only by `check_cited_docs.py` itself, which prints no line
    `scripts/mutate.py` can read, so a mutation to a real citation could not
    be scored at all. Its two sibling guards both carry one of these.
    """
    seen, broken = guard.check(guard.ROOT)
    ok = seen > 0 and not broken
    print(f"{'ok  ' if ok else 'FAIL'}  every citation in this repository resolves")
    for problem in broken:
        print(f"        {problem}")
    if not seen:
        print("        no citation was found at all")
    return ok


def main() -> int:
    passed = [
        case(
            "a citation that resolves is counted and not reported",
            {"src/a.rs": "// see docs/x.md\n", "docs/x.md": "# x\n"},
            seen=1,
            broken=0,
        ),
        case(
            "a citation that does not resolve is reported",
            {"src/a.rs": "// see docs/gone.md\n"},
            seen=1,
            broken=1,
        ),
        case(
            "a broken citation in an error message is reported too",
            # The case this guard was written for: not a comment a reader may
            # never see, but a string the server hands to somebody who is
            # already confused.
            {"src/a.rs": 'Err("not supported. See docs/gone.md")\n'},
            seen=1,
            broken=1,
        ),
        case(
            "the workflow, the config, the page and the shell script are read too",
            # The residual of
            # `ledger/2026-09-26-the-citation-nobody-could-follow.md`: four
            # guards between them read `.rs`, `.py`, `.go`, `.ts` and prose, so
            # an entry named in CI's own workflow, in a `head.toml`, on a docs
            # page or in a shell script was invisible — "the eighth invented
            # citation will be there". All five are real places this repository
            # cites an entry from today.
            {
                "ci.yml": "# see ledger/gone.md\n",
                "head.toml": '# see docs/gone.md\n',
                "page.html": "<!-- docs/gone.md -->\n",
                "run.sh": "# docs/gone.md\n",
                "app.tsx": "// docs/gone.md\n",
            },
            seen=5,
            broken=5,
        ),
        case(
            "every source language is read, not only Rust",
            {
                "a.rs": "// docs/gone.md\n",
                "b.py": "# docs/gone.md\n",
                "c.go": "// docs/gone.md\n",
                "d.ts": "// docs/gone.md\n",
                # Not a source suffix, so not read. A Markdown file's links are
                # `site/check/docs.py`'s job.
                "e.md": "docs/gone.md\n",
            },
            seen=4,
            broken=4,
        ),
        case(
            "source citing a ledger entry that resolves is counted",
            # Added after `stats.rs` cited one and nothing would have caught a
            # typo in the name. Nine source files cite an entry, and the
            # failure this guard is about — a reader sent to a file that is
            # not there — does not care which directory it was in.
            {
                "src/a.rs": "// see ledger/2026-01-01-a-thing.md\n",
                "ledger/2026-01-01-a-thing.md": "# a thing\n",
            },
            seen=1,
            broken=0,
        ),
        case(
            "source citing a ledger entry that is gone is reported",
            {"src/a.rs": "// see ledger/2026-01-01-gone.md\n"},
            seen=1,
            broken=1,
        ),
        case(
            # Reversed. This said "a ledger entry's own citations are still out
            # of scope" and passed for as long as the exemption stood; the
            # exemption is what let two fabricated filenames sit in committed
            # entries with every guard printing `ok`.
            "a ledger entry citing an entry that is gone is reported",
            {
                "ledger/2026-01-01-a.md": (
                    "# a\n\ndocs/gone.md and ledger/2026-01-01-also-gone.md\n"
                )
            },
            seen=2,
            broken=2,
        ),
        case(
            "a document citing a document that is gone is reported",
            {"docs/a.md": "# a\n\ndocs/gone.md and ledger/2026-01-01-gone.md\n"},
            seen=2,
            broken=2,
        ),
        case(
            # The hole the two passes leave between them, stated rather than
            # hidden: `source_files` skips anything under `docs/` or `ledger/`
            # and `prose_files` reads only `.md`, so a script living in either
            # tree is read by neither. There is none today — scripts live in
            # `scripts/` — and the case exists so that the day one appears,
            # this line is what a reader finds.
            "a non-Markdown file under ledger/ is read by neither pass",
            {"ledger/2026-01-01-a.py": "# docs/gone.md\n"},
            seen=0,
            broken=0,
        ),
        case(
            "a rostered citation is neither counted nor reported",
            {"ledger/2026-01-01-a.md": "# a\n\ndocs/illustrative.md\n"},
            seen=0,
            broken=0,
            roster={("ledger/2026-01-01-a.md", "docs/illustrative.md"): "a worked example"},
        ),
        case(
            # Why the roster is keyed on the pair rather than on the file. An
            # entry with one illustrative path still has real citations, and a
            # whole-file exemption would stop checking them — which is how the
            # `docs/` and `ledger/` exemption this widening removed came to
            # hide two fabricated filenames.
            "the same citation in another file is still checked",
            {
                "ledger/2026-01-01-a.md": "# a\n\ndocs/illustrative.md\n",
                "ledger/2026-01-02-b.md": "# b\n\ndocs/illustrative.md\n",
            },
            seen=1,
            broken=1,
            roster={("ledger/2026-01-01-a.md", "docs/illustrative.md"): "a worked example"},
        ),
        case(
            "a rostered file's other citations are still checked",
            {"ledger/2026-01-01-a.md": "# a\n\ndocs/illustrative.md and docs/gone.md\n"},
            seen=1,
            broken=1,
            roster={("ledger/2026-01-01-a.md", "docs/illustrative.md"): "a worked example"},
        ),
        case(
            # The roster's never-fires half. Without it a row outlives the
            # sentence it was written for, and an exemption nobody reads is
            # the only way a fabricated citation gets through this guard.
            "a roster row whose citation is gone is reported",
            {"ledger/2026-01-01-a.md": "# a\n\ndocs/x.md\n", "docs/x.md": "# x\n"},
            seen=1,
            broken=1,
            roster={("ledger/2026-01-01-a.md", "docs/moved-on.md"): "was illustrative"},
        ),
        case(
            "vendored trees are skipped, because their docs are not ours",
            # Measured, not hypothetical: `playwright-core`'s type definitions
            # cite `docs/user_data_dir.md`, which this repository has no reason
            # to contain.
            {"web/node_modules/p/types.d.ts": "// docs/user_data_dir.md\n"},
            seen=0,
            broken=0,
        ),
        case(
            "generated and build trees are skipped too",
            {
                "target/debug/a.rs": "// docs/gone.md\n",
                "clients/typescript/dist/a.ts": "// docs/gone.md\n",
            },
            seen=0,
            broken=0,
        ),
        case(
            "a named fixture file is excluded, and only by name",
            {
                "scripts/test_check_cited_tests.py": "{'docs/d.md': 'x'}\n",
                # Same directory, not named: still checked, so the exclusion is
                # the file rather than the folder.
                "scripts/other.py": "# docs/gone.md\n",
            },
            seen=1,
            broken=1,
            fixtures={"scripts/test_check_cited_tests.py": "a fixture tree"},
        ),
        case(
            # `FIXTURES`' never-fires half. A whole-file exemption hides more
            # than a single-path one, so a row outliving its file is worse
            # here than in `NOT_A_FILE`.
            "a FIXTURES row for a file that is gone is reported",
            {"src/a.rs": "// docs/x.md\n", "docs/x.md": "# x\n"},
            seen=1,
            broken=1,
            fixtures={"scripts/vanished.py": "wrote a fixture tree once"},
        ),
        case(
            # The quieter half: the file is still there, still readable, and
            # has stopped containing anything this guard would have checked.
            # The reason still sounds right, which is what makes it dead.
            "a FIXTURES row for a file with no citation left is reported",
            {"scripts/quiet.py": "# nothing cited here\n", "src/a.rs": "// docs/x.md\n",
             "docs/x.md": "# x\n"},
            seen=1,
            broken=1,
            fixtures={"scripts/quiet.py": "used to write `docs/d.md`"},
        ),
        case(
            "a FIXTURES row for a file that still has one is clean",
            {"scripts/loud.py": "# docs/d.md\n", "src/a.rs": "// docs/x.md\n",
             "docs/x.md": "# x\n"},
            seen=1,
            broken=0,
            fixtures={"scripts/loud.py": "writes a tree containing `docs/d.md`"},
        ),
        case(
            "a path that is not a .md is not a citation",
            {"src/a.rs": "// docs/x.html and docs/ and docsomething.md\n"},
            seen=0,
            broken=0,
        ),
        case(
            "two citations on one line are both seen",
            # The regex is applied per line with `findall`, and a first version
            # that used `search` would have counted one and missed the other —
            # which reads as a clean line rather than a half-checked one.
            {"src/a.rs": "// docs/a.md and docs/b.md\n", "docs/a.md": "#\n"},
            seen=2,
            broken=1,
        ),
    ]
    passed.append(never_fires())
    passed.append(the_tree())
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
