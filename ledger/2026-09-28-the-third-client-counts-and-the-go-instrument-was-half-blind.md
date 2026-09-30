# Round trips are counted in all three clients now, and adding the third one found that the second's instrument could not see a read at all

- **Date:** 2026-09-28
- **Author:** Claude Code, on task J3's tail
- **Touches:** `clients/typescript/src/client.ts`,
  `clients/typescript/test/roundTrip.test.ts`,
  `clients/go/slate/round_trip_test.go`, `clients/go/slate/related_test.go`,
  `scripts/check_closed_caveats.py`, `docs/caveat-status.json`
- **Kind:** feature

## What changed

`Client.connect` in the TypeScript client takes a fourth argument, a
`grpc.ChannelOptions`, forwarded to the channel untouched. It is additive and
defaults to `{}`, so no existing call site changes. On top of it,
`clients/typescript/test/roundTrip.test.ts` installs a counting interceptor and
asserts three counts: twenty singles are twenty `Insert` calls, the same twenty
rows in a batch are one `Batch` and no `Insert`, and four keyset pages are four
`Query` calls.

Writing the Go counterpart of that third count then found that
`dialRecording` — the Go suite's request counter, in place since relations
landed — held only `grpc.WithChainUnaryInterceptor`. `Query` is
server-streaming, so it reported **zero** `/Query` calls for four pages that
had plainly happened. `grpc.WithChainStreamInterceptor` is added beside it, and
`TestPagingByCursorIsOneRequestPerPage` is the test that now passes because of
it.

## Why

Two entries earlier today recorded that a batch being one round trip was
inherited from a Rust wire test rather than shown from a client. Python got a
counter, Go turned out to have had one for relations and gained a batch count,
and the residual was written down as *"TypeScript has no request counter at
all, and only Python counts paging."* Both halves of that sentence are now
false.

TypeScript could not have had one. Python's `Client` takes `channel=` and Go's
`Dial` takes `...grpc.DialOption`, so a caller in two languages of three could
configure the transport — interceptors, keepalive, message-size limits — and in
the third could not reach the channel at all. That asymmetry was the blocker
and is worth more than the test: it is the door any caller needs, not only a
test.

The Go finding is the more interesting one. The Python `Counting` class
registers *both* `grpc.UnaryUnaryClientInterceptor` and
`grpc.UnaryStreamClientInterceptor`, and its docstring says why: an interceptor
registered for only the first "would count writes and report zero for every
query, which reads as *queries are free* rather than as a hole in the
instrument." That exact hole was open in Go the whole time, written up in one
client and unnoticed in another. It only surfaced because something finally
asked the Go instrument about a read.

## Alternatives rejected

**Timing instead of counting, in any of the three.** The inherited figure was
9.4x, a ratio of durations on a loopback socket against an in-memory store,
which is mostly scheduling. A timing claim from this repository reversed
between this container and a CI runner earlier today
(`ledger/2026-09-28-a-measurement-that-reversed-under-ci.md`), and the lesson
recorded there was that a count has no spread. Twenty writes are twenty
requests or they are one, on any machine. Rejected for the same reason it was
rejected in the Python entry.

**Counting at the server's `/metrics` instead.** `slate-serverd` exports
`slate_requests_total{method=…}` and the daemon's own suite scrapes it. It
answers "how many requests arrived", which equals "how many the client sent"
only if the client sent what it thinks it sent — and the caveat is about the
client. It would also have needed no TypeScript change, which is the tell: it
would have measured around the gap rather than through it, and the missing
`options` argument would still be missing.

**Wrapping `SendMsg` in the Go stream interceptor, to record the request
message too.** The unary interceptor hands `seen` the request; the stream one
hands it `nil`, because a stream is opened before anything is sent. Recovering
the message means wrapping the returned `grpc.ClientStream`, which is thirty
lines of forwarding for a thing no caller wants — the one caller that reads the
request filters on `/Related`, which is unary. `nil` is honest about what the
interceptor saw. If a test ever needs a streamed request body, the wrapper goes
in then.

**Leaving Go's paging uncounted and closing only the TypeScript half.** The
caveat "Neither Go test counts paging" would have stayed open on a client that
already had the instrument and needed six lines of it pointed at a read. It
also would have left the unary-only hole in place, undiscovered, under a
comment claiming the interceptor covered "every unary call" — accurate, and
accurate in a way that reads as complete.

