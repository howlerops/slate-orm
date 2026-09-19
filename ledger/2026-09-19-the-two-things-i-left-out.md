# The two things the metrics scrape left out, one for a stated non-reason and one labelled a hypothesis

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `crates/slate-serverd/tests/observing.rs`
- **Kind:** fix

## What changed

Two tests, and a second table in the harness fixture.
`a_rolled_back_transactions_rows_never_reach_the_scrape` writes inside a
transaction, rolls back, and asserts the `slate_rows_written_total` family is
absent from `/metrics`. `a_purge_reports_its_own_statement_label` inserts two
rows into a soft-deleting table, retires one, purges, and reads all three
statement labels off the port.

## Why

Both were written down in the previous entry's *What this does not do*, and
both were written down in a way that said they should not stay there.

The first: "Adding it here is three lines and was left out only because the
test is already the longest in the file — which is a preference, not a reason,
and is said so." A stated non-reason is a to-do with better manners.

The second: "The statement label travels the same path as `insert`, so there is
no reason to expect a difference — a hypothesis, labelled as one." This
repository's standing instruction for a labelled hypothesis is to go and test
it.

The first is not a smaller version of the mid-transaction scrape it sits beside.
"Not counted yet" and "never counted" are different claims: the reading before
a commit proves rows are withheld, and only a rollback followed by a scrape
proves they are dropped. The second is the claim an operator relies on when
they read the series as rows that landed.

## Alternatives rejected

**Widen `docs` with a `deleted_at` column.** One table instead of two, and
every row literal in the file is two columns wide — nine tests would have
changed so that one statement label could be reached. A second table is five
lines of TOML and touches nothing.

**Assert `… purge_deleted …} 0` on a node that purged nothing.** Cheaper than
seeding, retiring and purging, and it tests the zero rather than the label: a
purge that erased nothing and a purge that never ran both produce no series at
all here, because the observer is only told about statements that executed.
Seeding two rows and retiring one makes the answer `1`, which distinguishes
"purged the retired row" from "purged everything" — the failure that would look
fine at zero.

**Assert the rolled-back scrape contains no `} 3`.** What a first draft does.
It passes on a body saying `} 30`, and on one where the rows were counted under
a different label. Asserting the family is absent is the stronger statement and
is available here because nothing else in the test writes.

## Evidence

Thirteen tests in `observing.rs`, up from eleven. Two mutations, both caught by
the new test and by nothing else:

| mutation | result |
| --- | --- |
| report the tally on `Rollback` as well as `Commit` | caught — `a_rolled_back_transactions_rows_never_reach_the_scrape` |
| label a purge `"delete"` instead of `"purge_deleted"` | caught — `a_purge_reports_its_own_statement_label` |

The second answers the hypothesis directly: the label does travel the same
path, and a test now says so instead of an argument.

## What this does not do

**The purge test asserts three labels and one node.** `insert`, `delete` and
`purge_deleted` on one table, which is the set this fixture can reach.
`update`, `delete_where` and `update_where` are covered in process by
`transaction_counts.rs` and by `purge_wire.rs`, and not from a socket.

**Neither test scrapes twice.** The rollback one reads after the rollback and
the purge one after the purge; neither establishes a *before*, so strictly they
assert a final state rather than a delta. For a counter that only goes up and a
process that started a moment ago, those are the same thing — said because it
would not be on a long-lived node.

**The fixture's second table exists for one test.** `retired` is two columns
and a soft delete, used by nothing else in the file. That is the cost of not
widening `docs`, and it is the cheaper of the two.
