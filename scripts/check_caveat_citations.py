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
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STATUS = ROOT / "docs" / "caveat-status.json"

#: The trees a `by` may name. Rooted rather than free-form, so that a sentence
#: mentioning `site/` prose or a bare `foo.py` is not read as a citation — the
#: same narrowness `check_cited_docs.py` uses, and for the same reason: a
#: pattern wide enough to catch every careless reference also catches every
#: ordinary noun.
#: Vendored or generated trees, skipped when collecting defined names. The
#: same list `check_cited_docs.py` carries, for the same reason: a megabyte of
#: `node_modules` would make almost any name look defined.
SKIP_TREES = frozenset(
    {"node_modules", "dist", "dist-test", "target", ".git", "_proto", "__pycache__"}
)

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


#: A function or test name a `by` cites, in backticks.
#:
#: The same shape `scripts/check_cited_tests.py` reads, and the same threshold:
#: five or more underscore-separated words. Below that the pattern matches
#: ordinary identifiers — `read_text`, `max_groups` — and a citation guard that
#: reports a field name is one people learn to ignore.
#:
#: `ledger/2026-09-26-the-citation-nobody-could-follow.md` recorded this as the
#: half not done: "Nothing checks a `by` that names a test, a function or a
#: commit. Several name `security_probe.rs`'s test functions … and only the
#: file half of those is followed."
NAMED = re.compile(r"`([a-z][a-z0-9]*(?:_[a-z0-9]+){4,})`")

#: Where a cited name may be defined, per language.
DEFINES = (
    ("*.rs", re.compile(r"(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)")),
    ("*.py", re.compile(r"def\s+([a-z_][a-z0-9_]*)")),
    ("*.go", re.compile(r"func\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)")),
    ("*.ts", re.compile(r"(?:function|const)\s+([A-Za-z_][A-Za-z0-9_]*)")),
)


#: A path a verdict cites *because it is gone*, and why that is the citation.
#:
#: A caveat closed by a deletion has nowhere else to point. Two verdicts on
#: 2026-09-30 described assertions in `site/check/playground.py`, which was
#: deleted with the landing page it checked; the whole content of both closures
#: is that the file is not there, and rewording the `by` to avoid naming it
#: would make the closure unfollowable to hide it from this rule.
#:
#: Keyed by `(entry, path)` rather than by path, like `check_cited_docs.py`'s
#: `NOT_A_FILE`, so the exemption is per-claim and a second verdict citing the
#: same dead path has to argue for itself. Two rot rules below: an entry whose
#: path has come *back* is refused, because the citation is then ordinary and
#: the exemption hides a live check; and an entry nothing cites any more is
#: refused, because a roster nobody is forced to edit is a roster that goes
#: stale — the argument `EXTERNAL` and `NOT_A_FILE` both make.
GONE: dict[tuple[str, str], str] = {
    (
        "2026-09-14-a-playground-nobody-could-find.md",
        "site/check/playground.py",
    ): (
        "deleted in 5ffc331 when the landing page became the workbench, which "
        "is what closed both of that entry's deliberate caveats: they describe "
        "assertions in that file"
    ),
}


def defined(root: Path) -> set[str]:
    """Every function and test name this repository defines."""
    names: set[str] = set()
    for glob, pattern in DEFINES:
        for path in root.rglob(glob):
            if SKIP_TREES & set(path.relative_to(root).parts):
                continue
            names.update(pattern.findall(path.read_text(encoding="utf-8", errors="replace")))
    return names


def cited_names(by: str) -> list[str]:
    """Every function-shaped name a `by` cites, deduplicated, in order."""
    seen: list[str] = []
    for hit in NAMED.findall(by):
        if hit not in seen:
            seen.append(hit)
    return seen


def citations(by: str) -> list[str]:
    """Every path-shaped citation in one `by` string, in order, deduplicated."""
    seen: list[str] = []
    for hit in CITATION.findall(by):
        if hit not in seen:
            seen.append(hit)
    return seen


