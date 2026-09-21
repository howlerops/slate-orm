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


def case(name: str, files: dict[str, str], seen: int, broken: int) -> bool:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        tree(root, files)
        got_seen, got_broken = guard.check(root)
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
            # Unchanged, and now the sharper statement of it: a ledger *file*
            # is still not searched, even though a ledger *path* is now a
            # citation when source names one. A dated record cites what was
            # true on its day, and rewriting that destroys what a ledger is.
            "a ledger entry's own citations are still out of scope",
            {
                "ledger/2026-01-01-a.py": (
                    "# docs/gone.md and ledger/2026-01-01-also-gone.md\n"
                )
            },
            seen=0,
            broken=0,
        ),
        case(
            "a document's own cross-links are out of scope",
            {"docs/a.py": "# docs/gone.md and ledger/2026-01-01-gone.md\n"},
            seen=0,
            broken=0,
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
