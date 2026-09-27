# The purge's contract was documented in five places, all of them saying two grants, and it takes three

- **Date:** 2026-09-27
- **Author:** Claude Code, closing an open caveat from
  `ledger/2026-09-19-who-may-see-a-deleted-row.md`
- **Touches:** `crates/slate-kernel/tests/soft_delete.rs`,
  `crates/slate-server/proto/slate/v1/records.proto`, the Go and Python
  generated stubs, `clients/go/slate/predicate_write.go`,
  `clients/typescript/src/client.ts`, `clients/python/src/slate/client.py`,
  the three SDK READMEs, `site/docs/clients.html`
- **Kind:** a test, and a correction it found

## What changed

Two tests in `soft_delete.rs` cover the grants `purge_deleted` needs, and every
place that documents them now says three rather than two.

`a_purge_needs_all_three_grants_and_says_which_is_missing` grants each proper
subset in turn and asserts the refusal names the missing action *and* that
nothing was erased on the way to refusing.
`a_purge_holding_all_three_grants_erases_the_retired_row` is the other half,
without which both refusals are satisfied by a purge that refuses everybody.

## Why

The caveat said the contract had changed an hour before it was written and that
nothing tested it — every purge test runs as a superuser, and
`SecurityCatalog::authorize` returns early for one, so no grant is consulted at
all.

The test found the contract is not what any of the documentation says. Granting
exactly `delete` and `read_deleted` is **refused, naming `read`**:

```
holding [Delete], expected a refusal naming read_deleted,
got AccessDenied { table: "docs", action: "read" }
```

`purge_deleted` authorises `Delete`, then runs a query. The query authorises
`Read` like any other read, and `read_deleted` is only consulted once
`include_deleted` is in scope. So the order is `delete`, then `read`, then
`read_deleted`, and an operator who grants the two that were written down is
refused with a message about a third they were never told about.

Five places said two: the proto's `PurgeDeleted` comment — which generates into
the Go and Python stubs, so it was really seven — the hand-written docstrings
in all three clients, and the sentence added to `site/docs/clients.html` earlier
today. All now say three, and the proto comment says why it changed, because an
operator granting what a comment lists and being refused has no way to tell an
incomplete list from a bug.

## Alternatives rejected

**Make `purge_deleted` authorise only `Delete` and `ReadDeleted`, so the
documented contract becomes true.** Tempting, and wrong: it would mean a purge
reading rows from a table the caller may not read, which is a strictly larger
hole than an under-documented grant. The scan is a read and should authorise as
one.

**Say "and whatever a read needs" rather than naming `read`.** Accurate and
useless at the moment it is read, which is when somebody is writing a grant.

**Assert only that a refusal happens.** One line shorter and it would have
hidden the finding completely: a refusal naming `read` satisfies "it refused",
and the whole value here is in *which* action the message names.

**Leave the generated stubs and change only the hand-written docs.** The
generated comment is what a Go or Python caller reads in their editor. Both
generators ran; `GOTOOLCHAIN` did its job and pulled go1.26.8 for
`protoc-gen-go-grpc@v1.6.2`, which is the note added to `CLAUDE.md` yesterday
working.

## Evidence

- `cargo test -p slate-kernel --test soft_delete -- a_purge` — 8 passed, 0
  failed (6 before).
- The refusal chain was read off the failures, not from the source: granting
  `[Delete]` names `read`; granting `[Delete, Read]` names `read_deleted`;
  granting `[Read, ReadDeleted]` names `delete`. Each is a case.
- One mutation via `scripts/mutate.py`, recorded as
  `ledger/mutations/20260927T025819-crates-slate-kernel-src-record-rs.json`:
  dropping `authorize(context, table, Action::Delete)` from `purge_deleted` is
  caught by `a_purge_needs_all_three_grants_and_says_which_is_missing`. Before
  these tests, that line was authorising nothing any test could see.
- `python3 site/check/docs.py` — the site holds together.
- Both proto generators re-run; the Go and Python stubs carry the corrected
  comment.

## What this does not do

**The refusal order is now asserted, and it is an implementation detail.** A
caller holding none of the three is told about `delete` first because that is
the line that runs first. That is stable only as long as the authorisation
order is, and a future rearrangement would fail these tests for a reason that
is not a defect — the cost of asserting which action is named, taken knowingly
because the alternative hid the bug this found.

**Nothing checks the documentation against the code.** Five places agreed with
each other and disagreed with the kernel, which is exactly what a
single-source-of-truth would prevent — and the single source would have to be a
machine-readable list of the actions each RPC authorises, which does not exist.
What caught it was a test, and only because it asserted the action's name.

**The wire path is still superuser-only in its own tests.** `purge_wire.rs`
drives `PurgeDeleted` through a real server and does it as a caller with
everything, so the daemon's translation of a kernel `AccessDenied` into a
`PERMISSION_DENIED` status is still untested for this RPC specifically. The
kernel half is what the caveat named.
