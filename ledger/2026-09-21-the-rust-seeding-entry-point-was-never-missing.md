# A gap row claimed the Rust library could not seed; it can, in two lines

- **Date:** 2026-09-21
- **Author:** Claude Code (agent session)
- **Touches:** `crates/slate-orm/tests/seeding.rs`, `docs/orm-comparison.md`
- **Kind:** docs

## What changed

`orm-comparison.md`'s "Factories for seed data" row said two things were open:
that nothing generates rows, and that the Rust library has no seeding entry
point. The second is false and is now corrected in both the row and its note.
`crates/slate-orm/tests/seeding.rs` is the demonstration: two tests over a
`notes` table with an `own_rows` policy, one seeding rows for two owners under
`SecurityContext::superuser()` and reading each back as its owner, the other
showing the identical batch refused under an ordinary context.

## Why

The claim was reached by looking for a function named `seed` rather than for
the operation. `slate-serverd --seed` is TOML parsing in front of
`insert_many` under `SecurityContext::superuser()`; `insert_records` is the
typed spelling of the same call and is re-exported from `slate_orm`, as is
`SecurityContext`. So the entry point was not merely present — it was the one
the daemon itself uses, which is the strongest form of "exists".

A gap table that invents a gap is worse than one that misses a real one. A
missed gap is discovered by a user who needed it; an invented one directs work
at building something that is already there, and nobody finds out because the
work produces a second way to do it.

The second test exists because the first is worthless alone. If the policy did
not refuse the same rows under an ordinary context, the superuser argument
would be decoration and the test would pass over a table nothing protects.

## Alternatives rejected

**Correcting the row from the export list.** `SecurityContext::superuser` is
`pub` and re-exported; a sentence citing that would have been cheaper than a
test file and would have been correct. Rejected because "a finding must be
demonstrated" cuts both ways — withdrawing a claimed gap is a finding, and the
export list does not show that a seed actually lands rows a policy would refuse
and that they read back afterwards. It also would not have caught that the
policy has to be a `WITH CHECK` on writes for any of it to mean anything, which
the second test is about.

**Building a `seed()` helper so the row becomes true.** Rejected: the row would
then be closed by adding a synonym for a call that already exists. What the
named tools have and this does not is the *factory* — row generation, id
sequencing, related graphs — and wrapping `insert_records` gets none of it.
That half of the row stands and is still open.

**Leaving the row and noting the correction only in the ledger.** Rejected for
the reason the repository states directly: a stale doc is read as current, and
this one is the document that decides what gets built next.

## Evidence

Both tests pass (`cargo test -p slate-orm --test seeding`, 2 passed). Two
mutations in `crates/slate-kernel/src/security.rs`, both caught, each by the
test that should have caught it:

| mutation | caught by |
| --- | --- |
| `superuser()` sets `bypass: false` | `a_rust_program_can_seed_rows_for_owners_that_are_not_itself` |
| `row_filter_with` returns early for everybody, not only superusers | `an_ordinary_caller_cannot_write_a_row_it_could_not_read` (and the first) |

**A third assertion was written, found dead by mutation, and deleted.** The
refusal test originally also asserted that the refused batch left no prefix
behind. Three mutations of `insert_many` — writing each row as it is decided,
and two narrowings of the pre-check loop — all **survived**: `insert_many`
refuses the whole batch in a pre-check before the write loop is reached, so no
single-point mutation can produce a partial write there. The property is real
and the kernel already pins it in `constraints.rs`'s
`a_bulk_insert_with_one_failing_check_lands_nothing`, which *commits* after the
refusal and then looks — strictly stronger. An assertion no mutation can break
is decoration, so it is gone and the file says why.

The first version of that assertion was worse still: it read the table back in
a *fresh* transaction, which reports empty however the write path behaves
because the refused transaction was never committed.

## What this does not do

It does not build a factory, which is the half of the row that is genuinely
open: nothing generates a plausible row, sequences an id, or builds a graph of
related records, and the two rows in the test are written out by hand.

It does not give the Python, Go or TypeScript clients a seed. That half is
refused rather than open — seeding is a superuser write and `seed.rs` designs a
superuser write path out of the wire — and the note says so.

It does not add a `seed` helper to `slate-orm`. The entry point is
`insert_records` with a superuser context, spelled out at the call site, which
is what the "one grep away" argument in `security.rs` asks for.
