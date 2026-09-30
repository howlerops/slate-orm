# A mutation survived the conformance runner because the runner was pointed at a binary built before it

- **Date:** 2026-09-30
- **Author:** Claude Opus 5
- **Touches:** `scripts/prebuilt.py`, its test, three `run.sh`, `clients/python/tests/conftest.py`, the three adapters, the conformance runner
- **Kind:** process

## What changed

Three things, and the first is why the other two exist.

**`scripts/prebuilt.py`**, the one refusal of a prebuilt binary older than the
source it was built from, and `scripts/test_prebuilt.py`, a roster holding
every harness that takes `SLATE_SERVERD` or `SLATE_TESTSERVER` to using it.
Three of six did; the three `examples/*/run.sh` did not, and one of those
starts the three-SDK conformance runner.

**`error.details`** in the demo's HTTP contract, so the three adapters publish
the refusal metadata this morning's work put on the wire, and
`MUST_CARRY_DETAILS` in the conformance runner, so three adapters agreeing on
an empty map is a failure rather than a pass.

**Two stale claims corrected**, both found by re-reading exemptions I had
written hours earlier without reading what they exempted.

## Why

The mutation. `crates/slate-server/src/status.rs` was mutated to send an
`ErrorInfo` with **no metadata at all** — the whole map, gone — and the
three-SDK conformance runner reported *140 cases: the three SDKs agree on all
of them*. `scripts/mutate.py` scored it **SURVIVED**, which in this repository
means "write a test".

It is not a missing test. `examples/explorer/run.sh` was pointed at
`SLATE_SERVERD`, a binary built before the mutation, so the mutated code never
ran. That is `mutate.py`'s own first documented lie — *"the suite runs against
unmutated code and passes"* — reached through a door `mutate.py` cannot see,
because the staleness is in a binary rather than in a patch that failed to
apply. And it arrives wearing the most expensive possible costume: a survivor
reads as a gap in the tests, so the honest response to it is to go and write a
test that was never missing.

`clients/python/tests/conftest.py` had refused exactly this since the day a
full run reported 153 passing tests against a stale `slate-testserver`. The
Go and TypeScript harnesses had copied it. The three `run.sh` had not, and
nothing said so.

## Alternatives rejected

**Leaving the check in three places and adding it to three more.** Six copies
of a modification-time comparison, and the roster would still be the thing
catching the seventh. One implementation plus a roster is the same amount of
enforcement and one place to fix.

**A content hash rather than a modification time.** Exact, and it needs the
source hash recorded at build time — a second thing to keep in sync, whose own
staleness nothing would catch. What mtime misses beyond this is "somebody
touched a file without changing it", which costs a rebuild rather than a wrong
answer.

**Importing `prebuilt` in `conftest.py` the ordinary way.** It does not
typecheck: `ty` runs over `clients/python` alone, because the published
package must stand up without the rest of the repository. Loaded by path, with
that reason written where the load is.

**A dedicated conformance case for a bound refusal** — a handler in three
languages that asks for a path past `max_relation_depth`. Not built. Adding
`details` to the envelope means the existing check-violation case already
compares a ten-key map across the three, which is the property that was
untested; a second case would add a second *shape* of map, which is worth
less than it costs in three more handlers. Recorded as a caveat rather than
done.

## Evidence

`python3 scripts/test_prebuilt.py`: 19 passed, 0 failed. `prebuilt.py` over
written trees in both directions, a missing path refused, and the roster in
both directions — a listed file that stops reading the variable is reported
as loudly as an unlisted one that starts.

The refusal, against the binary that caused this:

```
$ SLATE_SERVERD=target/debug/slate-serverd python3 scripts/prebuilt.py
SLATE_SERVERD=…/slate-serverd was built before crates/slate-server/src/status.rs
was last changed, so this run would test a server this tree did not produce.
```

`./run.sh --conformance`: 140 cases, the three SDKs agree on all of them, now
including `error.details`. The mutation that started this — the Python adapter
dropping its `details` map — is caught
(`ledger/mutations/20260930T034543-examples-explorer-backends-python-adapter-main-py.json`),
and the server-side one is recorded as the invalid run it was
(`ledger/mutations/20260930T034740-crates-slate-server-src-status-rs.json`).

Two more bounded refusals converted to `status::refused`, with their numbers
in the metadata and a test reading them as values:

| refusal | reason | metadata | test |
| --- | --- | --- | --- |
| a batch over the cap | `BATCH_TOO_LARGE` | `limit`, `asked` | `a_batch_over_the_cap_is_refused` |
| a node at its transaction limit | `TRANSACTION_LIMIT` | `limit`, `open` | `a_node_will_not_open_more_transactions_than_its_limit` |

**Two stale claims, both in exemptions I wrote this morning.**
`ledger/2026-09-30-a-premise-nobody-here-can-falsify.md` exempted two caveats
from the outside-the-tree guard as "about this tree after all" — correctly —
and recorded, as a caveat of its own, that neither had been *re-read*. Reading
them took ten minutes:

- `2026-09-14-the-testserver-joins-the-workspace.md`: the demo's three
  backends are "a Go module, an npm package and a Python package that **no
  root-level command builds**". False. `scripts/check.sh` runs `gofmt` and
  `go vet` on the Go module, `tsc --noEmit` on the npm package and `pytest`
  over the Python one. Narrowed: outside a workspace, yes; unreached, no.
- `2026-09-14-frontend-tests-and-configurable-ports.md`: playwright as a
  heavyweight devDependency, justified by the browser already being present.
  Holds, and now carries a `recheck`.

That is a **fourth** stale claim today, and the first one found by a caveat
warning about its own author.

## What this does not do

**`prebuilt.py` compares one binary against the newest source in two trees.**
A harness that starts two binaries — the deployed example runs a head node and
a replica — gets one verdict for both, which is right today because both come
from the same build and would be wrong the moment they did not.

**The roster cannot tell whether a file's refusal works**, only that it
mentions one. A harness that imports `prebuilt` and never calls it passes.
That is the same cost `check_retired_claims.py` states about its registry, and
the same answer: what it buys is the next harness, not this one.

**Nothing re-runs the mutation that started this.** It was invalid, not
survived, and re-running it correctly means `run.sh` building from source —
about four minutes per case on this container against ten seconds with a
prebuilt binary. The refusal now makes the invalid version impossible, which
is the fix; the finding it would have produced is still unmeasured.

**`MUST_CARRY_DETAILS` has one row.** It names the check-violation case,
because that is the richest map the server sends. A bound refusal's flat
two-key map is not compared across the three SDKs by anything live, only by
the three decoder tests over one fixture.
