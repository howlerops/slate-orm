#!/usr/bin/env python3
"""Every file the docs, the site and the READMEs point at must be a file.

`scripts/check_cited_tests.py` checks the *names* a document points at:
`docs/security-review.md` spent a week naming four probes that no longer
existed, each inverted by the commit that fixed the finding it demonstrated,
and nothing compiles a document. This is the same failure one level up. A
document that says "see `crates/slate-kernel/tests/oracle.rs`" after that file
is renamed is as useless as one naming a dead function, and until this guard
nothing in the repository looked at a cited *path* in `docs/` at all —
`check_cited_docs.py` runs the other way, from source to `docs/`, and
`check_cited_tests.py` matches snake_case identifiers, which a path is not.

Found by mutation, and the null result is worth as much as the guard: of 109
distinct paths cited across `docs/`, 108 resolve and the one that does not is
another project's. So this catches nothing today and is here for the same
reason `check_cited_tests.py` is — the class has a week-long instance on
record, in this repository, in a security review.

**A citation may be written relative to a crate.** `docs/performance.md` says
`examples/replicas.rs`, meaning `crates/slate-slatedb/examples/replicas.rs`,
and `docs/correctness.md` says `tests/rls_join.rs` for
`crates/slate-server/tests/rls_join.rs`. Twenty of the hundred and nine are
written that way and all of them read correctly in context, so a path resolves
when it is a **suffix** of a tracked file's path. That is weaker than an exact
match and it is the honest rule: what this catches is a file that is not there
at all, which is the failure that happened.

**And a citation may belong to another repository.** `docs/clickbench.md`
reviews `pgrust` and quotes the file describing that project's benchmark
methodology. `EXTERNAL` names those, one line each saying whose they are, and
an entry that stops being cited — or that starts resolving here — is itself a
failure, because a roster nobody is forced to edit is a roster that goes stale.

Run directly: `python3 scripts/check_cited_files.py`.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Where a path is read as a live claim.
#:
#: `docs/` was the whole of it, and the entry that added this guard recorded
#: why that is not enough: *"`README.md`, `CLAUDE.md` and `site/` cite paths
#: too, and `site/check/docs.py` checks the site's links but not its file
#: citations."* A path named on the front page points a reader at nothing just
#: as squarely as one named in a design note, and the README is the document
#: most people read.
#:
#: `ledger/` stays out, by principle rather than by cost, for the reason
#: `check_cited_tests.py`'s docstring sets out at length: an entry is a dated
#: record, and a path that has since moved is provenance rather than a broken
#: link. Correcting one would be rewriting history to make a check pass.
#:
#: Directories and single files both, because two of the four are files.
SCOPE: tuple[str, ...] = ("docs", "site", "README.md", "CLAUDE.md")

#: What a document is, under a directory in `SCOPE`.
#:
#: The site is HTML and the rest is Markdown. `site/` also holds generated
#: JavaScript and JSON that quote paths for their own reasons — a bundler's
#: source map is not a claim about this repository — so the extensions are
#: listed rather than every file being read.
PAGES = ("*.md", "*.html")

#: A path: at least one slash, and an extension this repository writes. The
#: extension list is what keeps `google/rpc` and `tokio/net` out — a module
#: path is not a file path, and flagging one would be the 87% false positive
#: rate `check_cited_tests.py` measured and rejected.
#:
#: A leading `./` or `../` is excluded because it is not a path *here*: the
#: only one in the tree is `./run.sh`, in `cd examples/explorer && ./run.sh`,
#: which names a file relative to a directory the reader was just told to
#: change to. Resolving it from the root would report a file that is not
#: missing. It was the single new match this pattern found when it started
#: reading inside code spans, which is the measurement that decided the rule.
PATH = (
    r"(?!\.{1,2}/)[A-Za-z0-9_.\-]+(?:/[A-Za-z0-9_.\-]+)+"
    r"\.(?:rs|py|ts|tsx|go|toml|json|sh|md|proto|yml|yaml|css|html)"
)

#: How a document marks something as code. Markdown backticks, and the
#: `<code>` the site's hand-written HTML uses — which is the whole of how
#: `site/docs/*.html` cites anything, so a backtick-only pattern would have
#: read the ten site pages and found nothing while reporting that `site/` was
#: in scope. That is the check-that-never-fires shape this repository keeps
#: meeting, arriving this time inside the change that widened the scope.
SPAN = re.compile(r"`([^`\n]+)`|<code>([^<]+)</code>")

#: A whole word inside a span, because a span is often a command rather than
#: a path. `python3 scripts/reclaim.py` is how `CLAUDE.md` names the script it
#: tells you to run, and a pattern anchored to the delimiters could not see
#: it: a mutation breaking that exact line **survived**, which is how this was
#: found rather than argued. The path most worth checking is the one in a
#: command, because a command naming a script that moved does not run.
#:
#: Whole-word, so `head.toml` in `slate-serverd --config head.toml` still does
#: not match (no slash) and a `path.rs:12` with a line number does not either.
#: Measured before it was adopted: reading inside spans is a strict superset
#: of reading only whole spans, and over the tree today it adds one match.
CITED = re.compile(PATH)

#: A place within a file, written on the end of the path. Two forms are used
#: in scope: a line number, because the terminal makes `file.rs:12` clickable
#: (`crates/slate-derive/src/lib.rs:629`,
#: `crates/slate-sql/tests/front_end.rs:185`), and `::Symbol`, which
#: `docs/security-review.md` uses to name the item it is about
#: (`crates/slate-serverd/src/seed.rs::analyze` and two more).
#:
#: All five were invisible until the place was stripped before matching, and
#: `crates/slate-serverd/src/seed.rs` was cited *only* that way — so the
#: security review named a file nothing checked. The file is the checkable
#: half; a line number and an item name are not checkable here at all, and the
#: line is the part that goes stale fastest.
AT_PLACE = re.compile(rf"({PATH})(?::\d+|::[A-Za-z_][A-Za-z0-9_]*)\Z")

#: Paths that belong to another project, with whose they are. Each must still
#: be cited somewhere in scope, and must still not resolve here; both are
#: checked below, so this list cannot quietly outlive its reason.
EXTERNAL: dict[str, str] = {
    "benchmarks/README.md": (
        "pgrust's. docs/clickbench.md reviews that project and quotes the file "
        "describing its own benchmark methodology, which is the thing being "
        "assessed — there is no copy here and there should not be one."
    ),
}


def tracked() -> dict[str, list[str]]:
    """Every suffix of every indexed path, and which files it could be.

    A `list` rather than a `set` because the count is the point. A citation
    written relative to a crate — `src/lib.rs`, `tests/oracle.rs` — resolves
    by suffix, and a suffix matching **two** files resolves to neither: the
    reader is sent to whichever they guess. The guard that added this could
    not see that case, and its entry said so: *"`tests/oracle.rs` would match
    any `tests/oracle.rs` in the tree, and there could be two. Today there is
    one of each of the twenty, checked by hand while writing this; nothing
    keeps it that way."* Nothing had to.

    **An exact path is not a suffix question.** `CLAUDE.md` says
    `scripts/mutate.py` and means the one at the root, which is also the tail
    of `clients/python/scripts/mutate.py` — a different script, for that
    package's own suite. Reading that as ambiguous would be the guard
    misreading the repository's own most-read file: a path written from the
    root *is* the root's, because that is the base every reader uses. The
    suffix rule exists for citations that could not be read that way, so an
    exact match overwrites its own candidate list rather than joining it.
    """
    files = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.split()
    suffixes: dict[str, list[str]] = {}
    for path in files:
        parts = path.split("/")
        for at in range(len(parts)):
            suffixes.setdefault("/".join(parts[at:]), []).append(path)
    for path in files:
        suffixes[path] = [path]
    return suffixes


def differ(paths: list[str]) -> bool:
    """Do these files hold different bytes?

    A suffix matching two files is only a problem if the reader can land on
    the wrong one. `docs/orm-comparison.md` cites
    `google/rpc/error_details.proto`, which this repository vendors twice —
    once for `crates/slate-server`, once for `clients/typescript` — and the
    two copies are byte for byte the same file, because they are the same
    upstream file. A reader sent to either reads what the document meant.

    The rule earns a second job by existing: two vendored copies that drift
    stop being interchangeable, and the citation that was fine yesterday
    fails. That is the drift worth being told about, and nothing else in the
    repository looks for it.
    """
    bytes_at = {ROOT.joinpath(path).read_bytes() for path in paths}
    return len(bytes_at) > 1


def documents() -> list[Path]:
    """Every page under `SCOPE`, in a stable order."""
    found: list[Path] = []
    for where in SCOPE:
        at = ROOT / where
        if at.is_dir():
            for pattern in PAGES:
                found.extend(at.rglob(pattern))
        elif at.is_file():
            found.append(at)
    return sorted(found)


def cited() -> dict[str, list[str]]:
    """Each path cited under `SCOPE`, and the pages citing it."""
    where: dict[str, list[str]] = {}
    for page in documents():
        text = page.read_text(encoding="utf-8", errors="replace")
        for span in SPAN.finditer(text):
            for word in (span.group(1) or span.group(2)).split():
                at = AT_PLACE.fullmatch(word)
                path = at.group(1) if at else word
                # Whole word, never a substring: `--out=crates/x/src/lib.rs`
                # would otherwise be stored with its flag attached and read as
                # a file that is not there. Precision over reach — the reach a
                # looser match buys is one flag argument, and what it costs is
                # a guard that cries wolf, which is how a guard gets ignored.
                if CITED.fullmatch(path):
                    where.setdefault(path, []).append(page.relative_to(ROOT).as_posix())
    return where


def main() -> int:
    problems: list[str] = []
    here, seen = tracked(), cited()

    if not seen:
        # The never-fires case. `docs/` moved, or the pattern stopped matching,
        # and a rule that checked nothing must not print the line a rule that
        # checked a hundred and nine prints.
        print(
            f"no file paths cited under {', '.join(SCOPE)}. Either the tree "
            "moved or CITED stopped matching — both need a person, not a pass.",
            file=sys.stderr,
        )
        return 1

    for path, pages in sorted(seen.items()):
        if path in EXTERNAL:
            continue
        naming = ", ".join(sorted(set(pages)))
        if path not in here:
            problems.append(
                f"{naming} cites `{path}`, which is not "
                "a file here, nor the tail of one.\n"
                "  A document naming a file that is not there points a reader "
                "at nothing, and nothing compiles a document. Fix the path, or "
                "add it to EXTERNAL saying whose file it is."
            )
        elif len(here[path]) > 1 and differ(here[path]):
            problems.append(
                f"{naming} cites `{path}`, which is the tail of "
                f"{len(here[path])} files that differ: "
                f"{', '.join(sorted(here[path]))}.\n"
                "  A suffix that matches two files resolves to neither — the "
                "reader is sent to whichever they guess, and this guard would "
                "have called it resolved. Write enough of the path to pick "
                "one. (Identical copies of one vendored file are fine; these "
                "are not identical.)"
            )

    for path, whose in sorted(EXTERNAL.items()):
        if path not in seen:
            problems.append(
                f"EXTERNAL names `{path}` and nothing in scope cites it "
                "any more.\n  Drop the entry. A roster nobody is forced to "
                "edit is a roster that goes stale, which is this guard's own "
                f"subject. It said: {whose}"
            )
        elif path in here:
            problems.append(
                f"EXTERNAL names `{path}` as another project's and it now "
                "resolves here.\n  Either a file arrived at that path or the "
                "citation changed meaning. Drop the entry so the path is "
                f"checked like any other. It said: {whose}"
            )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    # "Exactly one file" would be the easy sentence and the wrong one: two
    # citations resolve to a pair of identical vendored copies, and a summary
    # that hid that would be this guard's own subject one level up.
    copies = sum(1 for path in seen if path not in EXTERNAL and len(here[path]) > 1)
    print(
        f"ok    {len(seen)} file paths cited across {len(documents())} pages "
        f"under {', '.join(SCOPE)}, each resolving to one file or to "
        f"identical copies of one ({copies} of the latter); "
        f"{len(EXTERNAL)} named as another project's"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
