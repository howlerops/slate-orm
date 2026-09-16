# Predicate writes on the wire with `RETURNING`, and eight kernel errors that reached a caller as `INTERNAL`

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `crates/slate-kernel/src/record.rs`, `crates/slate-server/{proto,src/{convert,service,session,status}.rs}`, `crates/slate-orm/src/ext.rs`, new tests in `crates/slate-server/tests/`
- **Kind:** feature

## What changed

`DeleteWhere` and `UpdateWhere` RPCs, with a `returning` flag and
`WriteResponse.rows`. `delete_where` and `update_where` in the kernel now
return the rows rather than a count — `.len()` is the count — because the rows
were already in memory and a caller who named a *condition* has no other way to
learn which rows it hit.

Separately, and found on the way: eight of `KernelError`'s twenty-nine variants
had no arm in `code_for` and reached callers as `INTERNAL`. All eight are
classified, and a guard test asserts every variant is.

## Why

P1 built predicate writes in the kernel and stopped at the wire, so a remote
caller had to query the keys, carry them back and write one per key — N+1 by
construction, and not atomic with the query that found them: a row inserted in
between is missed, and a row deleted in between is deleted twice.

**`RETURNING` is here and not on `Insert`, and that is a finding rather than a
scoping decision.** The plan said "RETURNING returns the rows as written, not a
projection", assuming rows-as-written differ from rows-as-sent. Over this wire
they do not. The proto's `Row` is full width and its nulls are values a caller
meant, so there is no unset column for a `DEFAULT` to fill — `insert_partial`
is the only place a default is applied and no handler calls it — and there is
no auto-increment, no trigger and no generated column. `RETURNING` on an insert
would hand the caller its own request back.

A predicate write is the opposite. The caller named a condition, and for a
delete the answer stops existing the moment the write lands, so no read
recovers it. `a_delete_returns_the_rows_it_destroyed` is the test that could not
have been written any other way.

## Alternatives rejected

**A `filter` field on `DeleteRequest`.** No new message. One message meaning
"these keys" or "this condition" depending on which field is set is one message
with two shapes, and the field that is *not* set is the one that decides.

**`RETURNING` on `Insert` and `Update` anyway, for symmetry.** It would echo the
request. Offering a feature that returns no information is worse than not
offering it, because a caller reasonably assumes it does something — and the
day a `DEFAULT` *can* be applied over the wire, the echo silently becomes
correct and nobody notices it was ever wrong.

**Keep `delete_where -> usize` and add `delete_where_returning`.** Two paths
through the same code, to be kept in agreement. The rows cost nothing to return
— they are already collected — so the count is the derived value, not the other
way round.

**Return the rows from the typed `Records::delete_records_where` too.** It
would make the record layer symmetric with the kernel. A typed caller pays a
decode per row, and one that asked "how many" should not. Left returning
`usize`, with the reason at the call site.

**Classify only the two variants this commit made reachable.** That is what the
`InvalidCursor` fix did last commit, and it is why this commit found six more.
Fixing the class rather than the instance is the difference between two
findings and one.

**Make `code_for` exhaustive so the compiler forces the decision.** The right
fix and not available: `KernelError` is `#[non_exhaustive]`, so a downstream
crate must end in a wildcard. Hence a test instead.

## Evidence

Twelve tests in `predicate_wire.rs`, fifteen in the kernel's `predicate_writes`
(updated for the new return type), one guard. `cargo clippy --workspace
--all-targets` with `-D warnings` clean; `cargo fmt --all` clean.

Mutation run, eight mutations over two rounds:

| mutation | first run | after |
| --- | --- | --- |
| `returning` ignored, rows always returned | killed | killed |
| `returning` ignored, rows never returned | killed | killed |
| an absent filter selects nothing | killed | killed |
| an update with no assignments is allowed | killed | killed |
| an assignment with no value is silently skipped | **survived** | killed |
| the schema claim is dropped | **survived** | killed |
| a duplicate assignment is `INTERNAL` again | killed | killed |
| a predicate delete is authorized as a read | **survived** | *stays* |

Two survivors became tests. The third is defence in depth and correctly
unobservable: `delete_where` authorizes `Delete` again in the kernel, so
weakening the handler's check to `Read` changes no answer. It stays, with a
comment saying it catches nothing and why it is still there.

A ninth mutation — "update_where returns the rows as they were, not as
written" — needs two edits at once, which the harness does one at a time, so it
was applied by hand: killed by `an_update_assigns_over_the_row_as_it_was_read`
and `assignments_are_simultaneous_not_sequential`.

**The eight unclassified errors are the finding worth reading.** Last commit
fixed `InvalidCursor` reaching callers as `INTERNAL`, noting it had been
unreachable from the wire until then. That framing was too kind. Scanning all
twenty-nine variants found eight on the wildcard, and three of them —
`SortTooLarge`, `TooManyGroups`, `TooManyDistinctValues` — are reachable from
**any sorted or grouped query** and have been since those went on the wire. A
caller that exceeded a server limit got `INTERNAL`, which reads as "the server
broke" and invites a retry that exceeds the same limit again. They were found
by a different variant landing on the wildcard, which is to say by luck.
`every_error_is_classified.rs` is the guard, and it was checked by deleting an
arm: it names the variant.

The remaining five were unreachable and are classified anyway — `RowChanged` to
`ABORTED` beside `TransactionConflict`, `MigrationRefused` and
`TransactionPoisoned` to `FAILED_PRECONDITION` — so that whoever puts
conditional writes or migrations on the wire does not rediscover this the way
this commit did.

*Also closed:* the previous entry's caveat that `slate-tuple`, `slate-schema`,
`slate-derive` and `slate-slatedb` had not been re-run after the kernel gained
a field. CI run 103 ran `cargo test --workspace` on that commit and passed all
sixteen jobs.

## What this does not do

No client has these yet. The RPCs and the wire tests exist; Python, Go and
TypeScript cannot call them, and the conformance runner does not compare them.
That is the next commit, and until it lands this is a server feature with no
users — which is exactly the shape the `Related` RPC was in for one commit.

`RETURNING` is all-or-nothing: the rows as written, never a projection of them.
A caller that wants one column of a million deleted rows gets a million whole
rows. The plan said so and it is still true, and it is a real cost now that the
rows can be large.

The rows are materialised whether or not `returning` is set, because
`delete_where` collects every matched row before writing anything. That is
pre-existing — a predicate write has always held its whole match set in memory
— and `returning` only decides whether they are encoded and sent. A
`DELETE FROM huge_table` is still bounded by memory, and nothing here changes
that or measures where the bound is.
