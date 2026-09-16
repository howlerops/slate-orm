# Relations in the TypeScript client, and a vendored `.proto` that had drifted the moment the RPC was added

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `clients/typescript/src/client.ts`, `src/value.ts`, `src/index.ts`, `test/related.test.ts`, `proto/slate/v1/records.proto`
- **Kind:** feature

## What changed

`Session.related` and `Transaction.related` in the TypeScript client, over the
`Related` RPC. A `Relation { on, through, way }` names the relationship by the
foreign key that declares it, exactly as Go and Python do; `way` defaults to
`"children"`. Ten tests, including an oracle against a filtered read per parent.

`valueKey` is new and exported: a `Map` key that is the same string for two
values exactly when they are the same value, kind included.

The bundled copy of `records.proto` is re-synced from
`crates/slate-server/proto`. It had been stale since the RPC was added, which
made every TypeScript test here fail with *"this server has no Related
method"* — the client resolves methods from the bundled file at runtime.

## Why

Relations existed on the wire and in two of three clients. The three-SDK
conformance runner compares the clients against each other, so a feature
missing from one is a feature nothing cross-checks. `docs/orm-comparison.md`
named that runner as the assertion that matters most for this piece.

`valueKey` exists because the response groups by value and the caller asked by
value, and the two have to be matched. Go and Python key on the serialised
protobuf; a `Value` here has already been decoded by the time `related` sees
it, so re-encoding it only to compare would mean carrying the wire shape
around for nothing.

## Alternatives rejected

**Match the groups pairwise with `valuesEqual`**, which already exists and
needs no new function. It is O(groups × keys): a caller resolving a page of two
hundred parents would pay forty thousand comparisons to save one round trip,
which is a strange trade to make on the way back out of the call that exists to
save work. `valueKey` makes it linear.

**Key the map on `JSON.stringify(valueToWire(value))`.** No new function and
deterministic in practice. It bakes the wire shape into a lookup that has
nothing to do with the wire, and `Buffer` and `bigint` both serialise through
`JSON.stringify` in ways that are incidental rather than chosen — `bigint`
throws outright unless something has been done to it.

**Two request paths, one for a session and one for a transaction**, which is
how the first version was written and how several other methods in this file
are written. A mutation dropping the schema claim from the transaction path
survived the whole suite: the duplicate site was not covered, and neither was
any other read through it. Collapsed into one `relatedIn` taking an optional
transaction. The remaining duplicated paths in this file are older than this
change and are not touched here.

**Leave the bundled proto to the drift test.** `test/proto.test.ts` does catch
it, and would have caught it in CI. It did not catch it *here* because nothing
ran the TypeScript suite between adding the RPC and adding this client — the
check existed and had not fired, which is the same shape as the CI workflow
that had never run. Copying it across is part of adding an RPC, not a thing to
be reminded of later.

## Evidence

Ten tests pass against a real node; the whole TypeScript suite is 83 tests,
all passing. `tsc -p tsconfig.json` and the build are clean.

Mutation run, eight mutations over two rounds:

| mutation | first run | after |
| --- | --- | --- |
| direction ignored, always children | killed | killed |
| the default direction becomes parents | killed | killed |
| transaction dropped | killed | killed |
| schema claim dropped, session path | *false survivor* | killed |
| schema claim dropped, transaction path | **survived** | path removed |
| a missing group yields the first group | killed | killed |
| groups in the server's order | killed | killed |
| the kind left out of the grouping key | **survived** | killed |

Two notes on that table, both worth more than the counts.

The session-path claim mutation reported as a survivor on the first run and was
not one. `freshness: this.#freshness(),\n schema: this.#client.claim(table),`
occurs at ten call sites in `client.ts`, and a first-occurrence replace hit
`get()` — which `related.test.ts` does not exercise. The check under test was
never mutated. Re-run against a longer unique context, it dies to
`the schema claim rides on a relationship load`. This is the second time in
two days a first-occurrence replace has produced a confident wrong answer in a
mutation run; the tell both times was that the surviving mutation was in a
region no test in the file mentions.

The kind mutation is a real survivor and the test written for it is a unit
test, not an integration one, because *within a relationship load the kind
cannot vary*: every key in one call comes from one column of one type, so
dropping the kind from the key changes no answer any `related` call can
produce. `valueKey` is exported, though, and a caller keying its own map across
two tables is exactly where `int(7)` and `uint(7)` would collide. The test
asserts that directly, and says in a comment why it is not an end-to-end one.

## What this does not do

The conformance runner still does not exercise relations. All three clients
have the call now, which is what that needs, but the runner has not been
extended and so nothing yet compares the three against each other — the thing
this whole piece was for. Next commit.

No freshness-floor assertion on the TypeScript side. The Go client got one
through a gRPC interceptor; the equivalent here would mean wrapping
`Client.call`, and the floor is built in one shared place per client, so the
Go test covers the shape of the mistake rather than every instance of it. That
is a weaker claim than it sounds and is stated rather than glossed.

`valueKey` is not used anywhere but `related`. It is exported because a caller
mapping rows onto parents needs the same key function the client used, and an
unexported one would push them to invent a worse one.
