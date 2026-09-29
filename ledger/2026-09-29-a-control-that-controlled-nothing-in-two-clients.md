# A control that controlled nothing, in two clients

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `clients/typescript/test/{harness.ts,related.test.ts}`, `clients/go/slate/related_test.go`
- **Kind:** fix

## What changed

The TypeScript client asserts its freshness floor on the wire now, through an
interceptor — the check Go has had since 2026-09-16 and this client had no way
to write.

And the Go test it was ported from was wrong. Its control — *a non-monotonic
session sends no floor* — passed whether or not the client consulted
`s.monotonic`, because a fresh session has no watermark either way. Dropping
that half of the condition survives. Both now write first, so the watermark
exists and only the flag distinguishes.

`Serving.client()` in the TypeScript harness takes channel options, which is
what reaches `Client.connect`'s fourth argument.

## Why

> **No freshness-floor assertion on the TypeScript side.** The Go client got
> one through a gRPC interceptor; the equivalent here would mean wrapping
> `Client.call`, and the floor is built in one shared place per client, so the
> Go test covers the shape of the mistake rather than every instance of it.
> That is a weaker claim than it sounds and is stated rather than glossed.
> — `ledger/2026-09-16-relations-in-typescript-and-a-proto-copy-that-had-drifted.md`

It was a weaker claim than it sounds, and the reason it was made is now gone:
`Client.connect` grew an `options` argument on 2026-09-28 specifically because
this client had no door to pass an interceptor through. The caveat said
"wrapping `Client.call`" because that was the only way in at the time. It costs
nine lines through the door.

The floor is a property of the *request*, not the answer. A client that dropped
it returns exactly the right rows; nothing about the result distinguishes it.
That is why it has to be read off the wire, and why the caveat mattered.

**And porting it is what found the defect.** Writing the control in TypeScript,
mutating it, and watching the mutation survive sent me to look at the Go test —
which has the same shape and the same hole, unnoticed for a fortnight.

## Alternatives rejected

**Assert on the answer instead.** There is nothing to assert on: the rows are
identical with and without the floor, on a single-node test fixture where no
replica can lag. That is the caveat's whole point.

**Wrap `Client.call`, as the caveat proposed.** A test-only subclass or a
monkey-patch over a `#private` method. It reaches the same messages and it
tests a client this package does not ship — and the door exists now, so the
reason for the workaround does not.

**Leave the Go control alone and only fix the new one.** Tempting, because the
Go test is green and out of scope. It is green for the wrong reason, which is
the thing this repository treats as a defect rather than as tidiness: a control
that cannot fail is a control that is not there, and the next person to read it
would take the same assurance from it that I did.

**Make the loose session's watermark explicit with `observe()`.** Both clients
can fold a token in directly, which is fewer lines than a write. A write is
what a real caller does and it is what the monotonic half of the test already
does, so the two halves differ in one thing rather than two.

## Evidence

- `clients/typescript`: `npx tsx --test "test/*.test.ts"` — **201 pass, 0 fail**
  (200 before).
- `clients/go`: `go test ./...` — **ok**, 9.6s.
- **Six mutation runs across the two clients' `freshness`. Two survived, and
  the survival is the finding.**
  - TypeScript
    (`ledger/mutations/20260929T043143-clients-typescript-src-client-ts.json`,
    outcome `problems`): never sending the floor was caught; **sending it from
    a non-monotonic session survived**. That is the vacuous control. Caught
    after the fix in
    `ledger/mutations/20260929T043223-clients-typescript-src-client-ts.json`.
  - Go
    (`ledger/mutations/20260929T043242-clients-go-slate-client-go.json`,
    outcome `problems`): the same mutation survived against the *existing*
    test, which is how the defect was confirmed rather than inferred. Both
    mutations are caught in
    `ledger/mutations/20260929T043311-clients-go-slate-client-go.json` after
    the same fix.
- Each caught mutation names `TestRelatedCarriesTheFreshnessFloor` or
  `a monotonic session sends a freshness floor and a loose one does not` and
  nothing else.

## What this does not do

**It covers `Related` and no other RPC.** The floor is built in one shared
place per client — `#freshness()` and `Session.freshness()` — so a mutation
there breaks every read, and this test is where it surfaces. That is the
weaker claim the 09-16 caveat made about Go, restated, and it is still the
honest one: nothing checks that `Query` or `Get` actually call the shared
helper.

**The Python client still has no such assertion.** It has a `Counting`
interceptor for round trips, so the door and the machinery are both there; what
is missing is the case. Three clients, two of them covered.

**The harness's staleness guard fired twice on unrelated crates today.** It
walks every `.rs`, `.toml` and `.proto` in the tree, so a mutation run's
restore of `crates/slate-orm/src/field.rs` — and, separately, an edit to
`clients/python/testserver`, which `slate-serverd` does not depend on — both
made it refuse a binary that was current. Its own comment calls modification
times crude and says they catch the whole of the real failure, which is true
and is the safe direction to err in. Unchanged here, and named because two
false positives in one session is the first evidence that the crudeness costs
anything.
