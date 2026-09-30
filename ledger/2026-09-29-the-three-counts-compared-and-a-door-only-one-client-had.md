# Three clients counting their own round trips is not three clients agreeing

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `examples/explorer/backends/{go,node,python}`,
  `examples/explorer/conformance/conformance.py`,
  `examples/explorer/CONTRACT.md`, `clients/typescript/src/index.ts`,
  `scripts/check_demo_surface.py`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`POST /api/round-trips` in all three demo adapters. It runs one of four fixed
workloads on a counting client and answers with how many gRPC calls that cost,
per RPC. Four conformance cases compare the three answers, and three
`MUST_DIFFER` pairs stop an adapter that ignored `workload` from agreeing with
two others doing the same. 133 cases to 137.

Measured, identical in all three SDKs: four singles cost **4 `Insert`**, the
same four rows in a batch cost **1 `Batch`**, three keyset pages cost **3
`Query`**, and one relationship loaded for three parents costs **1 `Related`**.

The Node adapter needed something first, and it is the same shape as the thing
this closes. `Client.connect`'s third and fourth arguments are
`grpc.ChannelCredentials` and `grpc.ChannelOptions`, so a caller wanting an
interceptor needs grpc-js — and needs *this* copy of it, because npm would
install the adapter a second one and two instances of a library whose objects
cross the boundary is a class of bug nobody wants to debug from a demo. The
client re-exports it now (`export * as grpc`). Python's `Client` takes
`channel=` and Go's `Dial` takes `...grpc.DialOption`, and neither language can
have this problem — one interpreter, one module graph — so only the TypeScript
client needed the door.

## Why

Two entries on 2026-09-28 asked for exactly this, in two wordings:

> **It does not check that the three agree.** Each client is measured against
> its own expectation. A conformance case comparing round-trip counts across
> the three would catch a client that quietly looped where the others batched.
> — `ledger/2026-09-28-the-go-client-was-already-counting.md`

> **No conformance case compares the three counts against each other.** Three
> clients agreeing that a batch is one call is a stronger statement than three
> independent assertions of it.
> — `ledger/2026-09-28-the-third-client-counts-and-the-go-instrument-was-half-blind.md`

The gap is real and is the one this runner exists for. Four singles and a batch
of four write the same four rows. A relation loaded for three parents returns
the same three groups whether the client sent one request or three. A client
that loops is right about every row and costs N times as much, so all 133
existing cases pass it — and each client asserting its own count against its
own expectation cannot catch two clients wrong the same way, which is the
failure mode this file was written for in the first place.

That is not hypothetical here. The Go instrument *was* half blind for a day:
`dialRecording` held a unary interceptor and no stream one, so it counted
writes and reported zero for every `Query`. It was found by needing a paging
count, in one client, while the same hole sat open in another.

## Alternatives rejected

**Compare the server's `/metrics` counters instead.** `slate-serverd` exports
`slate_requests_total{method=…}` and the daemon's suite already scrapes it, so
the instrument exists and is cheaper. Rejected for the reason the Python
round-trip file gives about its own design: a server-side count answers "how
many requests arrived", which is the same number only if the client sent what
it thinks it sent. The claim is about the client. It is also one counter for a
process serving three adapters, so attributing a request to an SDK would mean
a label the server does not carry.

**Assert the expected counts in the runner rather than only comparing them.**
Tempting, and it would catch all three clients being wrong the same way — which
is the one thing agreement cannot see. Rejected because the `MUST_DIFFER` pairs
already cover the realistic version of it: three clients that all sent four
`Insert` for the `batch` workload would make `singles` and `batch` identical
and fail the pair, and three that all looped the relation would make it
identical to `paging`. An expected-count table would also be a second place to
edit when a workload's size changes, and this runner's whole design is that it
holds no expected answers at all.

**Count on the adapters' existing `app` client.** One fewer connection. It
would fold a browser polling the demo into a measurement, and the failure would
be an intermittent disagreement between three adapters that are all correct.
A fourth client costs one socket per adapter and cannot be polluted.

**Time the workloads instead of counting them.** The inherited "batching is
9.4x" was a ratio of durations, and a timing claim from this repository already
reversed between two machines. A count has no spread: four writes are four
requests or they are one, on any machine and under any runner load. This is the
third file to make that argument and the first to make it across three clients
at once.

**Twenty rows, as the per-client suites use.** Twenty is right there — `20 != 1`
is a failure message worth reading. Four here, because this runs against the
shared demo database on every conformance run and every CI job, and what is
being compared is the *shape* of the count, which four shows as well as twenty.

**Add `@grpc/grpc-js` to the Node adapter's own dependencies.** The obvious fix
and the wrong one: npm installs it into the adapter's `node_modules` while the
client keeps its own, and the interceptor the adapter builds is then handed to
a different grpc-js instance. Re-exporting from the client guarantees one.

