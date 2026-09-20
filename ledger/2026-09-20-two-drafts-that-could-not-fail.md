# A purge over an index its rows have already left, and two tests that proved nothing

- **Date:** 2026-09-20
- **Author:** Claude, closing the last caveat of the entry before this one
- **Touches:** `crates/slate-kernel/tests/soft_delete.rs`
- **Kind:** process

## What changed

One test. No production code. It took three drafts, and the two that were
thrown away are most of what this entry is about.

## Why

`ledger/2026-09-20-the-other-arm-over-the-same-index.md` closed with:

> **It does not revisit `purge_deleted` against this schema.** … Whether a
> purge over *this* partial index leaves anything behind is untested, and the
> entries it would leak are ones the index does not hold in the first place —
> an inference, not a measurement.

The inference was right and the reasoning behind it was not the interesting
part. `erase_row` filters on `index.admits(row)`, and `write_row_with` says
exactly why:

> The delete would be harmless in itself and is not harmless in a transaction:
> it writes the key, and so conflicts with any concurrent writer of the row
> that really does own that slot.

A purge is where that bites hardest, because *every* row a purge touches is
retired and therefore outside a partial index on `deleted_at IS NULL`. The
existing `a_purged_row_takes_its_index_entries_with_it` runs against a table
whose index does hold the row, so it exercises the other branch entirely.

**The two wrong drafts are the finding.**

**Draft one counted keys.** It asserted the backend held no stray index key
after the purge, and that the total key count moved by exactly one. It passed —
and so did the mutation that deletes the `admits` filter, because *deleting a
key that was never written leaves nothing to count*. A test that cannot
distinguish "the code is right" from "the store forgives it" is not evidence.

**Draft two used two overlapping transactions over the wrong index.** Right
idea — the harm is a conflict, so it needs a writer to conflict with — and it
still passed under the mutation, for a reason worth knowing: a **non-unique**
index entry key carries the primary key, so two rows never share one and there
was nothing to collide over.

**Draft three uses a unique partial index**, where the entry key is the indexed
value alone: the slot one row frees by being retired and another then takes.
The purge and the writer overlap, the writer takes the freed slug, and under
the mutation the purge writes a delete for that exact key and the writer's
commit is refused.

## Alternatives rejected

**Ship draft one and note that the mutation survives.** The repository's rule
is that a surviving mutation is a missing test, to be written rather than
hidden, and this one was writable — it took understanding why, which is the
work. Shipping it would have left a test whose comment described a hazard it
could not detect, which is worse than no test: it looks like coverage.

**Keep draft one *as well*, for the key count.** Two tests, one of which can
only fail if the other does. The count assertion adds nothing once the conflict
assertion exists, and it would go on implying the count is what matters.

**Assert on `MemoryStore`'s write set directly** rather than through a second
transaction. It would catch the mutation without the ceremony, and it would
assert on the store's bookkeeping instead of on the behaviour a caller sees —
and a caller sees a refused commit, not a write set.

**Leave it at the inference.** What the previous entry did, honestly labelled.
The reason not to is that the inference was about `erase_row`'s filter, and the
thing worth protecting is the *interaction*: a purge's rows are always outside a
"live rows only" index, so that filter is load-bearing on this path in a way it
is on no other.

## Evidence

45 tests in `soft_delete`, all green. The mutation — `erase_row` iterating
every index rather than only those that `admit` the row — against each draft:

| draft | result under the mutation |
| --- | --- |
| count the backend's index keys | **passed** — nothing to count |
| two transactions, non-unique index | **passed** — entry keys carry the pk, no collision |
| two transactions, unique partial index | `a_purge_does_not_conflict_with_the_writer_that_took_the_freed_slot` **FAILED** |

`cargo clippy -p slate-kernel --all-targets` clean, `scripts/check.sh` 20/20.

**A tally worth stating plainly, since it is the fourth time today.** Mutations
that survived a first attempt this session: the bulk write arm, the bulk grant
boundary, and this one twice. Every one was in code or a test I would otherwise
have reported as covered. The mutation step is not a formality here; it is the
only thing that has caught any of them.

## What this does not do

**It does not test the same interaction on a `delete_where` or a cascade.**
Both funnel through `remove_row` rather than `erase_row` — they retire rather
than erase — so the filter under test is not on their path. That is why this
test is about a purge specifically.

**It does not cover an expression index**, which `admits` also governs and
whose entry key is computed rather than projected. The same filter applies; no
test exercises it through a purge.

**It says nothing about SlateDB.** `MemoryStore`'s conflict detection is what
refuses the commit here. The real backend is what deployments run, and whether
its conflict detection agrees on this case is inherited from
`tests/concurrency.rs` rather than asserted for this path.

**The row count in draft one was not wrong**, only useless. A purge that
deleted a row's index entries *and* some other row's would still move the total
by more than one, so a count is a real assertion about something — just not
about the hazard the comment names, which was the claim being made.
