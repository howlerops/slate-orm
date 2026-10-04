# The column-grant gaps closed: three SDKs, the wire, a guard, and the RLS battery

- **Date:** 2026-10-04
- **Author:** Claude Code (session: closing what `2026-10-03-column-grants-built.md` left)
- **Touches:** `examples/explorer/head.toml`, the three adapters'
  identity maps, `examples/explorer/CONTRACT.md`,
  `examples/explorer/conformance/conformance.py`,
  `crates/slate-server/tests/common/mod.rs`, `crates/slate-server/tests/security_probe.rs`,
  `scripts/check_concealment.py`, `scripts/test_check_concealment.py`, `scripts/check.sh`,
  `scripts/mutations.json`, `.github/workflows/ci.yml`,
  `crates/slate-kernel/tests/rls_matrix.rs`, `scripts/check_closed_caveats.py`,
  `docs/column-grants.md`
- **Kind:** security

## What changed

The previous entry recorded four caveats as `deliberate`, each with an argument
for leaving it. The owner asked for all of them closed, and each one now is.

1. **Three SDKs.** The explorer has a fourth identity, `analyst`, which reads
   `authors` without `born`. There are three new conformance cases:
   - a default read, which must succeed with `born` null;
   - a filter on `born`, which must be refused;
   - a sort on `born`, which must be refused.

   These go through the Python, Go and TypeScript adapters, and all three
   must produce byte-identical answers.
2. **The wire.** There are two new `security_probe` tests. The first runs a
   self-join, a grouped aggregate, `UpdateWhere` and `DeleteWhere` (both with
   `returning`), and two refused shapes, all through gRPC as a narrowed role.
   Every response is scanned for the hidden value, and the store is read
   afterwards to show the update kept the column its caller could not see.
   The second covers a role that cannot read the column its own row policy
   filters on. That role is `owner_blind`, the one that reaches the cursor's
   concealment over the wire.
3. **A guard.** `scripts/check_concealment.py` rosters every call, in every
   workspace crate, to the five primitives that read a row out of storage
   with every column in it. Each entry carries a reason. It refuses an
   unrostered call, a stale entry, and a scan that finds no calls at all.
4. **The RLS battery.** `rls_matrix.rs`'s twelve access paths now return rows
   instead of titles. A new test runs all twelve as Alice with a column grant
   that leaves out `owner_id`, the column the policy reads. Each path must
   return exactly the rows the policy admits, with `owner_id` null in every
   one.

## Why

These were boundaries argued for, and the owner overruled the argument. Two of
the four turned out to be worth more than the argument allowed:

- **The cursor's concealment was untested over the wire.** The first wire test,
  `a_column_grant_holds_over_the_wire`, used `email_blind`, whose policy reads
  `owner`, a column that role *can* see. Nothing ever decoded a column the role
  could not read, so removing the concealment from `QueryCursor::next` survived
  the whole `security_probe` suite. `owner_blind` exists because of that
  survivor.
- **The RLS battery could not see a leak.** Its paths returned titles, so a
  path that handed out a withheld column with the right rows would have passed.
  Making the paths return rows is what let the battery check concealment at
  all.

## Alternatives rejected

- **A conformance case per path for the analyst.** The runner compares the
  three SDKs to each other, so its value is agreement, and three cases cover
  the three behaviours: narrow, refuse a filter, refuse a sort. The paths
  themselves are the kernel's, tested there.
- **Recognise "returns a row" in the guard.** There is no reliable textual
  answer to that question. Rostering the five row-producing primitives asks
  one that has an answer. Every new way of reading rows has to pass through
  one of them.
- **Hide `score` or `embedding` in the RLS battery.** Several paths filter or
  sort on those columns, so they would be *refused*, which tests the
  refusal (already covered in `column_grants.rs`) and not the concealment.
  `owner_id` is referenced by no path and read by the policy, so every path
  reaches the concealment.
- **Leave the verdicts `deliberate`.** That was the previous entry's position,
  overruled above.

## Evidence

**Three SDKs.** `cd examples/explorer && ./run.sh --conformance` gave "143
cases: the three SDKs agree on all of them". The runner fails a non-refusal
case that all three refuse, so the analyst's read succeeded in each SDK.
Mutations, `ledger/mutations/20261004T165423-examples-explorer-head-toml.json`:

- with the analyst's grant made whole-table, both refusal cases failed with
  "listed as a refusal and all three answered it";
- with `born` added to the analyst's columns, the same two cases failed the
  same way.

On the first run on this machine, the TypeScript client did not build. The
errors were `Buffer` against `Uint8Array`, with no `node_modules` installed;
`tsc` was resolving unpinned versions. `npm ci` against the lockfile fixed it.
That was the environment, not the client.

**The wire.** `security_probe`: 18 passed. Mutations against that suite, in
`ledger/mutations/20261004T165551-crates-slate-kernel-src-record-rs.json`,
`ledger/mutations/20261004T165620-crates-slate-kernel-src-exec-rs.json` (the
survivor) and `ledger/mutations/20261004T165700-crates-slate-kernel-src-exec-rs.json`:

| mutation | caught by |
| --- | --- |
| a predicate write reads concealed rows | `a_column_grant_holds_over_joins_aggregates_and_predicate_writes` |
| `update_where` returns hidden columns | the same |
| `delete_where` returns hidden columns | the same |
| the cursor conceals nothing | **survived**, then caught by `a_policys_hidden_column_is_withheld_over_the_wire` once it existed |

**The guard.** 8 cases in `test_check_concealment.py`, and the real tree
passes: 17 reads across 15 rostered functions. Real-tree mutations through
`mutate_guard.py`, in `ledger/mutations/20261004T170034-crates-slate-kernel-src-record-rs.json`
and `ledger/mutations/20261004T170021-crates-slate-kernel-src-read-rs.json`:
- renaming `read_rows_concurrently` takes its read off the roster, and is
  caught;
- moving `get` off its raw read leaves its entry stale, and is caught.

One of these runs is recorded as interrupted, because my spec placed a
`read.rs` anchor in a `record.rs` run and `mutate.py` refused it
(`ledger/mutations/20261004T170020-crates-slate-kernel-src-record-rs.json`). The case is
re-run cleanly.

**The RLS battery.** `rls_matrix`: 6 passed. Removing the cursor's concealment
is caught by `every_access_path_withholds_the_policys_column`
(`ledger/mutations/20261004T170216-crates-slate-kernel-src-exec-rs.json`).

## What this does not do

- **The guard trusts its reasons.** It checks that every raw read has a reason
  on the roster, not that the reason is true. That is the same bargain the
  `check_handlers.py` rosters make.
- **`RowCursor` remains a raw, unauthorised scan.** It is rostered with its
  reason: its only caller is the ClickBench binary, and nothing on the wire
  constructs one. That predates column grants, and the roster says so instead
  of pretending it conceals.
- **The analyst's cases cover `authors` only.** A column grant on a table with
  a row policy is covered in the kernel and over the wire, but not through the
  three adapters.
- **Column-level writes still do not exist**, by design (§6 of the note).