def ignored(root: Path) -> set[str]:
    """Every path in the tree that git is told to ignore; empty outside one.

    Resolving against what git *will not carry* rather than against the
    filesystem alone, and the difference is not pedantry — it is the
    local-versus-CI gap this repository keeps meeting. A verdict cited
    `site/slate_wasm_bg.wasm`, a **build output** listed in `.gitignore`: it is
    on the container that built it, so this printed `ok` here, and it is not in
    the checkout, so CI failed on a file nobody had touched. The same shape
    `CLAUDE.md` describes for `ty` resolving imports against whatever
    `site-packages` happens to be installed.

    Ignored-ness rather than tracked-ness, and the difference matters on the
    commit that *adds* the cited file: a brand-new entry is untracked until
    `git add`, and a rule of "must be tracked" fails a `check.sh` run made
    before staging — which is when it is run. An ignored path is never going to
    be in a checkout; an untracked one is about to be.

    Empty when git cannot answer — a tarball, a fixture that is not a
    repository — which is the same answer as "nothing here is ignored" and
    wants the same behaviour: fall back to the filesystem alone, because a
    guard that refuses to run outside a checkout is a guard that cannot be
    tested over a temporary directory. An earlier draft returned `None` for
    that case and branched on it; a mutation collapsing the two was caught by
    nothing, because there was nothing to catch.
    """
    found = subprocess.run(
        ["git", "status", "--porcelain", "--ignored=matching", "-z"],
        cwd=root,
        capture_output=True,
        check=False,
    )
    if found.returncode != 0:
        return set()
    out = set()
    for entry in found.stdout.decode("utf-8", "replace").split("\0"):
        if entry.startswith("!! "):
            out.add(entry[3:].rstrip("/"))
    return out


def resolves(cited: str, hidden: set[str], root: Path) -> bool:
    """Is the cited path something a checkout of this repository would carry?

    On disk, and not ignored — nor inside an ignored directory, because
    `git status` names a whole ignored tree by its directory rather than
    listing what is in it.
    """
    if not (root / cited).exists():
        return False
    parts = Path(cited).parts
    return not any(
        "/".join(parts[: n + 1]) in hidden for n in range(len(parts))
    )


#: A reason that asserts something is *absent* from this tree.
#:
#: The class this catches, in one sentence: **a verdict whose reason states an
#: unchecked fact about the tree.** It was written after doing exactly that.
#: `ledger/2026-09-30-the-image-that-copies-part-of-the-tree.md` justified not
#: reading `.dockerignore` with
#:
#:   > Nothing currently excludes such a directory, and a second parser is
#:   > cost against a failure nobody has had.
#:
#: — a claim about a twenty-line file nobody had opened, in an entry arguing
#: that a hand-maintained list needs a guard rather than a comment. The file
#: is an allow-list, and the next CI run failed on it. Four minutes.
#:
#: A negative existential is the most checkable kind of claim and the least
#: checked, because nobody asks for a demonstration that a problem does not
#: exist. The rule is therefore the weakest thing that would have caught it:
#: **say where you looked.** A reason claiming an absence must cite at least
#: one path, so a reader has somewhere to go and disagree. It cannot tell
#: whether the sentence is *true* — that is the reading — only whether it is
#: checkable at all.
ABSENCE = re.compile(
    r"\bnothing (?:currently |in the tree |here |else )?"
    r"(?:excludes|names|reads|checks|uses|calls|references|holds|carries|"
    r"matches|imports|declares|defines|touches|depends)\b"
    r"|\bno (?:file|caller|test|guard|script|crate|module|entry|path) "
    r"(?:currently |in the tree |here )?"
    r"(?:excludes|names|reads|checks|uses|calls|references|holds|carries)\b"
    r"|\bnothing (?:currently |in the tree |here )?(?:does|has) (?:this|that|so)\b",
    re.IGNORECASE,
)

#: A search recorded in the reason: `git grep …`, `grep …`, `rg …`, `ls …`.
#:
#: The second way to anchor an absence, and for most of them the *only* one:
#: "nothing in the tree uses an anchor" is a claim about the tree, not about
#: a file, so there is no path to cite and the honest anchor is the search
#: that was run. Requiring a path would have pushed every such reason into
#: the exemption roster, which is how a rule becomes a formality.
#:
#: What both forms have in common is the thing that matters: the reason says
#: **where somebody looked**, so a reader can look in the same place and
#: disagree. Neither says the answer was read correctly.
SEARCHED = re.compile(r"`(?:git grep|grep|rg|ls|git ls-files)\b[^`]*`")

