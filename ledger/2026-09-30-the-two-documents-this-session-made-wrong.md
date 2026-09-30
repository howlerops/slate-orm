# The two documents this session's own work made wrong

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `scripts/caveats.py`, `CLAUDE.md`
- **Kind:** docs

## What changed

`unchecked()`'s docstring in `scripts/caveats.py` described a worklist over
1038 `deliberate` caveats and explained how successive passes would pool
against it. The frame is empty: all 1016 rows were read on 2026-09-30. The
docstring now says so, says what an empty worklist does *not* tell a reader —
zero rows and a tool nobody runs print the same thing — points at
`--unchecked <days>` as the question that replaces it, and names the summary
line as the number that still moves when the bucket grows.

`CLAUDE.md`'s paragraph on `SLATE_SERVERD` stopped at *"a path that is set and
missing is a hard error"*. It now also carries the staleness refusal, that
`scripts/prebuilt.py` is the single implementation all six harnesses use, that
`scripts/test_prebuilt.py` drives four of them and requires a named drive test
for the Go and TypeScript ones, and that an unknown `SLATE_*` variable naming
an executable is refused rather than run.

## Why

`CLAUDE.md` says it: *"Stale documentation is worse than none, because it is
read as current. If a change makes a doc comment, a README bullet or a design
note wrong, fixing it is part of the change."* Both of these were made wrong
by work committed earlier today, and both are documents a session reads before
doing anything — a docstring that says 1038 rows are unread would send the
next pass to re-derive a frame that is empty, and a `CLAUDE.md` paragraph that
stops at "missing is an error" is how a stale binary gets trusted, which is
the failure the whole prebuilt mechanism exists for.

This is a separate entry rather than an amendment because the entries it
corrects are committed and dated, and `ledger/README.md` forbids rewriting
one. What it is not is a second account of those changes: it records that a
sweep for documents they falsified happened, and which two it found.

## Alternatives rejected

**Folding both into the entries that caused them.** Those entries are
committed, and an entry rewritten after the fact is not a record — the rule
`ledger/README.md` states and that `docs/caveat-status.json` exists to work
around. The cost is a fourth entry today that says little on its own.

**Leaving the docstring, since `--unchecked` prints `0` and a reader would
see it.** They would see zero and not know whether that means read or
unlooked-at; the number that distinguishes them is on the summary line, and
nothing said so.

**A guard holding prose to the tracker's counts.** `check_site_claims.py` and
`check_cost_prose.py` already hold *numbers* a reader would take as fact, and
the count in a docstring is narration rather than a claim anything derives
from — the same call `ledger/2026-09-27-one-javascript-error-would-have-emptied-every-sidebar.md`
makes about `CLAUDE.md`'s job count. Adding a third prose guard for one
sentence is a rule fitted to one example.

## Evidence

`grep -n '1038\|1008\|thirty-four sessions\|246 of'` over `scripts/`, `docs/`,
`CLAUDE.md`, `README.md` and `site/docs/` found four hits in
`scripts/caveats.py` and one unrelated number in `docs/performance.md` (a
confidence interval that happens to contain 1038.4 µs). All four were in the
one docstring.

`sh scripts/check.sh` is green, which includes `check_cited_files.py` over
`CLAUDE.md` — 135 paths across 30 pages, each resolving — and
`check_site_claims.py` over the 28 site pages.

No mutation run: prose has nothing to mutate, which is the standing answer
recorded against `ledger/2026-09-23-five-built-features-the-docs-never-mentioned.md`.

## What this does not do

**It is a grep, not a reading of every document.** Four number-shaped strings
were searched for, chosen because they are the counts this session moved. A
sentence made wrong in words rather than in digits — "the frame is a worklist
nobody has emptied" — is not findable that way and was not looked for
elsewhere.

**Nothing stops the next one.** A guard would have to decide whether a
sentence of English is still true of the tree, which is the prose-parsing
problem every guard in `scripts/` declines. The habit is the mechanism, and
this entry is the record that it ran today.
