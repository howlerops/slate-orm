#!/usr/bin/env python3
"""Tests for `check_cited_files.py`, over a tree this one builds.

Run against the real tree every case would be "it passed" — the guard is
green there, and a rule that had stopped checking would look identical. So the
rules are exercised over a miniature: one page, one repository, one entry in
each roster.

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


def a_path_inside_a_command_is_read(root: pathlib.Path) -> None:
    """A span is often a command, and its path is the one worth checking.

    `CLAUDE.md` writes `python3 scripts/reclaim.py`. A pattern anchored to the
    span's delimiters saw nothing there, and a mutation of that exact line
    survived — which is what put this case here.
    """
    tree(
        root,
        page=(
            "# A page\n\nRun `python3 crates/thing/src/gone.py`, and see "
            "`elsewhere/METHOD.md`, which is another project's.\n"
        ),
    )


def a_cwd_relative_invocation_is_not_a_citation(root: pathlib.Path) -> None:
    """`cd somewhere && ./run.sh` names a file relative to somewhere else.

    Resolving it from the root would report a file that is not missing. The
    real instance is in `site/docs/index.html`, and it was the one new match
    reading inside spans produced.
    """
    tree(
        root,
        page=(
            "# A page\n\nRun `cd crates/thing && ./run.sh`, cite "
            "`crates/thing/src/lib.rs`, and `elsewhere/METHOD.md`, which is "
            "another project's.\n"
        ),
    )


def a_location_names_a_file_that_is_gone(root: pathlib.Path) -> None:
    """`file.rs:12` is a citation of `file.rs`, and the file half is checkable."""
    tree(
        root,
        page=(
            "# A page\n\nSee `crates/thing/src/gone.rs:12` and "
            "`elsewhere/METHOD.md`, which is another project's.\n"
        ),
    )


def an_item_names_a_file_that_is_gone(root: pathlib.Path) -> None:
    """`file.rs::Symbol` is how `docs/security-review.md` names what it is about.

    One real file — `crates/slate-serverd/src/seed.rs` — is cited that way and
    no other, so before this the security review named a path nothing checked.
    """
    tree(
        root,
        page=(
            "# A page\n\nSee `crates/thing/src/gone.rs::analyze` and "
            "`elsewhere/METHOD.md`, which is another project's.\n"
        ),
    )


def a_path_inside_a_longer_word_is_not_a_citation(root: pathlib.Path) -> None:
    """A flag argument is one word, and it is not the path it contains.

    Matching a substring would store `--out=crates/thing/src/lib.rs` whole and
    report it as a file that is not there — a false positive on a path that
    resolves, which is the way a guard earns being ignored.
    """
    tree(
        root,
        page=(
            "# A page\n\nRun `build --out=crates/thing/src/lib.rs`, cite "
            "`crates/thing/src/lib.rs`, and `elsewhere/METHOD.md`, which is "
            "another project's.\n"
        ),
    )


def twin(root: pathlib.Path, text: str) -> None:
    """A second `src/helper.rs`, so the relative citation matches two files."""
    (root / "crates" / "other" / "src").mkdir(parents=True)
    (root / "crates" / "other" / "src" / "helper.rs").write_text(text)
    commit(root)


def a_relative_citation_matches_two_files_that_differ(root: pathlib.Path) -> None:
    """The case the guard could not see, and said so.

    Two `src/helper.rs` holding different code: the reader following
    `src/helper.rs` lands on whichever they guess, and before this the guard
    called that resolved.
    """
    tree(root)
    twin(root, "// a different helper\n")


def a_relative_citation_matches_two_identical_copies(root: pathlib.Path) -> None:
    """Two matches, one file's worth of content, so nothing is ambiguous.

    The real instance is a vendored `google/rpc/status.proto`, present once
    for the Rust server and once for the TypeScript client. Failing here
    would be the guard demanding a document disambiguate between two names
    for the same bytes.
    """
    tree(root)
    twin(root, "// helper\n")


def an_exact_path_is_also_the_tail_of_another(root: pathlib.Path) -> None:
    """`CLAUDE.md`'s `scripts/mutate.py`, in miniature.

    A path written from the root is the root's. Reading it as ambiguous
    because some package has a script by the same name would make every
    root-relative citation hostage to any file added below it.
    """
    tree(
        root,
        page=(
            "# A page\n\nIt cites `scripts/tool.py` and `elsewhere/METHOD.md`, "
            "which is another project's.\n"
        ),
    )
    (root / "scripts").mkdir()
    (root / "scripts" / "tool.py").write_text("# the root's\n")
    (root / "crates" / "thing" / "scripts").mkdir()
    (root / "crates" / "thing" / "scripts" / "tool.py").write_text("# a different one\n")
    commit(root)


def a_readme_outside_docs_is_read(root: pathlib.Path) -> None:
    """`SCOPE` past `docs/`: a broken path on the front page is still broken."""
    tree(
        root,
        page=(
            "# A page\n\nIt cites `crates/thing/src/lib.rs` and "
            "`elsewhere/METHOD.md`, which is another project's.\n"
        ),
    )
    (root / "README.md").write_text("Start at `crates/thing/src/gone.rs`.\n")
    commit(root)


def a_site_page_is_read(root: pathlib.Path) -> None:
    """`PAGES` past Markdown, and `CITED` past backticks.

    The site's HTML marks a path with `<code>`, which is the only way it marks
    one. A pattern that read backticks alone would have found nothing across
    ten pages and called `site/` covered.
    """
    tree(
        root,
        page=(
            "# A page\n\nIt cites `crates/thing/src/lib.rs` and "
            "`elsewhere/METHOD.md`, which is another project's.\n"
        ),
    )
    (root / "site").mkdir()
    (root / "site" / "index.html").write_text("<p>See <code>notes/gone.md</code>.</p>\n")
    commit(root)


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
        "nothing in scope cites it any more",
    ),
    (
        "an EXTERNAL entry that now resolves here fails",
        an_external_entry_that_resolves_here,
        1,
        "as another project's and it now resolves here",
    ),
    ("a page citing no file at all fails", the_page_cites_nothing, 1, "no file paths cited"),
    (
        "a relative citation matching two files that differ fails",
        a_relative_citation_matches_two_files_that_differ,
        1,
        "`src/helper.rs`, which is the tail of 2 files that differ",
    ),
    (
        "a relative citation matching two identical copies passes",
        a_relative_citation_matches_two_identical_copies,
        0,
        "3 file paths cited",
    ),
    (
        "an exact path that is also the tail of another file passes",
        an_exact_path_is_also_the_tail_of_another,
        0,
        "2 file paths cited",
    ),
    (
        "a broken citation in a README outside docs/ fails",
        a_readme_outside_docs_is_read,
        1,
        "README.md cites `crates/thing/src/gone.rs`",
    ),
    (
        "a broken citation in a site HTML page fails",
        a_site_page_is_read,
        1,
        "site/index.html cites `notes/gone.md`",
    ),
    (
        "a broken path inside a command inside a span fails",
        a_path_inside_a_command_is_read,
        1,
        "`crates/thing/src/gone.py`, which is not a file here",
    ),
    (
        "a cwd-relative ./ invocation is not read as a citation",
        a_cwd_relative_invocation_is_not_a_citation,
        0,
        "2 file paths cited",
    ),
    (
        "a file:line location whose file is gone fails",
        a_location_names_a_file_that_is_gone,
        1,
        "`crates/thing/src/gone.rs`, which is not a file here",
    ),
    (
        "a file::item citation whose file is gone fails",
        an_item_names_a_file_that_is_gone,
        1,
        "`crates/thing/src/gone.rs`, which is not a file here",
    ),
    (
        "a path inside a longer word is not read as a citation",
        a_path_inside_a_longer_word_is_not_a_citation,
        0,
        "2 file paths cited",
    ),
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
