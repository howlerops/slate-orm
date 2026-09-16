# Relations in the Go client, and a doc comment that spun a test at 100% CPU for eight minutes

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `clients/go/slate/related.go`, `clients/go/slate/related_test.go`, `clients/go/slate/client.go`
- **Kind:** feature

## What changed

`Session.Related` and `Transaction.Related` in the Go client, over the `Related`
RPC added with the Python half. A `Relation{On, Through, Way}` names the
relationship by the foreign key that already declares it — no second
declaration on the client, which could disagree with the catalog's. `Way` is
`Children` or `Parents` rather than `Direction`, which this package already
spends on a sort order. Eleven tests, including an oracle against a filtered
read per parent, and a gRPC unary interceptor that reads the outgoing request.

The interceptor also caught a false doc comment on `RowStream.Next`, which is
corrected here: `Next` does not advance, `Row` does.

## Why

Relations were on the wire and in one of three clients. The conformance runner
compares the three SDKs against each other, so a feature in one of them is a
feature nothing cross-checks — the assertion the plan in `docs/orm-comparison.md`
called the important one does not exist until all three have it.

`Transaction.Related` is there because Go's `Transaction` already carries
`Query`, `Join` and `Get`, and a relationship load that could not see the
transaction's own uncommitted writes would be the one read that behaves
differently inside one.

The doc comment said "Next advances to the next row". It does not: it reports
whether a row is available and refills the batch, and `Row()` moves the cursor —
which is what lets `Computed()` be read for the same row first. A counting loop
written from that comment, `for s.Next() { n++ }`, never terminates. It span at
100% CPU for eight and a half minutes before a `top` showed it, and it was
written from the comment, not in spite of it.

## Alternatives rejected

**Make `Next()` advance and `Row()` return the current row.** That is the
API most Go iterators have and would make the misuse impossible. It inverts
the cursor for every existing caller and breaks `Computed()`'s contract, which
is documented as "read it before `Row`, which advances the cursor". A
correctness-preserving rename across the three clients is a bigger change than
the bug justifies, and the three would then disagree with each other until all
three were done.

**Have `Next()` detect a second call with no intervening `Row()`** and advance
itself. Cheaper, but it makes the cursor's position depend on the call history,
which is worse to reason about than either honest rule.

**A `Related` on `Session` only, and let a transactional caller use `Query`.**
Half the point of the call is that it is one round trip; telling a transaction
to loop over `Query` per parent gives back exactly what was bought.

**Describe the relationship client-side** — "relate ordinal 1 to ordinal 0" —
rather than naming a foreign key. It would work without the server resolving
anything, and it is how a client could relate two tables with no constraint
between them. Rejected because two clients could then describe the same
relationship differently and both be right, which is precisely the divergence
the conformance runner is there to catch. The catalog already encodes both
directions of every foreign key; a second description can only disagree with it.

**Test the freshness floor by standing up a replica.** It is the only way to
observe a stale read for real. The Go harness runs one node and a replica would
be minutes of setup per test; the interceptor asserts the floor is on the
request, which is what the client is responsible for. See what this does not do.

## Evidence

Eleven tests pass against a real node (`go test ./slate -run TestRelated`,
0.35s). `go vet ./...` and `gofmt -l` clean; the whole Go suite passes in 5.0s.

Mutation run over `related.go`, six mutations:

| mutation | first run | after |
| --- | --- | --- |
| direction ignored, always children | killed | killed |
| transaction dropped | killed | killed |
| freshness floor dropped | **survived** | killed |
| schema claim dropped | **survived** | killed |
| composite key not refused | **survived** | killed |
| groups in the server's order, not the caller's | killed | killed |

Three survivors, three missing tests, all three written:
`TestRelatedCarriesTheFreshnessFloor`, `TestRelatedCarriesTheSchemaClaim`,
`TestRelatedRefusesACompositeKey`. The first two are the same shape and the
reason the interceptor exists: a client that drops the floor, or drops the
schema claim, returns *exactly the right rows* on a single node, so nothing
about the answer distinguishes it. The request has to be looked at directly.
`schema_test.go` already says this in its own words — "the claim is attached at
ten call sites, and a missed one is invisible" — and a new call site is now
eleven.

The `Next()` bug reproduced as an eight-and-a-half-minute 100% CPU spin
(`slate.test` at `8:32.41` in `top`) with zero test output, which is what sent
me looking for a hang rather than a loop.

## What this does not do

The freshness test asserts the floor is **on the request**, not that a stale
replica honours it. Nothing in the Go suite covers a replica at all — this is
the first freshness assertion in it, and it is the weaker of the two available.
The strong version belongs with `examples/deployed`, which does run a replica.

No TypeScript `related` yet, so the conformance runner still does not cover
relations — the thing this was for. That is the next commit, not this one.

The three clients now key their group lookups on a serialised `Value`, and
nothing asserts the three agree on that encoding. They would only disagree over
a field the encoders order differently, which a flat oneof does not have; if
`Value` ever grows a map, the conformance runner is where that should be caught
and it does not look at relations yet.
