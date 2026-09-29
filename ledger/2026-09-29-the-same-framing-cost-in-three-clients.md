# The same framing cost, in three clients

## What changed

Go and TypeScript weigh their requests now, as Python started doing an hour
ago, and each has the same two tests:

- `TestBatchingSavesRoundTripsAndCostsBytes` /
  `batching saves round trips and costs bytes, here too`
- `TestACursorPageCostsMoreBytesThanAnOffsetPage` /
  `a cursor page costs more bytes than an offset page, here too`

Go needed a `sendingStream` wrapper in `related_test.go` before it could weigh
a read at all, and the report moved from the stream's *open* to its `SendMsg`.
TypeScript's `Counting` gained a `sendMessage` hook and `counted()` now builds
its client through the harness.

## Why

`ledger/2026-09-29-bytes-do-not-need-a-network.md` found two things in Python
and left this open: *"Only the Python client weighs … so the three-client
comparison is still counts alone."*

That is the gap worth closing, because it is the one no comparison of answers
can reach. Three clients sending the same *number* of requests can build
different ones, and every answer-comparing case in the conformance suite
passes either way. `related.test.ts` found exactly that shape in the freshness
floor a few hours ago.

**The three agree, and the agreement is sharper than expected.** Python's
twenty rows batch to 1382 bytes against 1300 sent singly. Go's twenty rows —
a different table, different values, different key range — batch to 502
against 420. Different totals, and **the same delta: +82**, which is
`4n + 2` in both. The framing cost is a property of the wire format and the
payload is a property of the fixture, and measuring it twice at three times
the size is what separates those.

Paging agrees in direction and not in size: four cursor pages cost 215 against
198 in Python (+17) and 80 against 66 in Go (+14). Both say a cursor page is
heavier than an offset page; the deltas differ because the keys do, a cursor
being the last row's primary key as a varint. That is why the assertion is a
range in all three and the numbers are narration.

**Go could count a read and never weigh one**, and the comment saying so had
been standing since relations landed: *"No request message: a stream is opened
before anything is sent, so a caller wanting the body has to wrap SendMsg.
Nothing needs it yet."* Accurate, and it stopped being true the moment bytes
were asked for. The wrapper is eight lines.

Moving the report from the open to the send is the part worth reading twice.
Reporting at both would have **double-counted every read** — and
`TestPagingByCursorIsOneRequestPerPage` would have gone from 4 to 8. A
server-streaming RPC sends its one request immediately after opening, so the
count is identical either way and the message is no longer nil. The mutation
that puts the open's report back is in the record, and that test catches it.

## Alternatives rejected

**Compare the three clients' byte counts against each other, in the
conformance runner.** The strongest version: one number, three clients, an
oracle. Rejected for now because the three adapters write different rows
against different tables, so the numbers *should* differ and a comparison
would be asserting on the fixtures. Making them comparable means one shared
workload with identical rows, which is a real change to
`/api/round-trips` and worth doing separately. The cross-client agreement
this entry reports is the *delta*, which is fixture-independent, and that
comparison is made by a person reading two tests rather than by a runner.

**Weigh in Go by serializing the request a second time.** `proto.Size` is what
the tests use and it is the message's length, not a re-encode — but the
tempting shortcut was to skip the stream wrapper and weigh only unary calls,
leaving reads at zero. That is the "half-blind instrument" the Go entry was
named for, one level down, and it would have made the paging test assert on a
difference between two zeros.

**Have TypeScript re-encode the message itself.** `options.method_definition.
requestSerialize(message).length` is the channel's own serializer — the exact
buffer grpc-js is about to write. A hand-rolled encode would be a second
implementation that could drift from what is sent, which is the whole failure
this instrument exists to avoid.

**Leave `counted()` building its own client.** It called `Client.connect`
directly, so `serving.stop()` killed the child with the channel still open.
That produced four `14 UNAVAILABLE: Connection dropped` errors *after* a test
had passed — node's runner turns that into a file-level failure with every
subtest green, which is the most confusing shape a failure has. Routing
through `serving.client()` fixes it for every test in the file, and that
method grew its options argument this morning for the freshness interceptor;
this is its second caller.

## Evidence

Go: `GOTOOLCHAIN=local go test ./slate -count=1` — ok, whole package.
Measured, logged once and then removed: `GO batch 502 singly 420 delta +82`
and `GO cursor 80 offset 66 delta +14`.

TypeScript: `npm test` — **204 passing, 0 failing**. The file went from three
tests to five.

Three mutations of `related_test.go`,
`ledger/mutations/20260929T070354-clients-go-slate-related-test-go.json`, all
caught:

| mutation | caught by |
| --- | --- |
| the stream wrapper is not installed | both paging tests |
| `SendMsg` reports nothing | both paging tests |
| the open reports again, double-counting | `TestPagingByCursorIsOneRequestPerPage` |

The third is the one that matters: it is the shape the change had to avoid,
and the existing count test catches it without being touched.

**A self-inflicted loss worth recording.** The two Go tests were written,
passing, and then destroyed by a `git checkout -- slate/round_trip_test.go`
run to undo two temporary `t.Logf` lines in the same file. The file was
untracked-modified, so the checkout discarded the tests with the logging. They
were rewritten from the transcript and the measured numbers survived in the
log output. The lesson is the obvious one and it is written here because it
cost fifteen minutes: `git checkout --` on a file with uncommitted work is a
delete, and a temporary edit for a measurement belongs in a copy.

## What this does not do

**It measures no latency, in any client.** Unchanged and still blocked on a
network; `examples/deployed` needs MinIO.

**It compares the three by hand.** Nothing runs all three against one workload
and asserts the numbers relate. The deltas agreeing at `+82` is a fact two
tests report and a person noticed, not a check.

**The request only, again.** Not the response, not HTTP/2 framing, not the
headers. Each client's instrument sees what its own channel is about to send.

**The Go paging test is the only thing that drains an offset read**, and it
drains it with `Collect()`. A caller who abandoned the stream half way would
still have sent the request and would weigh the same, which is right, but
nothing tests that.

**Nothing weighs a relation load, a join or an aggregate**, in any of the
three. Same as before: the workloads weighed are the two the caveats named.
