# The deployed example now runs all three SDKs, reads through replicas, and survives a `kill -9` — and one durability claim is withdrawn

- **Date:** 2026-09-15
- **Author:** Claude, working from "I don't want any gaps. Please address those"
- **Touches:** `examples/deployed/` (`head.toml`, `run.sh`, `check.py`, new `probe.py`, new `go/`, new `node/`, `README.md`), `.github/workflows/ci.yml`, `.gitignore`, `examples/explorer/web/src/api.ts`
- **Kind:** feature

## What changed

Three things the example's own README listed as gaps, and a fourth that was
found while closing them.

**All three SDKs, over the same socket.** `check.py --expect` writes the
answers its Python fold computed; `go/check.go` and `node/src/check.ts` ask the
same six questions through their own client and assert the same answers. The
fold stays in one language deliberately — three decoders of one packed file
would be three places to be wrong, and a common-mode error in them would agree
with itself.

**Two read replicas.** `head.toml` declares `reader-a` and `reader-b` with a
`[routing] catch_up` of two seconds. The check does eight reads, requires every
one to name the view that served it, and requires at least one of those names
to be a `reader-`. A read asking for `Freshness.latest()` is checked
separately, because that one has nowhere to go but the writer.

**A restart.** The head node is `kill -9`'d and a replacement starts on the
same bucket. Every check runs again — not a subset, because the recovered LSM
has to answer the whole query surface and not just a count.

**A durability probe, and a withdrawn claim.** `probe.py --write` inserts one
row and returns on the acknowledgement; `run.sh` kills the process on the next
line; `probe.py --verify` asks the replacement for it. What that proves is that
an acknowledged write was in the bucket rather than in the dead process's
memory. What it does *not* prove is written up below.

## Why

The README's "What it does not cover" listed exactly these: Python only,
no replicas, no restart. Each was a real hole rather than a cosmetic one.

The three-SDK hole mattered most. The Python client declares a `Table` and
carries a schema fingerprint; the Go and TypeScript clients name a table and an
ordinal. `join.computed(0)` against `slate.JoinComputed(0)` against
`joinComputed(0)`. `BigInt` against `int`. Every one of those is a place a
client can send something subtly different, and `examples/explorer` compares
them only over an **in-memory** head node — so nothing compared them over
SlateDB, over a bucket, over a socket that had already carried 100,000 rows.

## Alternatives rejected

**Decode the packed trip file in Go and TypeScript too**, so each check has its
own oracle. It sounds stronger and is weaker: the *value* of the Python fold is
that it shares no code with the kernel, and a Go decoder of the same format
would share its author's misunderstanding of the format. Three folds that
agree with each other and are all wrong is the failure mode the explorer's
conformance runner already exists to avoid.

**Have the Go and TypeScript checks re-ask everything Python asks.** They ask
six of the nine. The three left out — the wall-clock shape, the day-of-week
against `datetime`, the five-hour offset — are assertions about the *data* and
the kernel rather than about the client, and a client cannot get them wrong in
a way the six do not already catch.

**Start the replacement node immediately after the kill.** That is what the
first draft did, and it came up read-only: nothing renews a lease for a process
that is gone, so the dead node's writer lease outlives it and a replacement
cannot open the database until the term expires. The read-only node answered
every read correctly and made the restart prove nothing about the writer. Fixed
two ways: `run.sh` waits the term out, and `start_head` now *refuses to
continue* unless the node logged `leader` — a much better failure than a
restart that quietly proves nothing.

**Leave `[lease] term` at its default fifteen seconds.** The example would take
a quarter-minute longer for no gain. Three seconds is written into `head.toml`
with the trade spelled out: a short term replaces a dead leader faster and
fences a live-but-paused one sooner, which is not free.

**Assert only that a read names *some* view.** That was the first version of
the replica check and it passes with the writer serving everything — `writer`
is one of the names. Eight reads and "at least one `reader-`" is what separates
two replicas configured from two declared and never used.

## Evidence

**A full run, `./run.sh --trips 20000`:** nine Python checks plus four
replica/freshness ones, seven Go checks, seven TypeScript checks, then the
probe, then all of it again after the kill. Every one passes.

**Mutation testing, three mutations:**

| mutation | result |
| --- | --- |
| remove both `[[replicas]]` from `head.toml` | **caught**: `FAIL  and a read replica served at least one of them   ['writer']` |
| remove the lease wait before the restart | **caught**: the run stops with `slate-serverd came up without the writer`, and prints the log line `follower (read-only: no writer store, writes are redirected)` |
| the Go adapter's... (see the explorer entry) | — |
| `durability = "visible"`, three separate runs | **survived all three** |

**The withdrawn claim.** `durability = "visible"` documents itself as losable
"if the writer fails before its next flush", so a write acknowledged
microseconds before a `kill -9` ought to be losable. The first draft of
`run.sh` said so in a comment, as the reason the probe exists. It was then
tried: `visible`, kill immediately after the acknowledgement, three runs, and
the probe found the row every time. Whether SlateDB flushes before returning
from this path or whether three attempts simply never landed inside the window
is not something a shell script and a stopwatch can tell — so the claim is
gone from `run.sh`, gone from `README.md`, and replaced in `probe.py` by a
paragraph saying what was tried and what came back. The probe is kept, because
what it *does* prove — an acknowledged write is in the bucket and a
replacement finds it — is worth proving and nothing else here proved it.

**A guard caught a stale copy of the schema.** CI's `demo frontend units` job
failed on "the UI's column names match head.toml, in order" after the
explorer's `books` grew two columns in the previous change. That is the guard
doing its job; the frontend's `TABLES` now lists `released` and `embedding`.
Worth recording because it was found by CI and not locally: the explorer's
*unit* tests are a separate command from its conformance runner and its e2e,
and running the latter two is not running the former.

## What this does not do

**Failover under load.** The restart is a kill and a cold start: nothing is
reading while the lease changes hands and no second node is campaigning.
Overlapping writers across a lease change are `slate-server`'s own suite.

**A replica actually lagging.** On a local filesystem bucket the replicas poll
faster than anything here can observe, so `Freshness.at_least` is exercised in
the sense that it is served and not in the sense that it had to wait. Making a
replica lag on purpose needs a slow object store, which is
`slate-slatedb`'s fault-injection suite.

**Tenant affinity.** `[routing] tenant_affinity` is not set, so the pool
round-robins. The affinity path has kernel tests and no deployed one.

**Anything about which replica *should* serve a read.** The check requires that
some replica did; it does not check the distribution, because eight reads over
two replicas says nothing statistically and a check that flakes on a hash is
worse than none.

**A restart of a replica.** Only the writer is killed. The replicas are
`DbReader`s in the same process, so killing the process kills them too — a
separate reader process is a topology this example does not build.

**Byte counts in the committed bucket listing.** `site/data/bucket.json` is
verified by `crates/slate-wasm/tests/bucket_provenance.rs` against what the
workbench reports; nothing compares it to the objects *this* run leaves in its
bucket. The two are different datasets at different compaction states, so the
comparison would need a tolerance nobody could justify.
