# The child that made the arm reachable

## What changed

`common::mentions()` — a new fixture table, a child of `docs` declared
`ON DELETE RESTRICT`, with an `app` grant and nothing else referring to it.
Three tests in `crates/slate-server/tests/transaction_counts.rs` that it makes
possible, covering the plain (non-conditional) delete arm of the session actor:
a refusal that removed nothing, a refusal that removed one row first, and a
success over a mix of present and absent keys.

Two ledger entries from yesterday are marked closed, and one of them has a
correction attached to the alternative it rejected.

## Why

Yesterday's `Tally::applied` change gave all seven write arms one rule about
what a refused statement contributes. Six arms ended up pinned by tests. The
seventh — the plain delete loop — could not be reached: an absent key answers
`Ok(false)` by design rather than failing, and an unauthorized delete is refused
before the session task is ever dispatched to, which I measured by mutating the
arm and watching that case stay green.

I wrote that hole up twice and estimated it would cost a foreign key on the
shared `docs` fixture, which most of this crate's twenty-four test binaries
build rows against. That estimate was the reason I stopped, and it was wrong.

**A foreign key that makes a `docs` row undeletable does not have to be declared
on `docs`.** It can be declared on a *child of* `docs`, and a child with no rows
in it constrains nothing. I rejected the hole on the cost of the option I
happened to think of, which is a different mistake from judging a cost
correctly — and the kind that only shows up when somebody makes you go back.

## Alternatives rejected

**A foreign key on `docs` itself**, the option yesterday's entry named and
priced. Still rejected, and for the reason given then: it changes what every
test in the crate may write. The new child is strictly cheaper and buys the same
failure.

**Reuse an existing table as the child** — `sales` already references `books`,
so a `books` delete can be made to fail. Rejected because `transaction_counts.rs`
writes to `docs` throughout and its helpers (`insert`, `doc`, `kind_is`) are all
shaped for it; routing three tests through a second table to avoid nine lines of
fixture would make them harder to read than the thing they test. A new child of
the table the file already uses keeps every test in one vocabulary.

**A storage-fault injection to make `transaction.delete` fail.** The kernel has
a fault-injection harness (`slate-kernel`'s object-store layer) and it would
reach the arm without any schema change at all. Rejected because it makes the
test about the injector rather than about the counting rule, and because the
failure it produces is not one a caller can cause — a referential violation is
the realistic shape of "this delete was refused mid-loop".

**Leave it closed-by-documentation**, which is where yesterday left it. The
entry was honest and the comment beside the tests was accurate. But an accurate
note saying "this branch is untested" is still a branch that a future change can
break silently, and the cost turned out to be one table.

**Skip the third test** (`a_plain_delete_counts_the_keys_that_were_there`). It
is not about a refusal at all and was not in the task. Kept, because the
mutation that found it is the repository's own standard: making the loop count
`Ok(false)` alongside `Ok(true)` left all fifteen other tests green, and a
mutation that causes no failure is a missing test.

## Evidence

**Five mutations on the plain delete arm, each caught by a named test**, each
restored and re-verified:

| mutation | test that failed |
| --- | --- |
| `tally.applied(…)` → the unconditional `tally.add(…)` | `a_refused_plain_delete_that_applied_nothing_contributes_no_series` |
| pass `0` instead of `affected` when the loop failed | `a_refused_plain_delete_reports_the_rows_it_did_remove` |
| skip the tally entirely when the loop failed | `a_refused_plain_delete_reports_the_rows_it_did_remove` |
| count `Ok(false)` as affected alongside `Ok(true)` | `a_plain_delete_counts_the_keys_that_were_there` |
| never increment `affected` | both of the above |

Before this change, the first three of those left all thirteen tests in the file
green — that is what "unreachable" meant in practice.

**The fourth mutation found a gap nobody had named.** Counting an absent key
survived every test in the file. `a_statement_that_matched_nothing_still_reports_its_zero`
looks like it covers the same ground and does not: it goes through
`delete_where`, which takes its count from the rows it returns rather than from
this loop. The distinction is not only arithmetic — a row the policy hides
deletes as `Ok(false)`, deliberately, so a count that included absent keys would
leak exactly what that `Ok(false)` exists to withhold.

**Blast radius, measured rather than assumed.** `cargo test -p slate-server
--no-fail-fast`: 24 test binaries, all ok, none failed, with the new table in
the shared catalog. `cargo test -p slate-serverd --no-fail-fast`: 8 binaries,
172 + 3 + 3 + 13 + 7 + 19 + 42 + 2 passed, none failed. `sh scripts/check.sh`:
19 of 19.

**A note on the run.** The serverd suite first died with ENOSPC mid-link — the
condition `CLAUDE.md` warns reads as a compiler failure. Reclaimed by removing
`target/debug/incremental` and `target/debug/examples`, which took the tree from
full to 7.3G free, and the suite then passed. No code was involved and nothing
about the change caused it; recorded because the first output looked like a
build break and was not.

## What this does not do

- **`transaction.delete` can still fail in ways no test produces** — a storage
  error, a fence, a cascade over the limit. The arm's error *branch* is now
  exercised, which is what the counting rule needed; the arm is not proved
  correct under every error the kernel can raise.
- **`mentions` is a test fixture and nothing reads it.** It has no index, no
  policy, no conformance case and no client. It exists to make one row
  undeletable. If a later change wants a realistic child table, this is not it.
- **No measurement of the cost it adds.** Every `docs` delete in the crate now
  walks `catalog.referencing(DOCS)` and finds no rows. That is one extra scan
  per delete in tests only, and I did not time it — the suites' wall time was
  not visibly different, which is an impression, not a number.
- **The other six arms were not re-mutated.** They were pinned yesterday and
  nothing here touched them.