## Evidence

- `examples/explorer/run.sh --conformance`: **137 cases, the three SDKs agree
  on all of them. 137 passed, 0 failed.** Up from 133.
- The counts, read off a running stack rather than asserted — twice, ten
  minutes apart, identical both times and with the 9400 range empty afterwards:

  | workload | go | node | python |
  | --- | --- | --- | --- |
  | `singles` | `Insert` 4 | `Insert` 4 | `Insert` 4 |
  | `batch` | `Batch` 1 | `Batch` 1 | `Batch` 1 |
  | `paging` | `Query` 3 | `Query` 3 | `Query` 3 |
  | `related` | `Related` 1 | `Related` 1 | `Related` 1 |

  No RPC appears that is not listed: the counted clients make no schema fetch,
  no leadership probe and no retry, so the numbers are the workload and nothing
  else.
- **Six mutations, all caught**, recorded under `ledger/mutations/`:
  - `ledger/mutations/20260929T015742-examples-explorer-backends-go-roundtrips-go.json` — the Go adapter loops where the
    other two batch. Caught: `four rows in a batch cost one request … the
    adapters disagree`, and the `singles`/`batch` pair became incomparable.
  - `ledger/mutations/20260929T015818-examples-explorer-backends-go-main-go.json` — the Go counter loses its stream
    interceptor, which is the bug it shipped with for a day. Caught on
    `three keyset pages cost three requests`.
  - `ledger/mutations/20260929T015848-examples-explorer-conformance-conformance-py.json` — the `batch` case asks for
    `singles`. Caught by the pair, which is the only thing that could: all
    three still agree, and agree with the other case.
  - `ledger/mutations/20260929T015917-examples-explorer-backends-node-src-main-ts.json` — the Node adapter loops.
  - `ledger/mutations/20260929T015946-examples-explorer-backends-python-adapter-main-py.json` — two: the Python adapter
    walks one page fewer, and the counter is cleared *after* the workload
    rather than before, which lets the scrub land in the count. The second
    fails all four cases, which is what an instrument reading its own
    housekeeping looks like.
  - The seventh record, `ledger/mutations/20260929T015734-examples-explorer-backends-go-roundtrips-go.json`, is `baseline-reported-nothing`: the
    first attempt ran `run.sh` under `sh`, and `set -o pipefail` is not POSIX.
    Kept, because a run that could not score is exactly what that file is for.
- `python3 scripts/check_demo_surface.py`: `15 of 25 adapter endpoints in the
  UI, 10 left out on purpose`.
- `python3 scripts/check_closed_caveats.py`: 400 closed, 365 witnessed, 35
  exempt.
- `python3 scripts/caveats.py`: 1620 caveats, 110 open, 72 narrowed, 400
  closed, 905 deliberate, 0 untriaged.
- `sh scripts/check.sh`: 72 passed, all of them.

## What this does not do

**It still counts calls, not work.** Four rows inside one batch are still four
schema claims and four value encodings in the client, and no count can see
that. The surviving half of the original caveats is the per-request client
work, and it is untouched here — this makes the *request* half a three-way
comparison and leaves the other half exactly where it was.

**It says nothing about latency or bytes.** A count on a loopback socket
against an in-memory store is not a network round trip. That remains
`examples/deployed`'s to answer, and it needs MinIO, which this container does
not have.

**Four rows and three pages are not a scale.** The workloads are sized so a
disagreement is legible, not so the numbers mean anything about throughput. A
client that batched correctly up to four rows and looped above it would pass.

**Nothing still checks that a fourth client arrives with a transport door.**
The `grpc` re-export is one more way to reach a channel and not a guard: the
three clients now expose it three ways and no check fails if a fourth arrives
exposing none. That caveat is untouched and stays open, one client wider than
it was.

**The `MUST_DIFFER` pairs cannot see three clients wrong the same way about a
workload that has no pair.** `related` is paired against `paging` and `batch`
against `singles`, so a looping relation and a looping batch are both caught —
but the pairing is by hand, and a fifth workload added without one would be
compared only for agreement.

**The lock is real and untested.** Each adapter serialises measurements — a
`threading.Lock`, a `sync.Mutex`, a promise chain — because the counter is
process-wide state and two overlapping requests would each read the other's
calls. The conformance runner is sequential, so nothing exercises the
contention path; it is there because a count that is silently wrong under
concurrency is worse than one that waits.

**The endpoint ignores `X-Demo-Identity`.** Deliberate, and written into
`CONTRACT.md`: row-level security changes which rows a read returns and not how
many requests fetching them takes. A reader persona would therefore return the
same four numbers, which is a knob that cannot move the answer — so it is not
offered rather than offered and meaningless.
