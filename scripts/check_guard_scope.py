#!/usr/bin/env python3
"""A guard's docstring says which trees it reads, and the code agrees.

`check_cost_prose.py` walked `docs/` while one paragraph of its own docstring
said `docs/` was out of scope. #283 had widened the tree, corrected the
paragraph that *introduces* the scope, and left the paragraph forty lines below
that restates it — so the file said both, and the wrong one was the one a reader
hits looking for the answer. It was found by a person reading during unrelated
triage, and the entry that fixed it
(`ledger/2026-09-24-a-guard-said-it-did-not-read-the-tree-it-reads.md`) recorded
what is still missing:

> **Nothing checks a guard's account of itself.** [...] The class — a docstring
> that contradicts the code beneath it — is not covered by any check here.

This is that check, for the one part of a guard's self-description that is
mechanically comparable with its code: **which trees of the repository it
reads.** Everything else a docstring says — why a pattern is shaped the way it
is, what a mutation found — is prose a machine cannot judge, and this does not
try.

# The two rules

1. **A tree a docstring says is out of scope is not read.** This is the defect
   above, exactly. A guard whose paragraph says ``site/`` is out of scope and
   whose code holds a `ROOT / "site"` is contradicting itself.
2. **A tree a docstring says it reads, it reads.** The other direction, which
   is the one that arrives by ambition: a scope written before the code, or a
   widening reverted. Nothing here has had it yet; it costs one loop.

# Why a slash, and only a slash

A claim counts only when the tree is spelled with a trailing slash — `docs/`,
`` `site/` `` — never as a bare word. `check_cited_tests.py` says *"the ledger
is out of scope by principle"*, meaning a ledger entry's claims are not the
thing it verifies; it reads `ledger/` constantly. That sentence is about
content, and a guard that could not tell it from a statement about a directory
walk would report a contradiction that is not one — and a guard whose findings
need a human to sort into real and not is a guard people stop reading.

# What "reads" means here

Every `<root> / "tree"` expression in the file, with `<root>` any of the four
spellings `ROOT`, `REPO`, `root`, `repo` — the two module constants and the two
parameter names this repository uses for a root a test can override. Read with
`ast`, not by importing.

That range is not decoration, and it is measured: of the 21 guards here,
**10** build a tree path from a module-level `ROOT`, and **13** build one from
any of the four spellings. The three in the difference rebuild their paths from
a `root` *parameter* inside a function, which is this repository's convention
for a root a test can override. The first version of this file saw only the
ten, and rule 2 then reported three contradictions in `check_site_claims.py`
that were nothing but a root called `REPO`.

It is still deliberately incomplete: a path built by string concatenation, or
from a root under a fifth name, is invisible. The consequence is
one-directional for rule 1 — it can miss a contradiction, never invent one.
Rule 2 is the direction that *can* be wrong when a path is unseen, which is why
the subject pattern above is as narrow as it is: with both fixes it reports
nothing on this tree, and the four reports it made before them were four
false positives out of four.

Run directly: `python3 scripts/check_guard_scope.py`.
"""

from __future__ import annotations

import ast
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GUARDS = "check_*.py"

#: The trees a claim can be about. Not globbed off disk: a new top-level
#: directory should be a deliberate addition here, and a `target/` or a
#: `node_modules/` appearing in the list would make every mention of a build
#: path a claim.
TREES = (
    "crates",
    "docs",
    "site",
    "clients",
    "examples",
    "proto",
    "ledger",
    "scripts",
)

TREE = "|".join(TREES)

#: `` `docs/` is still out of scope ``, `site/ is not read`.
#:
#: The gap allows the hedges this repository's prose actually uses — "is still
#: out of scope", "is out of scope, and deliberately" — without letting a claim
#: reach across a sentence. `[^.\n]` is what stops it: a full stop ends a
#: claim, so "…reads `docs/`. Nothing here is out of scope by accident" is two
#: sentences and no contradiction.
DENIED = re.compile(
    rf"`?({TREE})/`?[^.\n]{{0,60}}?"
    r"(?:is (?:still )?out of scope|is not read|are not read|is not walked|is unread)"
)

#: `**It reads `crates/` and `docs/`.**` — the trees named in a sentence whose
#: subject is the guard itself.
#:
#: The subject is the whole difficulty. A first version matched any "reads" and
#: reported four contradictions, all four false: three were a root spelled
#: `REPO` that the path reader could not follow, and the fourth was this file's
#: own sentence about what *another* guard reads — "it reads `ledger/`
#: constantly", of `check_cited_tests.py`, four lines below. A guard whose
#: findings have to be sorted into real and not is a guard people stop reading,
#: so a claim must begin a sentence and its subject must be this file.
CLAIMS_READ = re.compile(
    r"(?:^|\. |\*\*)(?:It|This guard|This check|This)\s+(?:reads|walks)([^.\n]{0,120})",
    re.MULTILINE,
)
MENTION = re.compile(rf"`?({TREE})/`?")


def docstring(source: str) -> str | None:
    """The module docstring, or None if there is not one."""
    return ast.get_docstring(ast.parse(source))


