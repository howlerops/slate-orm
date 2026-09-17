# The delete that succeeded and reported failure

- **Date:** 2026-09-17
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `crates/slate-kernel/src/{error,record}.rs`, `crates/slate-orm/src/ext.rs`, `crates/slate-server/src/{service,session,status}.rs`, `crates/slate-serverd/src/{config,main}.rs`, `crates/slate-server/tests/returning_cap.rs` (new), `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** fix

## What changed

`Limits.max_returned_rows`, default 10,000: a predicate write that asks for its
rows back and matches more than that is refused **before it writes anything**.
The check lives in the kernel's `matching_rows`, reached through a new
`at_most` parameter on `delete_where` and `update_where`. Nine tests, a daemon
config key, three daemon tests, ten mutations.

## Why

The previous entry's plan listed this as **a hypothesis**, on the strength of a
`grep` showing no message-size limits configured anywhere. It reproduced on the
first try, and it is worse than the hypothesis said.

Eight thousand rows of about a kilobyte each, one `delete_where` with
`returning`:

```text
ERR: code OutOfRange, decoded message length too large:
     found 8423749 bytes, the limit is: 4194304 bytes
rows left in the table: 0
```

**Every row was destroyed and the caller was told the request failed.** The
message names a decode limit and says nothing about the write. A caller reading
the error taxonomy does the right thing with `OutOfRange` — it is not
retryable, so do not retry — and learns nothing about the eight thousand rows
that are gone. We took an outcome the server knew and threw it away.

`WriteResponse` is one message, `returning` puts every matched row in it, and
tonic's default client decode limit is 4 MiB. Nothing between the predicate and
the wire had an opinion about how many rows that is.

## The ordering is the fix, not the number

A cap that fires after the write is no fix at all, so the check has to be
upstream of the first row written, on every path a predicate write can arrive
by: a lone RPC, a batched one, and one inside a caller's open transaction.

`matching_rows` is that place. Its own comment already said so — *"exactly one
place where 'which rows does this touch' is decided"* — and every write in both
methods is downstream of it. Putting the check there also made the scan stop at
`limit + 1` rather than materialising the whole match to report a number
nobody can act on.

**One correction to the plan, which was too weak.** It said refusing before the
commit "is the only ordering that keeps the caller's world consistent". For a
*lone* write that is not quite true: the transaction rolls back on any error,
so even a post-write refusal leaves the table as it was. Inside a caller's open
transaction there is no rollback to lean on — the write sits in their buffer
and they may commit it. A mutation that moves the check after the write loop is
caught by exactly one test, the in-transaction one, and by none of the others.
That asymmetry is the reason the in-transaction test exists.

## Alternatives rejected

**Truncate the returned rows and set a `rows_truncated` flag.** No refusal, no
rollback, no kernel change, and the write still happens. It is the wrong answer
for a delete: the caller asked which rows it destroyed and would get some of
them, with the rest unrecoverable. For the one operation where the answer stops
existing when the write lands, a partial answer is worse than a refusal.

**Bound bytes rather than rows, by encoding the rows before allowing the
commit.** That is the bound that actually matters and it is honest about the
4 MiB. It also means doing the protobuf conversion inside the write
transaction, keeping it, and threading it back out to the handler — a
restructuring of the write path for a limit that a round row count approximates
well. Recorded as where this stops, not as solved.

**Raise the clients' decode limits instead.** Moves the wall. A limit exists
somewhere in any case, and a caller who asks for ten million rows in one
message should be refused rather than served.

**Stream a predicate write's rows, the way `Query` streams.** The general
answer, and a breaking change to the RPC shape in three clients. Scoped out of
this item deliberately and named in the plan.

**Put the cap in `ExecutionLimits` beside `max_sort_rows`.** Tempting, because
it needs no signature change: `ExecutionLimits` is per store. But this cap is
per *request* — it applies only when `returning` was asked for — and a
per-store number cannot see that. Making it per store would refuse a legitimate
uncapped `DELETE … WHERE` over a million rows, which is a thing people do on
purpose.

**A server-side check rather than a `KernelError`.** The refusal has to be
inside the transaction, and the transaction closure's error type is
`KernelError`. A server-side check would also have needed a second
implementation for the session path, which is the two-paths-to-keep-in-
agreement shape this repository keeps rejecting. The kernel already carries
four resource limits of exactly this form (`JoinBuildTooLarge`,
`TooManyGroups`, `TooManyDistinctValues`, `SortTooLarge`), so the variant fits
where it went.

**Test at the real default of 10,000.** A ceiling is only testable by setting
it low enough to reach. `CAP = 3`, and ten thousand and one rows would pin
nothing extra and cost seconds of CI.

## Evidence

Reproduced before the fix, in the numbers above; the reproduction is quoted in
the test file's module comment so the defect is legible from the test that
prevents it.

`cargo clippy --workspace --all-targets` clean with `-D warnings`.
`cargo fmt` on the four crates touched. 51 `slate-kernel` test binaries green,
14 `slate-orm`, 20 `slate-server`, `slate-serverd` green including the three
new config tests. `site/check/docs.py` passes.

**Ten mutations, one survivor, and the survivor was equivalent:**

| mutation | result |
| --- | --- |
| the ceiling is off by one (`>=` → `>`) | **survived** → killed |
| the ceiling never fires | killed |
| the ceiling is checked after the write, not before it | killed |
| a delete refuses only once the rows are already gone | equivalent, see below |
| the ceiling applies whether or not the rows were asked for | killed |
| a lone delete is never bounded | killed |
| a lone update is never bounded | killed |
| a write inside a transaction is never bounded | killed |
| a batched write is never bounded | **survived** → killed |
| an independent batch's write is never bounded | killed |

Two mutations survived the first run and each became a test.

**The off-by-one.** `rows.len() >= limit` weakened to `> limit` allows one row
too many, and every test then written passed with it in place: the
"exactly at the cap" test pins one side of a boundary, which is half a
boundary. `a_match_one_over_the_cap_is_refused` is the other half.

**The batched write in a transaction.** The batch handler has two paths — one
through `Write::apply` for a batch that commits itself, one through the session
actor for a batch joining an open transaction — and only the first had a test.
Setting the second path's ceiling to `None` changed no result until
`a_batched_write_inside_a_transaction_is_bounded` existed. It is the same class
as the in-transaction lone write: two ways to spell a request, one of them
untested.

**The equivalent mutant, stated rather than hidden.** "A delete refuses only
once the rows are already gone" *added* a post-write check while leaving the
real pre-write one in place, so the added code was unreachable. Rather than
call it equivalent and move on, the non-equivalent version was built — the
pre-check removed and the post-check put in its place — and it fails, by name,
in `the_ceiling_holds_inside_a_transaction`. A survivor nobody re-tested is a
survivor nobody has explained.

## What this does not do

**The cap is a row count and the thing that breaks is bytes.** One row holding
a large enough blob passes a cap of 10,000 and fails to encode anyway. The
default is derived from 10,000 × the 256-byte row this node was measured on
against the 4 MiB client limit, which is arithmetic rather than a measurement,
and it is round because it is a guard rather than a tuning knob.

**No client knows the cap.** A caller who asks for 50,000 rows pays the round
trip to be refused, exactly as with `max_batch_operations`. Same reasoning as
before: the limit is configurable, so a client-side copy is a second number
that can disagree with a server it cannot see.

**The memory cost of a predicate write is untouched.** `matching_rows` collects
every matched row whether or not `returning` was asked for, so a `DELETE …
WHERE` over ten million rows still materialises ten million rows. That predates
this change and this change does not help it: `at_most` is `None` for a write
that does not return. It belongs in `ExecutionLimits` as a per-store memory
guard, which is a different limit with a different default, and it is written
up in `docs/orm-comparison.md` rather than done here.

**Nothing measures how much the early stop saves.** The scan stopping at
`limit + 1` is obviously cheaper than materialising the whole match, and
"obviously" is the word doing the work — there is no number.
