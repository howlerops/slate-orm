# Go and TypeScript learned to read the check metadata Python already read, and a conformance case now makes the three agree against a live server

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `clients/go/slate/error.go`, `clients/typescript/src/details.ts`,
  `clients/typescript/src/errors.ts`, their test suites,
  `examples/explorer/backends/{go,node,python}`,
  `examples/explorer/conformance/conformance.py`
- **Kind:** feature

## What changed

`Error.Violations` in Go and `SlateError.violations` in TypeScript, each
decoded from `ErrorInfo.metadata` the way `check_failures_of` already did it in
Python: read the `violations` count, then `check.N`, `column.N`, `message.N`
counting up from zero. Both refuse to return a prefix when the count and the
keys disagree, both fall back to the unindexed `check`/`column`/`message` triple
a server too old to send a count would send, and both return nothing for a
failure whose reason is not `CHECK_VIOLATION`. Each of the three client suites
now decodes the *same* captured blob. Then a route — `/api/bad-status` — in all
three demo adapters, which writes a `shipments` row whose status no CHECK
admits, and a conformance case that compares what the three clients made of the
refusal.

## Why

The Q4 entry closed by admitting the gap in its own words: "Go and TypeScript
still parse nothing … the work is transcribing one decoder twice — it is not
done, and the three-client parity this repository usually holds to is broken
here until it is." A caller in Go or TypeScript holding a refused form had the
reason token and a sentence of prose, and the only way to put a message beside
the field it is about was a regular expression over that prose, which lasts
until somebody rewords it.

The conformance case is a separate argument and the one worth more. Every other
refusal in the corpus is compared on `kind` and `reason`, two strings the server
hands over whole; there is nothing for a client to get wrong beyond dropping
them. `violations` is the first thing in this protocol that each client *parses*,
out of a map whose shape is documented in prose and declared nowhere. Three
hand-written parsers of one undeclared shape is the most drift-prone thing in
these clients, and until this case existed each was checked only against a
recording of the bytes. A recording cannot notice that the server started
indexing from one.

## Alternatives rejected

**Leave Go and TypeScript with the token.** Free, and defensible: the token
already says `CHECK_VIOLATION`, so a form *can* branch. What it cannot do is say
which field. The whole point of V1 and V2 — a check carrying a column and a
message, and the server collecting every failing check rather than the first —
was one round trip per refused form instead of one per bad field, and that
payoff is a client-side decode or it is nothing. Two of three clients not having
it means the feature half-exists.

**Return the raw `metadata` map.** Much less code in all three, and it was the
documented position: `reasonOf`'s comment said the map is "deliberately not
returned" because the keys differ per variant. That objection is still right and
is why the map is still not returned. What changed is that the check-violation
keys are *specified* — a count and three indexed families — so there is exactly
one shape a client can promise something about. Both new decoders read that
shape into typed values and hand nobody the dictionary, which answers the
original objection rather than overruling it.

**Walk the map for `check.*` keys instead of counting up.** Shorter, and it
removes the dependence on the `violations` key. It is also wrong at eleven
failures: the metadata is string-keyed, so `check.10` sorts between `check.1` and
`check.2`. Demonstrated rather than argued — see the mutation below, which the
Go suite catches three ways because Go randomises map iteration and so fails
the same test run to run.

**Spell an absent column `None` in all three.** Python does; Go cannot without
a pointer, which is worse for a struct a caller renders. So Python keeps `None`
and Go and TypeScript use `""`, and the three demo adapters flatten both to `""`
in their JSON — the same normalisation `kindName`/`kind_name` already does for
status codes. Recorded because it is a real divergence in the surface, not a
detail: a caller porting between the clients meets it.

**Compare `violations` with a `MUST_DIFFER` pair instead of a refusal case.**
There is no natural pair — a check violation has no "the same request without
the flag" twin the way `includeDeleted` does. See what this does not do, which
is honest about what the refusal case therefore cannot see.

## Evidence

Five mutations of the Go decoder, each restored and re-verified, run against
`go test ./slate/`:

| mutation | result |
| --- | --- |
| drop the `reason != "CHECK_VIOLATION"` guard | caught — `TestCheckShapedMetadataUnderAnotherReasonIsIgnored` |
| return the prefix instead of nothing on a gap | caught — `TestACountTheKeysDoNotMatchYieldsNothing` |
| stop wiring `violationsOf` onto the error | caught — `TestFromRPCCarriesTheViolationsOntoTheError` |
| walk the map for `check.*` instead of counting up | caught — three tests, including `TestTheOrderIsTheSchemasAndNotTheMaps` |
| drop the unindexed fallback | caught — `TestAServerSendingOnlyTheUnindexedPairYieldsOne` |

The same five against TypeScript, run with `npm test` in `clients/typescript`,
caught by the corresponding named tests: 1, 1, 1, 4 and 1 failures out of 163.
Zero survivors either language.

One of these lied the first time and is worth recording: the "return the prefix"
mutation came back green, because the `-run` filter I used did not match
`TestACountTheKeysDoNotMatchYieldsNothing`. A mutation probe whose test never
ran reports exactly what a caught mutation reports if you only read the exit
status. Re-run with the test in the filter, it failed. The same class of
self-deception as the half-applied `returning` mutation two entries back.

Conformance: 97 cases, three SDKs agreeing on all of them, with the new case
among them. Then the load-bearing probe — `Violations: violationsOf(st)` in the
Go client replaced with `nil`, everything else untouched:

    a write the schema's CHECK refuses (app): the adapters disagree
        go      {... "reason": "CHECK_VIOLATION", "violations": []}
        node    {... "violations": [{"check": "status_known", "column": "status",
                                     "message": "Status must be pending, shipped or delivered."}]}
        python  {... "violations": [{"check": "status_known", "column": "status",
                                     "message": "Status must be pending, shipped or delivered."}]}

Which is the case doing its job: one client stopped parsing and the other two
said so. Restored, and the run is back to 97 agreeing.

And the complementary probe, because the limitation below is a claim and claims
here get demonstrated: all *three* clients mutated to report no violations at
once — Go to `nil`, TypeScript to `[]`, Python to `[]` — and the run reported
**97 cases: the three SDKs agree on all of them**. It is invisible, exactly as
stated. Each client's own unit suite fails under that same mutation, which is
the half that covers it.

## What this does not do

**It does not catch all three clients losing the field at once.** The new case
is in `EXPECTED_REFUSALS`, which asserts only that the answer *is* a refusal;
three clients reporting `violations: []` agree perfectly. This is the same blind
spot `MUST_DIFFER` exists for, and there is no pair to compare here. What covers
it instead is the other half: each client's unit suite decodes a blob the server
itself emitted, so a client that stopped parsing fails there. The two mechanisms
are complementary and neither is sufficient — units catch unanimous loss,
conformance catches drift between clients.

**The live case exercises one failing check, not three.** `shipments` declares
only `status_known`, so `/api/bad-status` reaches the single-failure shape. The
three-failure shape — including the cross-column check that has no column and no
message — is only ever decoded from the captured fixture. Adding a second CHECK
to `shipments` would close that, at the cost of a demo table that exists to be
refused.

**Nothing validates against the published checks before sending.** V3 publishes
them in `--print-schema` and codegen narrows an enum column's type from them,
but no client refuses a row locally; every bad row still costs a round trip.
That is the same gap the Q4 entry left and this does not touch it.

**A batched write's check violation carries no violations.** `fromBatchError`
and its two counterparts build an error from a code, a message and a reason
inside a successful response — there is no details blob in that path, so there
is nothing to decode. A caller batching a form's write gets the token and the
prose, which is where all three clients were before this change.
