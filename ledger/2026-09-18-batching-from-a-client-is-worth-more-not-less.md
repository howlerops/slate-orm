# Batching from a client is worth more, not less

- **Date:** 2026-09-18
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `examples/batchbench/` (new), `.github/workflows/ci.yml`, `docs/performance.md`, `docs/orm-comparison.md`
- **Kind:** measurement

## What changed

`examples/batchbench` runs N single inserts against one batch of N, once per
client, against one head node. A CI job runs it at a small N so it cannot rot.
`docs/performance.md` gains §3b.

At 100 rows over five runs:

| client | one row at a time | in one batch | ratio |
|---|---:|---:|---:|
| Python | 969.6 µs [925.9 – 1038.4] | 30.7 µs [29.7 – 31.8] | **31.6×** |
| Go | 763.4 µs [733.9 – 772.3] | 35.7 µs [33.8 – 36.1] | **21.4×** |
| TypeScript | 1155.8 µs [1114.7 – 1768.2] | 34.0 µs [33.5 – 64.6] | **34.0×** |

## Why

`docs/performance.md` §3 reports 15× for a hundred rows batched against a
hundred written one at a time. That is a Rust wire test: it goes through
`tonic` and nothing else. Every README sentence saying "a batch is a round
trip" was a claim about the three SDKs resting on a measurement that had never
gone through one.

## The finding, which contradicts the item that asked for it

`docs/orm-comparison.md` N6 predicted a **smaller** multiplier from a client:

> The clients add per-request work batching does not save — schema claims,
> value encoding, per-operation table resolution — so their multiplier is
> smaller by an unknown amount.

Measured, it is **larger**: 21–34× against the wire's 15×. **The prediction is
withdrawn.**

The arithmetic was backwards. That per-request work is paid *per request*, so
it multiplies the one-at-a-time arm by a hundred and the batched arm by one —
it widens the gap rather than narrowing it. A client pays more per round trip
than `tonic` does, which is exactly the reason saving round trips is worth
*more* to a client, not less. The prediction treated a per-request cost as
though it were per-row.

## The cap question, decided from the number

The item also asked whether a client should check `max_batch_operations` before
sending, and said explicitly not to guess which way it went.

The default cap is 1,000 operations. A batch that size is about 30 ms of the
measured work. The refused round trip a client-side check would save is one
RPC — the single-insert column, about 1 ms. So the refusal costs **about 3% of
what the batch would have cost**, once, at the moment a caller discovers their
batch is too big.

Three percent does not buy a client-side copy of a server-configurable limit.
Two numbers that can disagree is a client refusing a batch that a differently
configured server would have accepted, which is a worse failure than a wasted
millisecond. **Left server-side**, which is the outcome the item allowed for.

## Alternatives rejected

**Assert a threshold.** The obvious way to make this a test, and wrong for the
reason the Rust benchmark already gives: a wall-clock assertion on a shared
runner is a flake waiting for a slow morning. CI runs it at N=20 to prove it
still *runs* — a benchmark nobody runs stops building, and then its numbers are
quietly from whatever the code looked like last time somebody tried.

**Reuse the demo's head node.** `examples/explorer/head.toml` has foreign keys
and two RLS policies, and a write benchmark over those would be measuring them
rather than the round trip it is about. One table, no constraints, its own
config.

**Reuse the demo's `run.sh` with a `--batchbench` mode.** Tempting, because the
port allocation and process-group teardown there are hard-won and commented at
length. Rejected because that script also starts three HTTP adapters and a
frontend, and a benchmark that runs with four unrelated services on the same
machine is measuring them too. The port and teardown logic is reproduced, with
a pointer to the original rather than a copy of its reasoning.

**One process per client, sharing a key range.** Rejected: an insert refuses a
taken key, so a second arm over the same keys would time the refusal rather
than the write. Each run and each arm gets a disjoint range, which is why the
bases are three separate millions.

**Measure reads too.** Out of scope for the claim being checked, which is about
batching *writes*. Named below rather than done quietly.

## Evidence

The table above, five runs each, reported as median with the observed range.
The CI-shaped invocation (`--rows 20 --runs 2`) was run against this tree and
gives 10.8–15.2×, which is the same shape at a fifth of the size: the ratio
falls with N because the fixed per-RPC cost is amortised over fewer rows, and
that is the arithmetic rather than a finding.

The spread is worth reading. TypeScript's one-at-a-time maximum is 1768 µs
against a 1115 µs minimum — a 59% spread, where Go's is 5%. A single number
there would have been dishonest.

## What this does not do

**Writes only.** A read batch is not a thing this protocol has, so there is
nothing to compare; but nothing here says what batching does for reads, and the
README does not claim it.

**Loopback only.** Every number is smaller than it would be over a network,
because the round trip a batch saves is cheapest on loopback. That is the
conservative direction — a batch helps at least this much — and it is stated
rather than left for a reader to work out.

**No server-side attribution.** The numbers say a batch is faster from a client
and by how much. They do not say how much of the one-at-a-time cost is the
client's own per-request work against the server's, which is the measurement
that would turn "the prediction was backwards" into "and here is exactly how
much of it is the client". A per-arm breakdown needs client-side
instrumentation this does not have.
