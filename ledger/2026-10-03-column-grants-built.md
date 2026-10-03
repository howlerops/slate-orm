# Column grants, built: a narrowed reader is refused what it references and denied what it did not ask for

- **Date:** 2026-10-03
- **Author:** Claude Code (session: picking up from the handoff prompt, task T4)
- **Touches:** `crates/slate-kernel/src/security.rs`, `crates/slate-kernel/src/read.rs`,
  `crates/slate-kernel/src/exec.rs`, `crates/slate-kernel/src/plan.rs`,
  `crates/slate-kernel/src/record.rs`, `crates/slate-kernel/src/error.rs`,
  `crates/slate-kernel/tests/column_grants.rs`, `crates/slate-server/src/status.rs`,
  `crates/slate-server/tests/common/mod.rs`, `crates/slate-server/tests/security_probe.rs`,
  `crates/slate-serverd/src/config.rs`, `crates/slate-serverd/src/security.rs`,
  `docs/column-grants.md`, `docs/views.md`, `docs/orm-comparison.md`, `docs/agent-handoff.md`
- **Kind:** security

## What changed

`docs/column-grants.md` was built, in the kernel and in the daemon's
configuration.

- **Grants.** `Grant::read_columns(role, table, columns)` grants `Read` on
  those columns only. It refuses a set that leaves out any primary-key column,
  or that names a column the table does not have.
- **What a caller may read.** `SecurityCatalog::readable` is the union of the
  caller's grants. It is `None` (everything) when any grant they hold is
  table-level, when their column list happens to cover every column the table
  has today, or when they hold no column grant at all.
- **Narrowed readers.** For a caller whose reads are narrowed:
  - **References are refused.** A query that references a hidden column
    anywhere — projection, filter, sort, window, computed value, and through
    those, grouping, aggregates and join keys — fails with `ColumnsWithheld`.
  - **"Every column" is narrowed** to the readable ones.
  - **Rows are concealed.** Everything decoded but not readable — chiefly a
    row policy's own columns — is nulled in `QueryCursor::next`, after the
    residual, sort and windows have used it.
  - **`get` is concealed** in the same way.
  - **Some operations are refused outright.** `EXPLAIN`, `analyze`, the
    whole-row writes (`update`, `upsert`, `update_many`, `upsert_many`) and
    both `_if_unchanged` forms are refused for a narrowed reader.
  - **Predicate writes read whole rows.** `delete_where` and `update_where`
    read through a new `execute_for_write`, which still refuses references to
    hidden columns but returns whole rows. Each conceals only the rows it
    returns. `update_where` also refuses an assignment whose value reads a
    hidden column.
- **The wire.** `ColumnsWithheld` is `PERMISSION_DENIED` with reason
  `COLUMNS_WITHHELD`. `InvalidGrant` is `FAILED_PRECONDITION`.
- **serverd.** It takes `columns = [...]` on a `[[security.grants]]` block.
  It refuses at load a column grant that:
  - names more than one table;
  - grants anything but `read`;
  - names a column the table does not have;
  - omits part of the primary key;
  - sits beside `explain` without a whole-table read.

## Why

The handoff named this the real multi-tenant blocker, and the design note
argued the shape. Building it surfaced four things the note had not said, and
each is now in the code or in the note.

1. **`update_where` and `delete_where` would have corrupted data.** Both read
   their matching rows through the ordinary read path and then write them
   back: `update_where` stores them with assignments applied, and
   `delete_where` erases their index entries by their values. Concealment on
   that path would have nulled the hidden columns in storage, and left
   dangling index entries for them. The mutation that routes the write path
   back through the concealing read is caught by
   `a_narrowed_delete_where_leaves_no_index_entry_behind`, through an index on
   the hidden column.
2. **"Reads nothing" is not "reads some".** The first `readable` returned an
   empty set for a role with no `Read` grant. That turned every write-only
   role into a narrowed reader and refused it the whole-row updates it has
   always been allowed. No test of mine caught this. It was the existing
   `security_probe::each_handler_authorizes_the_action_it_performs`, which
   holds an `updater_only` role.
3. **The tenant column needed no rule.** The schema refuses a tenant column
   that does not lead the primary key, and a column grant must include the
   whole key, so the explicit rule I wrote was dead. The test for it could not
   build its fixture.
4. **§7 of the note was withdrawn.** The note had decided that a hidden column
   should be indistinguishable from a missing one. That needs the caller's
   context in the stateless converters that resolve column names on the wire.
   It is a large change, to hide column names that the schema already gives
   every client. The note now strikes the section through and says why.

## Alternatives rejected

- **Conceal in the existing `transient` nulling.** One mechanism instead of
  two. But `transient` runs *before* the residual, and a row policy may read
  exactly the column being hidden. That would filter every row out, or
  evaluate the policy against nulls.
- **Conceal at each API surface** (`execute`, `join`, `aggregate` and the
  rest). Every join side, chain step and probe already reads through
  `QueryCursor::next`, so one point in it covers all of them. Many call sites
  would be many chances to miss one.
- **Let the predicate writes conceal and re-read hidden values before
  writing.** That is a second read per row, to undo a nulling that should not
  have happened.
