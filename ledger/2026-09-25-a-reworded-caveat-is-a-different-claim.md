# Editing my own entry orphaned four verdicts, which is the tracker working. And the suite one caveat named has now been run.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `docs/caveat-status.json`
- **Kind:** bookkeeping, and one caveat closed by running the thing

## What changed

Three things in the verdict file, no code:

- **Four orphaned verdicts dropped.** The previous commit rewrote four bullets
  in `2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md` — the triage
  finished, so "502 caveats are still untriaged" and "No mutation run" stopped
  being true. Their verdicts detached and were reported as orphans.
- **The new wording triaged**, along with the four caveats the grammar entry
  added.
- **One caveat closed by evidence:** "The `slate-wasm` suite was not run."
  It has been now — `cargo test -p slate-wasm`, **226 tests across 14 files,
  all passing**.

`773 caveats: 280 open, 166 closed, 280 deliberate, 0 untriaged.`

## Why the orphans are the point

`ledger/README.md` forbids rewriting an entry, and this was the exception it
allows: a claim that had stopped being true in the same session that wrote it.
The verdicts were keyed to the old text, so they detached.

That is the behaviour `scripts/caveats.py` was designed for and it is worth
recording that it fired on its author within an hour. A reworded caveat is a
different claim and should be read again; the alternative — fuzzy-matching a
verdict onto whatever the bullet says now — would silently carry a judgement
onto a sentence nobody judged.

## The wasm number, and why it took two runs

The first run captured nothing. The background command ended `| tail -5`, so
what landed in the output file was the *doctest* line — `0 passed` — and I
nearly wrote that down as the result. The suite had in fact run fourteen test
binaries above it, and the pipe threw them away.

That is the second lie in `mutate.py`'s list arriving somewhere else: "nothing
ran" and "I did not capture what ran" produce identical output. The fix was to
re-run and `grep -E "^test result:"` rather than `tail`.

## Alternatives rejected

**Edit the entry again to say the suite passed.** The README permits editing a
claim that was wrong when written; this one was *right* when written and
stopped being true afterwards. That is precisely what a verdict is for, and
editing the entry would put the status back inside the record it is supposed
to sit beside.

**Re-key the orphans onto the new text automatically.** A prefix match would
have done it. It would also have carried "this is still work" onto a sentence
now saying the work is done — the failure the orphan report exists to prevent.

**Skip the entry because nothing but JSON changed.** The hook refused, and
correctly: the file encodes 773 judgements and how they are maintained is
exactly the reasoning a diff does not carry.

## Evidence

`python3 scripts/caveats.py` exits 0 with no orphans and no problems.
`cargo test -p slate-wasm`: 4, 5, 35, 15, 2, 7, 8, 6, 26, 50, 23, 22, 6, 17 —
226 passing, 0 failing.

## What this does not do

**No mutation run.** Nothing executable changed; the code this touches was
mutation-tested in the commit that added it.

**The 226 were not read.** I have the counts, not the names, so "the browser
suite covers the single-table grammar" remains an inference from the previous
entry's surviving mutation rather than something I confirmed by finding the
test. The point of that entry stands either way: the coverage should not have
been only there.

**Nothing prevents the next orphan.** Rewording a bullet still detaches its
verdict silently until somebody runs the tracker, and the tracker is not in
CI. Adding it there is a real thing to want and is not done.
