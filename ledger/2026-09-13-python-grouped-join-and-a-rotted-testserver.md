# A grouped join in Python, and the test crate that rotted outside the workspace

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `clients/python/src/slate/{query,client,__init__}.py`, its generated
  stubs, `clients/python/testserver/src/main.rs`, `clients/python/tests/`
- **Kind:** feature + repair

## What changed

`GroupedJoinQuery` in the Python client, and `sort`/`limit`/`offset` over groups
on both grouped shapes. Six tests for them. Separately, three repairs to
`clients/python/testserver`, which did not build.

## Why

The wire grew a grouped join and ordered groups last commit; the Python client
is the reference client and did not have them. Worse, its module docstring
*asserted their absence* — "there is no ordering over groups to limit" — which
is the stale-documentation failure this repository treats as worse than no
documentation, because it is read as current.

## What was actually broken

Running the Python suite to check my change found it already red, for three
reasons that had nothing to do with each other and everything to do with one
structural fact: **`clients/python/testserver` is a separate cargo workspace, so
`cargo test --workspace` at the root never builds it.** 930 root tests passing
said nothing about it.

1. `TcpListenerStream is not an iterator` — the TCP_NODELAY commit added a
   `.map()` over the listener stream without importing `StreamExt`. That commit
   was never compiled against this crate.
2. `DuplicateIndexId` at startup — index ids became unique per *catalog* rather
   than per table, and this crate gives four indexes id 1. Renumbered.
3. Every `EXPLAIN` test failing with permission denied — `EXPLAIN` became its
   own action and `Action::ALL` stopped including it, exactly as intended, but
   this crate grants `ALL`. Now `EVERYTHING`, plus a `reader` role holding
   `ALL` and nothing else.

Then two of my own: the committed protobuf stubs were stale (the repo's own
`test_the_committed_stubs_match_the_proto` caught it, which is the drift test
earning its keep), and `pb.AggregateQuery` had no `sort` field until they were
regenerated.

## Alternatives rejected

**One `AggregateQuery` holding both a table and a join.** Fewer types, and it
would make `.where()`, `.compute()` and `.using_index()` silently do nothing on
the join-shaped one. The module's own header says its builders exist so that a
refusal is "unreachable through this API rather than a runtime surprise";
`_Grouping` as a shared base keeps that true.

**Counting the join's inputs in the client.** `GroupedJoinQuery` takes whatever
join it is given and lets the server refuse a third input by name. A count here
would be a second statement of the kernel's limit, free to drift from it — and
the refusal is now something a Python caller can reach and test, which it is.

**Moving `testserver` into the root workspace.** The honest fix for the rot, and
not taken here: it pulls a client's test fixture into every root `cargo build`
and this commit is already three repairs deep. Recorded as open below rather
than done quietly.

## Evidence

138 Python tests pass (was 117 passing / 14 failing / unbuildable before), ruff
and mypy clean. Three mutations on the new code, all killed: dropping the group
sort, dropping the offset, and replacing the join with its first input each fail
named tests.

The right-side group-key test is the one that matters most — it fails if the
joined schema does not span every input, which a key on input 0 never catches.

## What this does not do

`testserver` stays outside the root workspace, so nothing stops it rotting
again. The three failures above were each silent for as long as it took someone
to run this suite. A root-workspace membership, or a CI job that builds it, is
the actual fix and is not in this commit.

Grouping a chain is still unbuilt in the kernel; the client can now *reach* that
refusal, which is not the same as the feature existing.
