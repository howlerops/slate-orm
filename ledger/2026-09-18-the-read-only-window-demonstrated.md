# The read-only migration window was described in a ledger entry and exercised by nothing; it now has a test that shows a query returning no rows

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `crates/slate-serverd/tests/process.rs`
- **Kind:** fix

## What changed

One test. No production code.

`a_follower_warns_and_serves_nothing_through_an_index_the_leader_never_built`
starts two nodes over one directory with different catalogs — the leader's
without `by_size`, the follower's with it — and shows three things: the
follower warns on startup, an unhinted read is unaffected, and a read hinted at
the index the keyspace does not hold comes back **empty with no error**.

## Why

`ledger/2026-09-15-the-guard-nothing-called.md` says it plainly, under what
that change did not do:

> **The warning is not tested.** The two process tests cover the writer path.
> The read-only path needs a second node, a shared backend and a lease contest,
> which the `slate-server` suite has machinery for and this file does not. It
> is the least-exercised branch in the change.

It is also the branch that guards the worst failure in the daemon: a read
through an unbuilt index returns no rows rather than an error. A warning
nothing exercises is a warning that can stop being emitted without anyone
noticing, and the only thing standing between an operator and a silently empty
answer is that warning reaching their terminal.

The machinery the entry said this file lacked had since arrived —
`local_with_a_replica` and the two-node test beside it were written for the
read-only *serving* path. Building the window on top of them is half a day's
work that the earlier entry had priced at more.

## Alternatives rejected

**Close the window instead of testing it.** The tempting one, and I came to
this item meaning to. Two designs were considered. The previous author's —
the follower waits for the leader — they rejected for good reasons: it needs a
way to tell "has not migrated yet" from "there is no leader", and inventing one
in a startup path gets you a node that hangs instead of one that is briefly
wrong. Mine was different: build the follower's *serving catalog* without the
unbuilt indexes, so the planner cannot choose one and the query falls back to a
scan — always right, merely slow.

Rejected, for now, on a trade the code as it stands wins: today the node is
wrong in the window and becomes **right and fast on its own** when the replica
catches up. With the catalog trimmed at startup it would be right throughout
and stay **slow until somebody restarts it**, because the serving catalog is
fixed when the process starts. Making it re-verify and swap back is real
machinery — a background check, a catalog behind a lock, and a new class of
"the plan changed under a running query" question. That is a design note
somebody should write before any of it is built, and writing it was not this
change. The test is what makes either version arguable from evidence.

**Assert only the warning, not the empty answer.** Smaller and it is what the
earlier entry asked for. Rejected because the warning's *text* is the claim —
"reads through an unbuilt index return no rows rather than an error" — and a
test that checks the string without checking the behaviour pins the sentence
rather than the fact. If the behaviour ever changes, the assertion on the rows
is what says the sentence needs rewriting.

**Let the planner choose the index rather than hinting it.** More realistic.
Rejected because it does not reproduce: on a four-row fixture the planner
correctly costs a table scan cheaper and returns the right answer, so the test
would pass for a reason unrelated to the window. That is the same trap that
made a plan assertion useless two commits ago, and the hint is how this one
avoids it.

## Evidence

**The test fails without the warning.** Replacing the `verify_of` check in
`main.rs` with `if false` — the node stops warning — turns it red. So the
assertion is load-bearing rather than incidental.

**The empty answer is real and is not an error.** The hinted read returns
`Ok` with zero rows, on a table where the same node answers the *unhinted*
form with all three. Both assertions are in the test, which is what makes it a
demonstration rather than a statement: an implementation that refused the
hinted read, or that served it from a scan, would fail on a different line each
time.

**A dead end worth recording.** The first version asserted the seeded rows
unpinned and got an empty answer from the *leader*. That is not the window — it
is the ten-second `catch_up` in `local_with_a_replica` and a replica that has
not polled. Every read in the test now names a sequence from the leader's own
commit, which is the same fix the neighbouring two-node test already carried
and which I should have copied rather than rediscovered.

`cargo test -p slate-serverd --no-fail-fast` and `cargo clippy -p slate-serverd
--all-targets`: green.

## What this does not do

- **The window is still open.** This is a test, not a fix. The README item
  stays open and now points at a demonstration instead of a description.
- **It tests one shape of unmigrated.** A missing index, which is the case that
  returns no rows. A follower whose catalog disagrees about a *column* is
  refused by the fingerprint or by the catalog load long before this; a
  follower mid-backfill on an index that exists but is partly built is a third
  case and is not constructed here.
- **Nothing exercises the recovery.** The warning says the node "becomes
  correct on its own once the node holding the writer lease migrates and this
  replica catches up", and no test watches that happen. It would need the
  leader restarted with the wider catalog and the follower re-read, which is a
  longer test than this one and a different claim.
- **Two nodes, one machine, a local directory.** The lease contest is a file
  lock rather than a conditional PUT, so this says nothing about the window's
  shape against real object storage, where the leader's migration and the
  follower's view of it are separated by a manifest poll.