- **Drop the narrowing of `Projection::All`.** A mutation removing it changed
  no test outcome, because the cursor conceals what it would have avoided
  decoding. It was kept on a measurement instead: 5,000 rows with a 10 KB
  hidden column, 15 runs each, took 25.9 ms median (24.0–29.6) narrowed
  against 40.6 ms (40.2–43.3) decoding everything. It is recorded as an
  expected survivor with that reason.
- **Build §7 as designed.** See point 4 above.
- **A new error variant for refused whole-row writes, separate from refused
  references.** One variant, `ColumnsWithheld`, carries an `action` string
  saying which operation needed which columns. Reusing `AccessDenied` instead
  would have told a role that holds `Update` that "no role grants update",
  sending the reader to the wrong fix.

## Evidence

**The sentinel oracle**, `no_hidden_value_reaches_a_narrowed_reader_by_any_path`.
Hidden columns hold values that occur nowhere else in the fixture. The test
runs every path in its battery as the narrowed role:

- every column, a filter, a sort with a limit, and a named projection;
- a computed value, a window, an `IN` over the key, and a hint onto the hidden
  column's own index;
- paging, `get`, count and sum, and grouping;
- a join under all three algorithms;
- six refused queries, and `explain` and `analyze`.

It then scans the `Debug` text of everything returned, errors included, for
the sentinels. None appears. The same scan over a whole-table reader's results
finds them, which shows the store actually holds them.

**End to end over gRPC**, `a_column_grant_holds_over_the_wire`: `Query` and
`Get` return the row without `email`, and a filter on `email` gets
`PERMISSION_DENIED` with `COLUMNS_WITHHELD`.

**Mutations**, every rule:

| record | cases | caught |
| --- | --- | --- |
| `ledger/mutations/20261003T232944-crates-slate-kernel-src-security-rs.json` + `ledger/mutations/20261003T233129-crates-slate-kernel-src-security-rs.json` | 7 | 7 |
| `ledger/mutations/20261003T231622-crates-slate-kernel-src-read-rs.json` | 10 | 9 |
| `ledger/mutations/20261003T231839-crates-slate-kernel-src-read-rs.json` | 1 | expected survivor |
| `ledger/mutations/20261003T231704-crates-slate-kernel-src-exec-rs.json` | 1 | 1 |
| `ledger/mutations/20261003T231905-crates-slate-kernel-src-record-rs.json` | 11 | 11 |
| `ledger/mutations/20261003T232659-crates-slate-serverd-src-security-rs.json` + `ledger/mutations/20261003T232750-crates-slate-serverd-src-security-rs.json` | 6 | 6 |
| `ledger/mutations/20261003T232801-crates-slate-server-src-status-rs.json` | 2 | 2 |

Notes on the records:

- The one `read.rs` survivor is the narrowing, measured and explained above.
- The first `security.rs` run is not cited. It ran on code from before the
  `readable` fix in point 2.
- `ledger/mutations/20261003T232659-crates-slate-serverd-src-security-rs.json`
  holds one case that did not compile. `mutate.py` refused to score it, and it
  is re-run in `20261003T232750`.
- `ledger/mutations/20261003T232944-crates-slate-kernel-src-security-rs.json`
  ends `interrupted`. It is not a crash: `cargo fmt` had rewrapped two
  anchors, and `mutate.py` refused rather than run a suite against unmutated
  code. My output filter hid its message for one run.
  `ledger/mutations/20261003T233031-crates-slate-kernel-src-security-rs.json`
  is that refusal again, before the anchors were fixed.

**CI's clippy found one thing this machine's could not.** Run 37162597902 failed
`question_mark` (clippy 1.99; this container's does not have the lint) on the
`match` in `readable`. Its suggested `&grant.columns?` would not compile — it
moves out of a borrow — so the fix is `grant.columns.as_ref()?`, and the
mutation of that rewritten line (`let ... else { continue }`, the table grant
no longer settling it) is caught again by
`a_table_grant_beside_a_column_grant_reads_everything`, in
`ledger/mutations/20261003T235020-crates-slate-kernel-src-security-rs.json`.

**Regression:**

| crate | passed | failed |
| --- | --- | --- |
| `slate-kernel` | 730 | 0 |
| `slate-orm` | 152 | 0 |
| `slate-server` | 337 | 2, both fixed and since passing (see below) |
| `slate-serverd` | 323 + 12 security | 0 |
| `slate-wasm` | 236 | 0 |
| `slate-sql` | 73 | 0 |

The two `slate-server` failures were `every_error_is_classified`, which wanted
`InvalidGrant` classified, and the `security_probe` regression from point 2.

## What this does not do

- **No guard requires a new row-returning path to conceal.** Concealment lives
  in `QueryCursor::next`, in `get`, and in the two predicate writes. A future
  path that hands out rows any other way would be caught only by the oracle,
  and only if it is added to the battery.
- **The three clients were not run against a narrowed identity.** No client
  API changed, and one gRPC test covers `Query`, `Get` and a refused filter.
  The conformance runner across Python, Go and TypeScript, which is step 5 of
  the note, was not done.
- **Joins, aggregates and the predicate writes are not covered over the
  wire.** The kernel tests cover them. The gRPC test does not.
- **The oracle is one battery, not every existing suite.** The note asked for
  the sentinel scan across the RLS matrix, join, chain, window and
  nearest-neighbour suites. This runs a battery written for it.
- **Column-level writes do not exist.** A column grant is `read` only, by
  design (§6 of the note).
