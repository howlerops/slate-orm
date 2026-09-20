#!/usr/bin/env python3
"""Every test `docs/` points at as a demonstration must be openable.

`docs/security-review.md` spent a week naming four probes that no longer
existed. Each had been *inverted* by the commit that fixed the finding it
demonstrated — `a_cascade_from_a_shared_parent_crosses_the_tenant_boundary`
became `a_shared_parent_with_a_tenant_scoped_child_is_refused`, and so on —
and the document went on saying "Demonstrated by" the old name. A security
review whose demonstrations cannot be located demonstrates nothing, and the
failure is silent in a way a renamed *function* is not: nothing compiles a
document.

WHY THE RULE IS AS NARROW AS IT IS. The obvious check — every backticked
snake_case identifier in every Markdown file must exist — was measured before
this one was written, and flags 38 names of which 5 are real: an 87% false
positive rate, from four sources.

  1. `ledger/` cites dead names **on purpose**. An entry is a dated record;
     `2026-09-15-pages-that-do-not-shift.md` says "There *was* a test called
     X", which is provenance, not rot, and updating it would destroy the thing
     a ledger is for. So the ledger is out of scope by principle rather than
     by convenience.
  2. Identifiers that are not tests at all: clippy lint names in `CLAUDE.md`
     (`chunks_exact_to_as_chunks`), tonic builder methods
     (`max_decoding_message_size`), SlateDB config keys (`l0_sst_size_bytes`),
     generated protobuf symbols. A four-word threshold excludes most of these,
     because an external API name is short and a test name here is a sentence.
  3. Python tests cited without their `test_` prefix, which the docs do
     routinely. Both spellings are accepted.
  4. A `docs/` passage that names the dead test *beside* its successor, which
     is exactly how the fix to the failure above was written. So the rule is
     scoped to a paragraph: a missing name is a finding only if its paragraph
     names nothing a reader can open. That single refinement is what took the
     rate from 87% to zero — and it is falsifiable, which is the point: it
     flagged 5 on the pre-fix document and 0 on the post-fix one.

The threshold is five words and not four because four admits
`max_encoding_message_size` and its four siblings, and no test in this
repository is named in four words. That is an observation about today's tree;
if it stops being true the failure is a false negative, which is the safe
direction for a check that blocks a commit.
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: Where a name is read as a live claim. `docs/` only — see the module docstring
#: for why `ledger/` and `CLAUDE.md` are deliberately not here.
SCOPE = "docs"

#: Five words or more, backticked. See the docstring for the threshold.
CITED = re.compile(r"`([a-z][a-z0-9]*(?:_[a-z0-9]+){4,})`")

DEFINED = (
    ("*.rs", re.compile(r"(?:async\s+)?fn\s+([a-z_][a-z0-9_]*)")),
    ("*.py", re.compile(r"def\s+([a-z_][a-z0-9_]*)")),
)

SKIP = ("target/", "node_modules/", "/.git/")


def defined_names(root: pathlib.Path) -> set[str]:
    names: set[str] = set()
    for glob, pattern in DEFINED:
        for path in root.rglob(glob):
            if any(part in str(path) for part in SKIP):
                continue
            names |= set(pattern.findall(path.read_text(errors="replace")))
    # A `pytest` test is `test_a_count_...` on disk and `a_count_...` in prose.
    # Accepting both is not laxity: the prose spelling is the one a reader
    # searches for, and refusing it would flag correct citations.
    return names | {n[len("test_"):] for n in names if n.startswith("test_")}


def unresolvable(text: str, names: set[str]) -> list[str]:
    """Names in `text` that resolve to nothing, paragraph by paragraph.

    A paragraph that also cites something real is history and is left alone;
    one that cites only the dead is a claim a reader cannot check.
    """
    found: list[str] = []
    for paragraph in re.split(r"\n\s*\n", text):
        cited = CITED.findall(paragraph)
        if cited and not any(name in names for name in cited):
            found.extend(name for name in cited if name not in names)
    return found


def main(argv: list[str] | None = None) -> int:
    # The root is an argument so the tests beside this file can build a small
    # tree and run the real thing over it, rather than asserting against the
    # repository and passing for whatever reason the repository happens to
    # supply. `scripts/test_mutate.py` earned that lesson the same way.
    argv = sys.argv[1:] if argv is None else argv
    root = pathlib.Path(argv[0]).resolve() if argv else ROOT

    names = defined_names(root)
    failures = 0
    scanned = 0
    for path in sorted((root / SCOPE).rglob("*.md")):
        scanned += 1
        for name in unresolvable(path.read_text(errors="replace"), names):
            rel = path.relative_to(root)
            print(f"{rel}: `{name}` names no test, and nothing beside it does either")
            failures += 1
    if failures:
        print(
            f"\n{failures} citation(s) a reader cannot open. Either point at the "
            f"test that exists now, or — if the dead name is deliberate history "
            f"— name the live one in the same paragraph, which is what a reader "
            f"needs anyway.",
            file=sys.stderr,
        )
        return 1
    print(f"{scanned} documents in {SCOPE}/, every cited test resolves")
    return 0


if __name__ == "__main__":
    sys.exit(main())