#: What a guard calls the repository root. Four spellings, because two are
#: module constants (`ROOT`, `REPO`) and two are the *parameter* the same paths
#: are rebuilt from inside a function — which is this repository's convention
#: for a root a test can override, and is how eleven of the twenty guards
#: actually reach the tree. A reader that only knew `ROOT` saw none of them.
ROOTISH = re.compile(r"^(?:root|repo)$", re.IGNORECASE)


def roots(source: str) -> set[str]:
    """Which trees this file's `<root> / "…"` expressions point into.

    An `ast.BinOp` chain of `/` whose leftmost operand is a root-ish name,
    anywhere in the file rather than at module level only. The first string in
    the chain is the tree; a `"scripts/run_examples.sh"` written in one piece
    counts as `scripts`, which is why the split is on the value rather than on
    the number of operands.

    Read rather than imported: importing twenty scripts to inspect them runs
    twenty scripts' worth of module-level code for a question answerable from
    the text.
    """
    found: set[str] = set()
    for node in ast.walk(ast.parse(source)):
        if not isinstance(node, ast.BinOp) or not isinstance(node.op, ast.Div):
            continue
        parts: list[ast.expr] = []
        walk: ast.expr = node
        while isinstance(walk, ast.BinOp) and isinstance(walk.op, ast.Div):
            parts.insert(0, walk.right)
            walk = walk.left
        if not (isinstance(walk, ast.Name) and ROOTISH.match(walk.id)):
            continue
        first = parts[0]
        if isinstance(first, ast.Constant) and isinstance(first.value, str):
            head = first.value.strip("/").split("/")[0]
            if head in TREES:
                found.add(head)
    return found


def claimed_read(text: str) -> set[str]:
    """Trees a docstring says it reads or walks."""
    return {tree for sentence in CLAIMS_READ.findall(text) for tree in MENTION.findall(sentence)}


def problems(root: Path = ROOT) -> list[str]:
    """Every guard under `scripts/`, both rules."""
    said = []
    guards = sorted((root / "scripts").glob(GUARDS))
    claims = 0
    for path in guards:
        source = path.read_text(encoding="utf-8")
        text = docstring(source)
        if text is None:
            continue
        read = roots(source)
        denied = set(DENIED.findall(text))
        wanted = claimed_read(text)
        claims += len(denied) + len(wanted)
        for tree in sorted(denied & read):
            said.append(
                f"scripts/{path.name}'s docstring says `{tree}/` is out of "
                f"scope, and the file builds a path into it.\n"
                "  One of the two is stale. That is the #299 failure — a "
                "correction that reached one copy of the claim —\n"
                "  and the copy a reader hits first is usually the wrong one."
            )
        for tree in sorted(wanted - read):
            said.append(
                f"scripts/{path.name}'s docstring says it reads `{tree}/`, "
                "and no module-level path points there.\n"
                "  Either the scope was written before the code, or a "
                "widening was reverted and the sentence stayed."
            )
        # And the other direction, which this rule did not have. A claim that
        # names *fewer* trees than the code reads is the same staleness with
        # the sign flipped, and it is the one that actually happened: both
        # guards making a scope claim understated it by exactly one tree,
        # `check_cost_prose.py` omitting `site/` after #288 widened it and
        # `check_site_claims.py` omitting `docs/`. Only checking claimed ⊆ read
        # let a widening land in the code and not in the sentence — which is
        # the failure this guard exists to catch, in this guard.
        #
        # Only when the docstring makes a claim at all: a guard that says
        # nothing about its scope is saying nothing, not saying the wrong
        # thing, and forcing twenty-eight of them to declare would be a
        # different rule with a different cost.
        for tree in sorted(read - wanted) if wanted else ():
            said.append(
                f"scripts/{path.name}'s docstring lists the trees it reads "
                f"and `{tree}/` is not among them, while the file builds a "
                "path into it.\n"
                "  A scope claim that understates is read as exhaustive. "
                "Widen the sentence, or say why that path is not scope."
            )

    # The never-fires halves, and there are three because three different
    # things could quietly leave this reading nothing: no guards found, no
    # scope claims parsed, no roots parsed.
    if not guards:
        said.append(
            f"no `scripts/{GUARDS}` at all, so this compared nothing. Either "
            "the guards moved or the glob did."
        )
    elif not claims:
        said.append(
            "no guard's docstring makes a scope claim this can read, which was "
            "false for at least three of them when this was written. The "
            "phrasings moved; move the patterns with them."
        )
    elif not any(roots(path.read_text(encoding="utf-8")) for path in guards):
        said.append(
            'no guard builds a `<root> / "…"` path at all, so every '
            "claim above was compared against an empty set and none of them "
            "could fail. The convention changed; teach `roots` about it."
        )
    return said


def main() -> int:
    said = problems()
    if said:
        for one in said:
            print(one, file=sys.stderr)
        print(f"\n{len(said)} problem(s)", file=sys.stderr)
        return 1
    guards = sorted((ROOT / "scripts").glob(GUARDS))
    scoped = sum(
        1
        for path in guards
        if (text := docstring(path.read_text(encoding="utf-8")))
        and (DENIED.search(text) or claimed_read(text))
    )
    print(
        f"ok    {len(guards)} guards, {scoped} making a scope claim, every "
        "claim matching the paths beneath it"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
