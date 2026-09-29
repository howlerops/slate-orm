#!/usr/bin/env python3
"""Tests for `check_cited_files.py`, over a tree this one builds.

Run against the real `docs/`, every case would be "it passed" — the guard's
first run found nothing, which is the null result its docstring records. A rule
that had stopped checking would look identical. So the rules are exercised over
a miniature: one page, one repository, one entry in each roster.

Run directly: `python3 scripts/test_check_cited_files.py`.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_cited_files as guard

#: The fixture's own roster, replacing the real one. Patched rather than
#: reused: the real entry names a file this miniature has no page about, and a
#: test that grew a page per real entry would be tracking the roster rather
#: than the rules.
EXTERNAL = {"elsewhere/METHOD.md": "another project's, quoted in the review"}

PAGE = """\
# A page

It cites `crates/thing/src/lib.rs` exactly, `src/helper.rs` relative to a
crate, and `elsewhere/METHOD.md`, which is another project's.
"""


def tree(root: pathlib.Path, *, page: str = PAGE) -> None:
    (root / "docs").mkdir(parents=True)
    (root / "docs" / "page.md").write_text(page)
    (root / "crates" / "thing" / "src").mkdir(parents=True)
    (root / "crates" / "thing" / "src" / "lib.rs").write_text("// lib\n")
    (root / "crates" / "thing" / "src" / "helper.rs").write_text("// helper\n")
    commit(root)


def commit(root: pathlib.Path) -> None:
    # `tracked()` shells out to `git ls-files`, so the fixture has to be a repo.
    if not (root / ".git").exists():
        subprocess.run(["git", "init", "-q"], cwd=root, check=True, capture_output=True)
    subprocess.run(["git", "add", "-A"], cwd=root, check=True, capture_output=True)


def run(root: pathlib.Path) -> tuple[int, str]:
    guard.ROOT = root
    guard.EXTERNAL = dict(EXTERNAL)
    out, err = io.StringIO(), io.StringIO()
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = guard.main()
    except Exception as raised:  # noqa: BLE001 - any raise is a failing case
        return 70, f"{out.getvalue()}{err.getvalue()}the guard raised {raised!r}"
    return code, out.getvalue() + err.getvalue()


def clean(root: pathlib.Path) -> None:
    tree(root)


def a_cited_file_is_gone(root: pathlib.Path) -> None:
    tree(root)
    (root / "crates" / "thing" / "src" / "lib.rs").unlink()
    commit(root)


def a_relative_citation_is_gone(root: pathlib.Path) -> None:
    """The suffix arm, which the exact arm cannot reach.

    Written because the mutation dropping suffix resolution — resolving only
    exact paths — left `clean` failing on `src/helper.rs` and so was caught,
    while the mutation *widening* it was not: with only the exact arm removed
    nothing distinguished a relative citation that resolves from one that does
    not. This is that case.
    """
    tree(root)
    (root / "crates" / "thing" / "src" / "helper.rs").unlink()
    commit(root)


def a_cited_file_is_untracked(root: pathlib.Path) -> None:
    """On disk and not in the index, which is how a file arrives half-added.

    `tracked()` reads `git ls-files` rather than the filesystem, so a path a
    reader can open locally and nobody else can is a finding. The mutation
    reading the filesystem instead survives everything else here.
    """
    tree(root)
    (root / "crates" / "thing" / "src" / "lib.rs").write_text("// lib\n")
    subprocess.run(
        ["git", "rm", "-q", "--cached", "crates/thing/src/lib.rs"],
        cwd=root,
        check=True,
        capture_output=True,
    )


def an_external_entry_nothing_cites(root: pathlib.Path) -> None:
    tree(root, page="# A page\n\nIt cites `crates/thing/src/lib.rs` and nothing else.\n")


def an_external_entry_that_resolves_here(root: pathlib.Path) -> None:
    tree(root)
    (root / "elsewhere").mkdir()
    (root / "elsewhere" / "METHOD.md").write_text("# arrived\n")
    commit(root)


def the_page_cites_nothing(root: pathlib.Path) -> None:
    """The never-fires case: nothing found must not read as nothing wrong."""
    tree(root, page="# A page\n\nNo paths at all.\n")


CASES = [
    ("a page whose citations all resolve passes", clean, 0, "3 file paths cited"),
    (
        "a cited file that is gone fails",
        a_cited_file_is_gone,
        1,
        "`crates/thing/src/lib.rs`, which is not a file here",
    ),
    (
        "a crate-relative citation whose file is gone fails",
        a_relative_citation_is_gone,
        1,
        "`src/helper.rs`, which is not a file here",
    ),
    (
        "a cited file on disk but not in the index fails",
        a_cited_file_is_untracked,
        1,
        "`crates/thing/src/lib.rs`, which is not a file here",
    ),
    (
        "an EXTERNAL entry nothing cites any more fails",
        an_external_entry_nothing_cites,
        1,
        "nothing under docs/ cites it any more",
    ),
    (
        "an EXTERNAL entry that now resolves here fails",
        an_external_entry_that_resolves_here,
        1,
        "as another project's and it now resolves here",
    ),
    ("a page citing no file at all fails", the_page_cites_nothing, 1, "no file paths cited"),
]


def main() -> int:
    was = (guard.ROOT, guard.EXTERNAL)
    failures = []
    try:
        for name, build, wanted, needle in CASES:
            guard.ROOT, guard.EXTERNAL = was
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                try:
                    build(root)
                except Exception as raised:  # noqa: BLE001 - so is a bad fixture
                    code, output = 70, f"the fixture raised {raised!r}"
                else:
                    code, output = run(root)
            if code != wanted:
                failures.append(f"FAIL  {name}: exit {code}, wanted {wanted}\n{output}")
            elif needle not in output:
                failures.append(f"FAIL  {name}: no {needle!r} in\n{output}")
            else:
                print(f"ok    {name}")
    finally:
        guard.ROOT, guard.EXTERNAL = was

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
