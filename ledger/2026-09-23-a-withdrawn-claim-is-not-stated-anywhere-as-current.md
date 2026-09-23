# Three times in one day I corrected a claim in one place and left it standing in the others. A guard now refuses the third copy.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #299 (F6s)
- **Touches:** `scripts/check_retired_claims.py`, `scripts/retired_claims.json`, `scripts/test_check_retired_claims.py`, `scripts/check.sh`, `scripts/check_cited_docs.py`
- **Kind:** a guard for the failure this session kept committing

## What changed

`scripts/retired_claims.json` lists phrases this repository has withdrawn, each
naming the entry that withdrew it. `check_retired_claims.py` reads the live
tree and fails on any occurrence not marked `NOT_A_CLAIM` or struck through.
`ledger/` is never read: an entry states what was believed on its date.

Text is flattened before matching — comment markers, Markdown emphasis and line
breaks stripped, whitespace collapsed — so a phrase is matched against the
sentence a reader sees. This is the part that matters, and it is not
hypothetical: the claim that prompted the guard was written

    /// reach — and on this container the runner that *does* reach it cannot be
    /// built, because nine debug example binaries exhaust the disk.

A line-by-line grep for that claim finds nothing. `check_cost_prose.py` reached
the same conclusion from the same evidence.

Three claims seed the registry, all retired today.

## Why

#298 re-measured what the example suite costs, corrected `CLAUDE.md`, wrote the
entry — and left the identical false claim in the doc comment on
`assert_wrote_something`, and left the mutation record carrying it in
machine-readable form with nothing linking it forward. `run_examples.sh` said
nine examples were eight, in two comments, and had since one was added.

All three were found by reading, afterwards, on a third pass. The one in the
source was written by the same session that was at that moment reasoning about
stale documentation. That is not a discipline problem to be solved by trying
harder; `CLAUDE.md` already says stale documentation is worse than none, and it
did not help.

## Alternatives rejected

**Detect that two sentences assert the same thing.** What would actually have
caught all three at the time, and is the undecidable problem `mutate.py`
already refuses for equivalent mutations. The cheap approximations — fuzzy
matching, shared n-grams — produce a guard whose failures a reviewer learns to
scroll past, which is worse than none.

**Parse retirements out of the ledger entries themselves**, so no registry is
maintained. Attractive, since the entry already exists and already names what
it withdraws. Rejected: it would need a structured marker inside the prose to
say which span is the retired wording, at which point the registry exists
anyway, spread across 166 files instead of one. `scripts/frozen_tables.json` is
the precedent for a list beside the guard that reads it.

**A pre-commit hook rather than a check.** The hook is for the ledger's
existence, which is cheap to verify. This reads 592 files; it belongs with the
other guards in `check.sh`, where CI runs it.

**Nothing, and rely on the reviewer.** This session is the argument against.

## Evidence

Eight mutations, **all caught**, each naming the test that caught it, in
[`ledger/mutations/20260923T165010-scripts-check-retired-claims-py.json`](mutations/20260923T165010-scripts-check-retired-claims-py.json)
— including the one that matters, joining lines before matching, which is
caught by `the claim is found when it wraps`.

The guard caught its own author on its first run: the docstring quotes two
retired phrases as examples, and both were reported until marked. The registry
hygiene rule rejected my own first entry, `eight binaries`, as too short to be
a claim.

`scripts/check.sh` at **51 passed, all of them** (48 before this).

## Three things found on the way, all fixed here

**`check_mutation_claims.py` was run by nothing.** Built this morning in #290;
`check.sh` registered its *test* and not the check. Its test fired, so the
guard was debugged — and the guard was pointed at the repository only when I
ran it by hand. Now registered.

**My test used `!!` where the house style is `FAIL`.** `mutate.py` scored every
mutation `UNREADABLE` rather than caught. This is exactly #261, in a runner
written after #261, by someone who had read it.

**The empty-registry case could not fail.** Its fixture had no scannable file,
so the *other* never-fires guard failed it first and the one under test never
had to. The mutation survived, correctly, and the fixture now has a file to
scan.

## What this does not do

**A claim nobody declares is a claim it cannot see.** This is the whole cost of
the registry, and it means the guard would not have caught any of the three
failures that prompted it at the moment they were made. What a declaration buys
is every later catch — a phrase deleted today and written again next month in a
new file. That is the shape the failure actually takes, but it is a weaker
claim than "this would have caught it".

**Three phrases is not coverage.** Every claim withdrawn before today is
unregistered, and there are entries in this ledger that withdraw a dozen more.
Nothing sweeps them in; each would have to be read and judged distinctive
enough to register.

**Matching is literal.** A retired claim reworded even slightly passes. The
hygiene rule pushes phrases longer, which makes a false positive rare and a
miss more likely, and that trade is chosen rather than measured.

**`NOT_A_CLAIM` is checked within three lines either side of the match**, which
is a guess at how far a marker sits from the thing it marks. A long quotation
with the marker at the top of the paragraph will fail and read as a false
positive.
