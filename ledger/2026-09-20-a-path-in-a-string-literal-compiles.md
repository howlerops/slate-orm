# A `docs/` path in an error message is a claim nothing checks

- **Date:** 2026-09-20
- **Author:** Claude, closing a gap named in the previous entry rather than leaving it named
- **Touches:** `scripts/{check_cited_docs.py,test_check_cited_docs.py,check.sh}`, `.github/workflows/ci.yml`
- **Kind:** process

## What changed

`check_cited_docs.py`: every `docs/*.md` a source file points at must be a
file. Eleven cases in its own guard, both registered in `check.sh` (now 31) and
in CI's `scripts` job.

No defect found. All 50 citations currently resolve.

## Why

The previous entry closed with:

> **The refusal's message is prose with no guard on its accuracy.** … nothing
> would notice if `docs/ctes.md` were deleted while the message kept pointing
> at it. `scripts/check_cited_tests.py` checks cited *test* names, not cited
> paths.

Writing that down and stopping is how a caveat becomes a defect. The check is
one loop.

It is the companion to `check_cited_tests.py` one level over: that one catches
a **document** naming a test that no longer exists, this one catches **code**
naming a document that no longer exists. Both exist because nothing compiles a
citation — and a path inside a string literal is the worse of the two, because
it reaches a user rather than a maintainer, and it reaches them at the moment
they are already confused enough to have triggered an error.

## Alternatives rejected

**Check every path-looking string, not just `docs/*.md`.** Wider and mostly
noise: source here names `target/debug/slate-serverd`, `clients/python[dev]`,
`.githooks/test-pre-commit.sh` and a dozen paths that are arguments, not
citations — some conditional, some created at run time. `docs/` is the one
prefix where "this file exists" is unconditionally the claim being made.

**Check Markdown too.** `site/check/docs.py` already requires every relative
link in the docs site to resolve, so a `docs/` cross-link is covered. Adding a
second checker over the same files would be two answers to one question.

**Include the ledger.** Rejected by the same principle
`check_cited_tests.py` states for test names: an entry is a dated record, and
"moved to `docs/x.md`" stays true about the day it was written even after `x.md`
moves again. Rewriting history to satisfy a linter destroys the thing a ledger
is for.

**A glob for the fixture files instead of a named list.** `test_check_cited_
tests.py` writes a temporary tree containing `docs/d.md` and runs the other
checker over it — a fixture path, not a claim. A pattern broad enough to skip
it (`scripts/test_*.py`, say) would also skip a real citation somebody wrote
carelessly in a test. `FIXTURES` names three files and gives each a reason,
which is the `EXPECTED_REFUSALS` idiom this repository already uses: a list you
are forced to edit is a list that stays true.

**Skip the never-fires guard.** The check reports zero problems on a clean tree
and zero problems on a tree it cannot see. Renaming `docs/` or editing the
regex wrong would produce the second and read as the first, so a run finding
*no citations at all* is a failure with its own message.

## Evidence

**Measured before the exclusion list was written**, which is what produced it.
Raw counts of `docs/*.md` across all source:

```
14 docs/performance.md      4 docs/security-review.md    1 docs/undo-window.md
10 docs/arrays.md           4 docs/ctes.md               1 docs/user_data_dir.md
 9 docs/paging-a-join.md    3 docs/orm-comparison.md     1 docs/chromium_browser…md
 7 docs/d.md                2 docs/topology.md
 5 docs/correctness.md      1 docs/validation.md
```

Three of those do not exist. All three are explained rather than reported:
`docs/d.md` is a fixture inside `test_check_cited_tests.py`, and
`user_data_dir.md` and `chromium_browser_vs_google_chrome.md` come from
`playwright-core`'s bundled type definitions under `node_modules` — neither is
ours and neither should exist here. Every real citation resolves, so **this
guard found nothing, and that is the honest result**: it is prospective, like
`every_type_has_its_own_non_zero_code` added earlier today.

Six mutations, all caught:

```
ok  a broken citation is not reported          -> four cases
ok  the ledger is checked after all            -> the ledger is out of scope
ok  vendored trees are read                    -> two cases
ok  only the first citation on a line is seen  -> two citations on one line are both seen
ok  the named fixture file is not excluded     -> a named fixture file is excluded, and only by name
ok  a non-markdown path counts as a citation   -> a path that is not a .md is not a citation
```

The fourth is the one worth having: a first version using `search` rather than
`findall` would count one citation per line and miss the second, which reads as
a checked line rather than a half-checked one.

`scripts/check.sh`: 31 of 31. `scripts/test_check_sh.py`: 69 steps and 6
blocks, all accounted for — the accounting guard agrees the two new CI steps
are registered. `ruff check`: clean.

## What this does not do

**It checks existence, not aptness.** A message pointing at `docs/ctes.md` when
the reasoning moved to another file passes, because the file is still there. The
same limitation `check_cited_tests.py` has: a test that exists and no longer
demonstrates the thing it is cited for passes too.

**It does not check anchors.** `docs/x.md#section` is matched as `docs/x.md`
and the fragment is ignored, so a link to a heading that was renamed resolves.
Nothing in the tree uses one today, which is why it was not worth the parsing.

**Four suffixes, and no guard on the list.** `.rs`, `.py`, `.go`, `.ts`. A
citation in a `.tsx`, a `.sh`, a `.toml` comment or a `.yml` is not read, and
nothing holds `SOURCE_SUFFIXES` to the languages this repository contains — it
is exactly the hand-written roster this repository keeps finding stale, in a
file written to catch staleness. Left because deriving it means deciding what
counts as source, which is a broader question than the one this closes.

**`SKIP_PARTS` is a hand-written roster too**, and a vendored tree with a name
not on it would be read. The failure mode is a false positive rather than a
miss, which is the safer direction, and it would be noisy enough to notice.

**It runs over the whole tree on every invocation.** `rglob("*")` across the
repository including `target/`, filtered afterwards. Fast enough that it was
not worth measuring, which is a judgement rather than a measurement.
