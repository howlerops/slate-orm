# The cascade half of the index question, and the premise both halves rest on

- **Date:** 2026-09-20
- **Author:** Claude, finishing a pair the entry before this one left half-done
- **Touches:** `crates/slate-kernel/tests/soft_delete.rs`
- **Kind:** process

## What changed

Two tests and a shared fixture. No production code.

`indexed_filing` builds the schema both arms need — a soft-deleting child whose
*only* index on the referencing column is partial on `deleted_at IS NULL` — so
the `RESTRICT` and `CASCADE` cases ask their opposite questions of one
structure. One new test covers the cascade arm; the other asserts the premise
both of them depend on and neither was checking.

## Why

`ledger/2026-09-20-the-index-that-could-not-hold-it.md` killed the hypothesis
that a partial index could hide a retired child from the `RESTRICT` search, and
closed with two admissions. Both are closed here.

**"It does not test a partial index on the `CASCADE` arm."** That arm reads
with `Deleted::Hidden`, so the conjunct *is* in the filter and the partial
index predicate follows from it — the planner may legitimately use it. Correct,
since a cascade wants live children and nothing else; and "correct by the same
reasoning" was exactly the phrase the previous entry used about the restrict
arm before turning it into a test. Two live-and-retired children, one delete,
and the assertion is that the live one was retired at the cascade's clock and
the already-retired one kept its own.

**The premise nothing asserted.** Both index tests mean nothing unless the
index really does drop the entry when the row is retired — and if maintenance
ever stopped dropping it, the `RESTRICT` test would keep passing, because the
row is found either way. The claim would quietly become untrue with every test
still green. `the_partial_index_holds_a_live_child_and_drops_a_retired_one`
counts the entries out of the backend, the way `constraints.rs` already does
and for the reason it gives: asking the record layer would let a leaked entry
hide behind the same code that leaked it.

That number — `live=1 retired=0` — was in the previous entry's evidence as
output from a scratch probe I then deleted. **Evidence that exists only in a
ledger entry is evidence nobody can re-run.** Worse, I left a comment in the
committed test pointing at a test name that did not exist, which is the stale
reference this repository's rules are about. Writing it for real fixes both.

## Alternatives rejected

**Leave the cascade arm alone: it is correct and the reasoning is short.** The
same argument that left the restrict arm untested this morning, which turned
into a real hypothesis when read twice. The cost of being wrong is not symmetric
— a cascade that missed a live child leaves a dangling reference — and the test
is thirty lines over a fixture that already exists.

**Two fixtures, one per arm.** Simpler to read in isolation and it answers a
question nobody asked: whether *a* partial index is safe for restrict and *a
different* partial index is right for cascade. The whole point is that it is the
same structure, so it is one builder with the action as a parameter.

**Assert the index count through a read with `include_deleted`.** It would say
"the row is still there", which is true and is not the claim. The claim is about
the *index*, and only the backend can answer that.

**Fold the premise into the two tests as a precondition.** Three tests asserting
the same thing, failing together and saying the same thing three times. Its own
test names the property once, and the two that depend on it now cite it by name
rather than by an approximate comment.

## Evidence

44 tests in `soft_delete`, all green. Three mutations, all caught by name:

| mutation | tests that failed |
| --- | --- |
| the cascade arm reads `Deleted::Visible` | `a_cascade_edge_over_the_same_index_still_retires_the_live_child`, `a_cascade_edge_leaves_an_already_retired_child_and_its_timestamp_alone` |
| `remove_row` writes the retired row with no `previous`, so the old index entry is never deleted | `the_partial_index_holds_a_live_child_and_drops_a_retired_one`, `a_partial_index_on_not_deleted_frees_its_entry_when_a_row_is_retired`, `restoring_a_row_whose_unique_slot_was_reused_is_refused` |
| the restrict arm reads `Deleted::Hidden` (from the previous entry, re-run) | `a_restrict_edge_blocks_on_a_retired_child`, `..._the_only_index_cannot_see` |

The second is the one that matters for the premise test: it is the mutation
under which the `RESTRICT` test would have gone on passing while the thing it
demonstrates became false, and three tests now catch it rather than two.

**A process note, because it happened twice today.** Both attempts at these
mutations initially reported a clean run for a patch that had **not applied** —
the anchor string had moved under a `cargo fmt`. The `assert s.count(old) == 1`
in the patch script is what caught it; without it, "no test failed" would have
read as "the mutation survived" and, worse, as evidence. A mutation run that
cannot tell "nothing broke" from "nothing changed" is not a mutation run.

`cargo clippy -p slate-kernel --all-targets` clean, `scripts/check.sh` 20/20.

## What this does not do

**It does not assert which plan either arm chose.** The cascade *may* use the
partial index and the restrict arm *cannot*; neither test says so, because
asserting a plan pins an implementation rather than the property, and the
property is what must hold if the planner changes. Whether the cascade actually
takes the index here is unmeasured — `explain` would say, and the answer would
be a fact about today's cost model.

**No composite or multi-column index**, no expression index, and nothing with
more than one index on the referencing column. One index that excludes the row
is the sharp case; a table with a second, total index on the same column would
give the planner a safe choice and prove less.

**It does not revisit `purge_deleted` against this schema.** A purge erases
retired rows and its own test already checks the entries go with them, on a
different table. Whether a purge over *this* partial index leaves anything
behind is untested, and the entries it would leak are ones the index does not
hold in the first place — an inference, not a measurement.
