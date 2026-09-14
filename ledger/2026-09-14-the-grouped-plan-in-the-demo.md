# The grouped plan reaches the demo, and the three clients learn to spell

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `examples/explorer/{CONTRACT.md,run.sh,conformance/conformance.py}`,
  the three adapters, `examples/explorer/web/src/{api.ts,panels.tsx}`,
  `examples/explorer/web/e2e/explorer.mjs`,
  `clients/{python/src/slate/client.py,typescript/src/client.ts}`,
  `clients/{python/tests/test_explain.py,typescript/test/join.test.ts}`
- **Kind:** feature

## What changed

`POST /api/explain-aggregate` — the plan of the *grouped* read — in the
contract, in all three adapters, in three conformance cases, and in the
explorer's grouped-join panel, which now shows each input's access path and
what it decodes.

And the three clients now spell a join algorithm the same way.

## Why

The previous commit added `ExplainAggregate` to the kernel, the wire and the
three clients, and its ledger entry recorded the demo as the one place a reader
would meet the feature and not find it. This closes that.

The value of putting it in the demo is not the panel. It is that the
conformance runner then compares three independent client renderings of a
grouped plan against each other, which is coverage no client's own suite can
produce.

## What it found, on the first run

**The three clients disagreed about the name of a join algorithm.** Go returned
`"hash"`; TypeScript returned `"hashBuild"`, which is whichever key
`proto-loader` gave the `oneof`; Python handed back the raw protobuf message.

Three answers to one question, and every client's suite was green, because each
compares itself to the *server* and the server never sends a name — it sends a
`oneof`, and each client invented its own rendering. This is precisely the
failure mode `conformance/` was built for, written up when it was built, and
here it is again in a place nobody had looked.

All three now return `"hash"` or `"nested loop"`, with `null`/`""`/`None` on the
first input. TypeScript's test asserted only that two forced algorithms
*differed*, which any two distinct spellings satisfy; it names them now. Python
had no test at all and has one.

**The node adapter was running yesterday's client.** It resolves
`@slate-orm/client` through a symlink into `clients/typescript` and imports the
built `dist/`, which nothing rebuilds. So the alignment above reached Go and
Python and not node, and the conformance runner reported it as the SDKs
disagreeing — which, at that moment, they did, for a reason that was not in any
of their source. `run.sh` builds the client before starting the adapter now.

## Alternatives rejected

**Leaving `algorithm` out of the contract, like `estimatedCost`.** The tempting
fix: it is a value the clients render differently, which is exactly the stated
reason `estimatedCost` is excluded. Rejected because the two are not alike.
`estimatedCost` is a float and the difference is *formatting*; the algorithm is
a name and the difference was three clients inventing three vocabularies for
one wire value. Excluding it would have hidden a divergence rather than
recorded one, and left the next caller to hit it.

**Normalising the name in each adapter.** Would make the demo agree while the
clients still disagreed — the adapter papering over its SDK is the one thing
this demo must never do, since a reader is meant to read the adapter as
evidence about the client.

**A separate `/api/explain-grouped-join` and `/api/explain-grouped-table`.** The
demo only groups a join. One endpoint mirroring `/api/aggregate`'s body exactly
means the plan explained is the read the chart draws, which is the same
property the RPC underneath has.

**Duplicating the join-and-grouping construction in each adapter's new
handler.** Each adapter now builds it once (`buildAggregate`, `#buildAggregate`,
`_build_aggregate`) and both handlers call it — the same argument as the
kernel's shared narrowing, one level up: an explanation of a *different*
request is worse than none.

## Evidence

`./run.sh --conformance`: **34 cases, the three SDKs agree on all of them** — up
from 31, and the three new ones are what caught the algorithm divergence. One of
them is a refusal (`a reader may not explain a grouping`), so the privilege is
checked through all three clients too.

`./run.sh --e2e`: 15 passed, up from 14. Mutated by making the Go adapter report
an empty `decodes` — `the grouped panel shows the plan of the grouped read`
fails, which is the whole chain from kernel to browser.

`clients/typescript`: 51 pass, with the forced-algorithm test now naming both
results rather than asserting they differ. `clients/python`: 12 in
`test_explain.py`, including the new naming test.

## What this does not do

The panel shows the two inputs' access paths and `decodes`; it does not show the
estimated rows or cost, which the contract excludes as client-formatted floats.
An operator wanting those reads the `display` block underneath, where they come
from the server as one string.

The demo groups a two-table join and nothing else, so `/api/explain-aggregate`
never exercises the chain or single-table shapes the RPC also serves. Those are
covered in `crates/slate-server/tests/multi.rs` and in the kernel, not here.

Nothing checks that the *adapters* agree with the clients about the algorithm
name — the conformance runner checks the three adapters against each other, and
all three could be wrong together if the shared vocabulary were wrong. The
vocabulary is asserted against literals in the TypeScript and Python client
suites, which is where that belongs.