**Reusing the Python trick of filtering the paged query by a column.** Both new
tests page with no filter, because each starts a node of its own on the memory
backend and seeds nothing, so the rows written are every row in the table. The
Python version needs its filter because its server is session-scoped and
shared; copying the filter here would have been cargo-culting a constraint that
does not apply.

## Evidence

Six mutations over four runs, all caught, each naming the test that caught it.
Run through `scripts/mutate.py`, one record per run:

- One case in `ledger/mutations/20260928T230945-clients-typescript-src-client-ts.json`:
  dropping `options` from `new Records(target, credentials, options)` fails all
  three TypeScript cases — *writing n rows one at a time is n round trips*,
  *writing the same n rows in a batch is one round trip*, *paging by cursor is
  one round trip per page*. That is the production change defended: with the
  argument gone the interceptor never reaches the channel and every count is
  zero.
- Two cases in `ledger/mutations/20260928T230922-clients-typescript-test-roundtrip-test-ts.json`:
  shrinking the insert loop to one row fails *writing n rows one at a time is n
  round trips*; shrinking the paging loop to `PAGES - 1` fails *paging by cursor
  is one round trip per page*.
- One case in `ledger/mutations/20260928T231211-clients-go-slate-related-test-go.json`:
  deleting the `seen(method, nil)` call from the new stream interceptor fails
  *TestPagingByCursorIsOneRequestPerPage*. That is the half-blind instrument
  defended — without it the Go paging count is zero and reads as free.
- Two cases in `ledger/mutations/20260928T231249-clients-go-slate-round-trip-test-go.json`:
  shrinking the paging loop to `batchRoundTripPages-1` fails
  *TestPagingByCursorIsOneRequestPerPage*, and shrinking the insert loop to one
  row fails *TestABatchIsOneRequestAndSinglesAreMany*.

Both paging mutations are the check that the self-fulfilling assertion from the
Python version was not reproduced: there, `assert calls["Query"] == pages` with
`pages` incremented by the loop held for any number of pages, and a loop-shrink
survived. Here both sides of both comparisons are constants, and a loop-shrink
fails.

The counts observed, on this container against a debug `slate-serverd`:
TypeScript 20 `Insert` / 1 `Batch` with 0 `Insert` / 4 `Query` for four pages
of five; Go 20 `/Insert` / 1 `/Batch` with 0 further `/Insert` / 4 `/Query`.
No spread is reported because there is none to report — a count is not a
duration, and repeated runs give the same integer.

Suites, not single files: `clients/typescript` 199 of 199 pass (`npm test`
against a prebuilt `SLATE_SERVERD`), `clients/go` `go test ./... -count=1`
passes. The Go run matters beyond the new test, because the stream interceptor
is in a shared helper: every existing `dialRecording` caller now sees streamed
calls it did not see before, and the one that inspects the request message
filters on a unary method, so none changed behaviour.

Three caveat verdicts move to `closed`, each with a witness in
`scripts/check_closed_caveats.py` that would go away if the closure were
reverted; the guard reports 352 of 377 closed caveats witnessed, the rest
exempt.

## What this does not do

**It still says nothing about latency or bytes.** Four calls for four pages is
the count; what those four cost over a real network, and how many bytes they
carry, is `examples/deployed`'s question and it does not exercise paging. That
caveat is untouched and stays open.

**It counts calls, not work.** Twenty rows inside one batch are still twenty
schema claims and twenty value encodings in the client. A count cannot see
that, and the surviving half of the original caveat is the per-request client
work, not the request count.

**No conformance case compares the three counts against each other.** Each
client is measured against its own expectation, in its own suite. Three clients
agreeing that a batch is one call is a stronger statement than three
independent assertions of it, and the conformance runner — which compares
answers, not requests — is where that would live.

**The Go stream interceptor records no request body.** `seen` is handed `nil`
for a streamed call, for the reason above. Any future test wanting to assert
something about a streamed *request* — a freshness floor on a `Query`, say —
has to add the `ClientStream` wrapper first, and will find `nil` rather than a
wrong answer.

**Nothing checks that a new client transport keeps this door.** The three
clients happen to expose the channel three different ways now
(`channel=`, `...grpc.DialOption`, `ChannelOptions`), and nothing fails if a
fourth arrives without one. The witness roster catches the *removal* of
TypeScript's, not the absence of a future client's.
