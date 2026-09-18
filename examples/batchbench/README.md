# Does batching help a *client*?

`docs/performance.md` reports a 15× difference between a hundred rows written
one RPC at a time and the same hundred written in one call. That number is a
Rust wire test: it goes through `tonic` and nothing else.

A client does work a batch does not save — a schema claim, per-operation table
resolution, value encoding in a language that is not Rust — so its multiplier
is smaller by an amount nobody here had measured. Until this directory existed,
every README sentence saying "a batch is a round trip" was a claim about the
three SDKs resting on a measurement that never went through one.

This measures it, once per client, in the shape the Rust benchmark uses: **N
single writes against one batch of N**, five runs, reported as a median with
its spread.

```sh
./run.sh              # all three, against one head node
./run.sh --rows 200   # a different N
```

## What it found

At 100 rows, five runs, against a head node on memory storage:

| client | one row at a time | in one batch | ratio |
|---|---:|---:|---:|
| Python | 969.6 µs [925.9 – 1038.4] | 30.7 µs [29.7 – 31.8] | **31.6×** |
| Go | 763.4 µs [733.9 – 772.3] | 35.7 µs [33.8 – 36.1] | **21.4×** |
| TypeScript | 1155.8 µs [1114.7 – 1768.2] | 34.0 µs [33.5 – 64.6] | **34.0×** |

**Bigger than the wire test, not smaller.** The plan predicted the opposite —
that a client's per-request work would eat into the saving. It does the
reverse: that work is paid *per request*, so it multiplies the one-at-a-time
arm by a hundred and the batched arm by one. `docs/performance.md` §3b has the
withdrawal.

## What it does not do

**It does not assert.** A wall-clock threshold on a shared runner is a flake
waiting for a slow morning, which is why the Rust benchmark reports rather than
asserts and why this does too. CI runs it to prove it still *runs*; the numbers
are read by a person.

**It is one process against a local head node.** Every number here is smaller
than it would be over a network, because the round trip a batch saves is the
thing a loopback makes cheapest. That is the conservative direction: a batch
helps at least this much.
