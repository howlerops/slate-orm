# The drift test compares the constraints too, and three comments that had gone stale

- **Date:** 2026-10-01
- **Author:** Claude Code (session: finish the backlog, the docs and the examples)
- **Touches:** `examples/helpdesk`, `crates/slate-derive/src/lib.rs`
- **Kind:** fix

## What changed

`the_hand_written_tickets_table_matches_the_derived_one` compares the
**checks** as well as the columns, field by field.

Three comments that stopped being true this morning now say what holds:
`a_status_the_application_does_not_know_is_refused` explained that it was
refusing in the service *because the derive could not declare a check*; the
drift test described the hand-written table as restating columns; and
`#[derive(Record)]`'s own module doc listed the attributes and did not list
`foreign_key`.

## Why

The drift test is the guard that makes duplicating a table declaration a cost
rather than a hazard, and
`ledger/2026-10-01-the-example-says-the-rule-now.md` added a constraint to
both copies without widening it. That left the exact hole the test exists to
close: the derived table and the hand-written one could carry different
rules, and the table the writes use is the hand-written one.

**And comparing the check *lists* would not have been enough.** `CheckDef`'s
`PartialEq` compares the name and nothing else — its own doc comment says so,
and `crates/slate-orm/tests/derive_checks.rs` found it by mutation. So the
comparison is `(name, column, message)` per check, which is the shape that
notices a message dropped or a column pointing at the wrong field.

The three comments are the stale-documentation rule applied to the place it
bites hardest. A comment saying "the derive cannot declare a check, so this
is refused here instead" teaches a reader to write the workaround; and a
module doc listing every attribute except the newest is the list somebody
reads to find out what exists.

## Alternatives rejected

**Leaving the drift test as it was.** It passed. Rejected on what it would
have passed *through*: a check on the derived table and none on
`TICKETS_TABLE` is a constraint no write enforces, and the test's entire
purpose is to notice that class. Mutation proved it — see below.

**Deleting the application check in `open_ticket` and its test.** Now
redundant in outcome, and the comment on that test said so by accident. Kept,
and the comment rewritten to say *why*: the two refusals differ in when they
arrive and in what they carry. A typed `NotAStatus` before any I/O is better
for a caller than a `CheckViolation` after a round trip, and the schema's
rule is what protects a caller who goes around the service.

**Rewriting the comments and not widening the test.** Half the work, and the
half that reads as done. The comments were wrong because the code changed;
the test was weak because the code changed. Fixing only the prose leaves a
tree whose documentation is accurate about a guard that does not guard.

## Evidence

**Two mutations, both caught**
(`ledger/mutations/20261001T162605-examples-helpdesk-src-lib-rs.json`,
`ledger/mutations/20261001T162633-examples-helpdesk-src-lib-rs.json`):

| mutation | caught by |
|---|---|
| the hand-written check loses its `message`, so a form shows nothing | `the_hand_written_tickets_table_matches_the_derived_one`, `the_same_status_written_past_the_service_is_refused_by_the_schema` |
| the hand-written check loses its `column`, which equality alone would not see | the same two |

The second is the one the widening is for: with `CheckDef`'s name-only
equality, a list comparison would have passed it.

**They first scored `NOTHING RAN`** — `linking with cc failed` on the
`over_slatedb` example, which is ENOSPC wearing a compiler error, exactly as
CLAUDE.md describes. `mutate.py` then reported that it could not score the
*restore* either, which is the right refusal: the tree may or may not be
clean and the script will not guess. Checked by hand (`git diff --stat` and
two greps), reclaimed 3.4 GB, and re-ran against `--test helpdesk` alone so
the examples are not built.

**`cargo doc -p slate-derive --no-deps` is warning-free**, which it was not
on the first draft: `[`slate_orm::Record::table_id`]` is an unresolvable
intra-doc link from a proc-macro crate that does not depend on `slate-orm`.

**Not measured.** Three comments and one more comparison in a test that runs
in a millisecond.

## What this does not do

**The drift test still compares what somebody listed.** Columns, key, tenant,
soft delete, version, indexes and now checks — and *not* foreign keys, which
`Ticket` has none of. A key added to `Ticket` and not to `TICKETS_TABLE`
would pass. The general fix is comparing the two `TableDef`s whole, which
`PartialEq` does and which is why the test opens with
`assert_eq!(written.name(), derived.name())` rather than `assert_eq!(written,
derived)` — the two are *meant* to differ by the two text indexes. A
difference-modulo-known-additions comparison is the shape that would close
it and is more machinery than the one line a new field needs.

**Nothing checks that a comment is true.** These three were found by grepping
for the phrases the closed features made false, which finds the sentences
somebody thought to search for — the same ceiling
`ledger/2026-09-29-the-file-named-after-the-guard-carried-the-stale-count.md`
records and the same answer: the sweep finds the rest, when somebody runs
one.