#: Reasons that make an absence claim and deliberately cite no path.
#:
#: Keyed the way every roster here is — `(entry, first 60 characters of the
#: key)` — so a reworded caveat orphans its row and is read again. The reason
#: is the point: "the absence is of a *concept*, not a file" is the only
#: shape that belongs here, and an entry that could name a file and did not
#: is the failure this rule exists for.
UNANCHORED: dict[tuple[str, str], str] = {}


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

    hidden = ignored(root)
    dead: list[str] = []
    excused: set[tuple[str, str]] = set()
    counted = 0
    for verdict in verdicts:
        by = verdict.get("by") or ""
        for cited in citations(by):
            counted += 1
            at = (verdict.get("entry", "?"), cited)
            if at in GONE:
                excused.add(at)
                continue
            if not resolves(cited, hidden, root):
                dead.append(f"{verdict.get('entry', '?')}: {cited}")

    # The never-fires guard every check in this directory carries. A renamed
    # field, or a file whose verdicts all cite prose, finds nothing and reads
    # exactly like a file whose every citation resolves.
    # The name half. Collected first so the never-fires guard below can cover
    # both kinds of citation with one check.
    wanted: list[tuple[str, str]] = []
    for verdict in verdicts:
        for name in cited_names(verdict.get("by") or ""):
            wanted.append((verdict.get("entry", "?"), name))

    # One never-fires guard over both halves. A renamed field, or a file whose
    # every `by` became prose, finds nothing of either kind and reads exactly
    # like a repository whose every citation resolves.
    record(
        "some verdict cites a path or a name at all",
        counted > 0 or bool(wanted),
        f"{counted} paths and {len(wanted)} names across {len(verdicts)} verdicts",
    )
    record("every cited path resolves", not dead, "\n      ".join(dead))

    # The two rot rules on `GONE`. Without them it is a list that only grows,
    # and an exemption for a path that has come back is an exemption hiding a
    # rule that would now pass on its own.
    back = [
        f"{entry}: {cited} resolves again"
        for (entry, cited) in sorted(GONE)
        if resolves(cited, hidden, root)
    ]
    record(
        "no GONE entry names a path that is back in the tree",
        not back,
        "\n      ".join(back),
    )
    unused = [
        f"{entry}: {cited}" for (entry, cited) in sorted(set(GONE) - excused)
    ]
    record(
        "every GONE entry is one some verdict still cites",
        not unused,
        "\n      ".join(unused) and
        "\n      ".join(unused) + "\n      drop the row; the verdict stopped citing it",
    )

    # `defined()` walks the workspace, which is seconds of I/O, so it runs only
    # when something cited a name.
    missing = []
    if wanted:
        known = defined(root)
        missing = [f"{entry}: `{name}`" for entry, name in wanted if name not in known]
    record(
        "every cited test or function name is defined somewhere",
        not missing,
        "\n      ".join(missing),
    )

    # The absence rule. See `ABSENCE`.
    unanchored: list[str] = []
    claiming = 0
    used: set[tuple[str, str]] = set()
    for verdict in verdicts:
        by = verdict.get("by") or ""
        if not ABSENCE.search(by):
            continue
        claiming += 1
        at = (verdict.get("entry", "?"), (verdict.get("key") or "")[:60])
        if at in UNANCHORED:
            used.add(at)
            continue
        if citations(by) or SEARCHED.search(by):
            continue
        unanchored.append(
            f"{at[0]}: {at[1]!r}\n        {by[:160]}"
        )
    record(
        "every verdict claiming an absence names a path somebody can open",
        not unanchored,
        "\n      ".join(unanchored) and
        "\n      ".join(unanchored)
        + "\n      A negative existential is checkable in one `git grep`, and"
          " nobody asks for\n      a demonstration that a problem does not"
          " exist. Cite the file you read, or the\n      `git grep` you ran,"
          " or add a row to UNANCHORED saying why there is neither.",
    )
    # The never-fires half: a pattern that matches nothing reads exactly like
    # a repository where every absence claim is anchored.
    #
    # Asked of the real tree only. `scripts/test_check_caveat_citations.py`
    # builds a fixture per rule, each holding the two or three verdicts that
    # rule is about, and requiring every one of them to also contain an
    # absence claim would add noise to six fixtures to answer a question
    # about none of them. The question — *is this pattern matching anything
    # in this repository* — is a question about this repository.
    if root == ROOT:
        record(
            "some verdict makes an absence claim at all",
            claiming > 0,
            f"{claiming} of {len(verdicts)} verdicts",
        )
    stale = [f"{entry}: {key!r}" for (entry, key) in sorted(set(UNANCHORED) - used)]
    record(
        "every UNANCHORED row is one a verdict still needs",
        not stale,
        "\n      ".join(stale),
    )
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
    print("every path, test and function name a caveat verdict cites is there")
    return 0


if __name__ == "__main__":
    sys.exit(main())
