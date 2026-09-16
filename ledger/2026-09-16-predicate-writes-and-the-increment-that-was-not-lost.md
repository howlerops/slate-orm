# `delete_where` and `update_where`, so that two concurrent increments make two.

- **Date:** 2026-09-16
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-kernel` (`record.rs`, `error.rs`,
  `tests/predicate_writes.rs`), `crates/slate-orm` (`ext.rs`,
  `tests/predicate_writes.rs`), `docs/orm-comparison.md`,
  `site/docs/roadmap.html`
- **Kind:** feature

## What changed

Two methods on `RecordTransaction`. `delete_where(context, table, predicate)`
removes every row the predicate selects and returns how many.
`update_where(context, table, predicate, assignments)` writes every row it
selects, each assignment being a column and a `Scalar` evaluated over the row as
it was read — so `views = views + 1` is one write. Both are surfaced on the
typed layer as `Records::delete_records_where` and `update_records_where`, where
the column is named through the derive's `COLUMNS` constant rather than by
ordinal.

Two new error variants: `NoSuchColumn` and `DuplicateAssignment`.

This is P1 of the plan written in `docs/orm-comparison.md` earlier today; both
that note and the docs page now say it is built, and say that it does not yet
cross the wire.

## Why

It was the first of the two largest gaps the ORM audit found. `DELETE FROM
sessions WHERE expires_at < now()` could not be expressed: the caller queried
the keys, carried them back and deleted one per key, which is N+1 by
construction and — the part that actually bites — is not atomic with the query
that found them, so a row inserted in between is missed and a row deleted in
between is deleted twice.

The update half is the one worth the work. Without `SET views = views + 1`,
incrementing a counter is read-modify-write, and two callers doing that lose one
increment. `update_if_unchanged` makes the loss *detectable* — that is real
optimistic concurrency and it works — but detecting a lost update and asking the
caller to retry is a worse answer than not losing it.

## Alternatives rejected

**A loop in the record layer instead of a kernel method.** By far the cheaper
build: query, then call the existing keyed `delete` per row. It is the version
that already exists at every call site and it is exactly what this replaces,
because it cannot be atomic with its own scan. A convenience wrapper around the
same race is a worse outcome than no wrapper, because it looks like the fix.

**Streaming the write off the cursor rather than collecting matches first.**
Would bound memory, which is the one real cost of the chosen shape. Rejected on
two grounds: a cursor borrows the transaction so it will not compile, and —
independently — the keyed `delete` already establishes that nothing is written
until the whole consequence is known, because a `RESTRICT` discovered halfway
through would otherwise leave a half-applied delete behind. That argument gets
*stronger* with many rows, not weaker. The memory cost is documented on the
method and the knob is the predicate.

**A private, faster scan for the write path.** Tempting, and it is how the
security hole gets in. The scan goes through `execute`, the same path a read
takes, because that is where the row policy is ANDed into the predicate. A
second scan would be a second place that rule is applied, and the one that got
it wrong would be the one nothing reads. Stated in the method's own comment so
the next person to optimise it meets the argument first.

**Left-to-right assignment.** The other reading of `a = b, b = a`, and the one a
naive implementation produces by evaluating against the row it is building. SQL
takes simultaneous and so does this; the alternative surprises people and has no
compensating advantage.

**Resolving a duplicate assignment instead of refusing it.** Last-wins silently
discards the first, first-wins silently discards the second, and the caller who
wrote both meant something. Refused.

## Evidence

**Fifteen kernel tests and two record-layer tests**, all passing. Two of the
kernel tests are oracles over generated data (64 cases each): `delete_where(p)`
must leave exactly the rows a fold over the generated set with `!p` leaves, and
`update_where(p, n = n*2)` must double exactly the matched ones. The oracle is
computed from the generated data without touching the database, so the two sides
cannot agree with each other about a wrong answer.

**`two_increments_make_two`** asserts all three spellings in one test, because
the value of the new one is only legible beside what it replaces:
read-modify-write across two transactions leaves `n = 1` and says nothing;
`update_if_unchanged` refuses the second write with `RowChanged`; `update_where`
with `n = n + 1` leaves `n = 2`. The first assertion is deliberately an
assertion *that the old way loses an increment* — if that ever stops being true
the test should fail, because its premise has changed.

**Eighteen mutations, no survivors — after four survived the first pass.** Nine
mutations were run twice. The first run caught five and left four alive, and
each of the four was a missing test rather than a false alarm:

- *`delete_where` skips the cascade closure* — nothing covered foreign keys.
  Now `a_predicate_delete_obeys_foreign_keys` asserts both halves: a `CASCADE`
  child goes with its parent, and one `RESTRICT` child refuses the whole delete
  including the rows that had no child.
- *`update_where` evaluates against the row it is building* — the swap test I
  had written could not tell the difference, because none of its assignments
  read a column an earlier assignment wrote. The fixture gained a second `i64`
  column so that `n = m, m = n` is expressible, which is the only assignment
  pair that distinguishes the two readings.
- *`delete_where` does not authorize* — every other test used a context holding
  the grant.
- *`update_where` skips index maintenance* — and this one is worth reading
  twice. The obvious test, reading back through a filter on the indexed column,
  **passed against the broken build**. A scan that fetches the row re-checks the
  predicate against what it fetched, so a stale index entry pointing at a row
  whose value no longer matches is filtered out silently: the executor does its
  job and hides the bug while doing it. Only an index-*only* read, which answers
  from the entry and never fetches the row, has nothing left to re-check
  against. The test now asserts the plan is covering (`plan_projected` →
  `Access::IndexScan { covering: true }`) before asserting the entry is gone,
  because otherwise it proves nothing.

`cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets`
clean; `site/check/docs.py` passes with both documents updated.

## What this does not do

**Nothing crosses the wire.** The proto has no predicate write, so the three
clients still have exactly the problem this closes for Rust callers. That is P3
of the plan and it is the reason this is not the end of the item.

**One table.** No `UPDATE … FROM`, no predicate write across a join.

**The matched rows are held in memory at once.** Documented on both methods. The
daemon bounds per-request work above this layer, and the kernel's knob is the
predicate; there is no `LIMIT` on a predicate write and adding one would raise
the question of *which* rows, which a predicate write has no answer to.

**No `RETURNING`.** Both return a count, not the rows.

**The oracles run 64 cases each**, which is the file's own configuration and not
a soak. The tuple codec's properties get 200,000 in CI; these do not, and the
generated tables are at most fourteen rows — enough to exercise the matched-set
boundary, not enough to say anything about scale.
