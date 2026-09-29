# Bytes do not need a network

## What changed

`Counting` in `clients/python/tests/test_round_trips.py` now weighs every
request as well as counting it — `request.ByteSize()`, in the same
interceptor, on the same workloads — and two tests read the numbers:

- `test_batching_saves_round_trips_and_costs_bytes`
- `test_paging_by_cursor_costs_more_bytes_than_paging_by_offset`

Two verdicts moved from `open` to `narrowed`, their residual being the half
that genuinely needs a network.

## Why

Five caveats across three entries said this work *"says nothing about latency
or bytes"*, and all five deferred **both** to `examples/deployed`, which needs
MinIO, which this container does not have.

That conflates two questions. A latency needs a network. A byte does not:
`ByteSize()` is the serialized length of the message about to be sent, it is
exact, and it has no spread — the same request weighs the same on every run and
every machine. The instrument was already on the channel. Weighing cost one
line and closes the half that was never blocked.

**Both numbers came out against the phrase they were meant to confirm.**

**Batching.** "A batch is a round trip" is true and this file already showed
it: twenty writes, one call. The unstated companion — that batching is
therefore cheaper — is false on the request. A batch of twenty costs *more*
bytes than twenty singles. At five sizes, twice each:

| rows | batch | singles | delta |
| --- | --- | --- | --- |
| 1 | 71 | 65 | +6 |
| 2 | 140 | 130 | +10 |
| 5 | 347 | 325 | +22 |
| 10 | 692 | 650 | +42 |
| 20 | 1382 | 1300 | +82 |

Exactly `4n + 2`: four bytes per statement for the tag and length prefix it
gains from being nested in a `repeated` field, two for the envelope. So the
honest version is **twenty writes go from twenty requests to one, and from
1300 bytes to 1382**. On any real link that is an enormous win — a round trip
costs a latency, 82 bytes costs nothing — but it is a trade, and "cheaper on
the wire" did not say so.

**Paging.** `2026-09-29-the-days-own-caveats-read-and-a-phrase-that-did-not-survive-counting.md`
asked precisely this: *"an offset page's request carries an integer where a
cursor's carries a key, and nothing weighs them."* Weighed: four cursor pages
are **215 bytes against 198** for four offset pages, +17, about 9%. Same
direction as the batch finding, and the same correction to the same phrase:
what keyset paging saves is *store* reads — the README's 495 key-value pairs by
offset against 5 by cursor — and that saving is on the server. On the request
it is a small loss.

Neither of these is a reason to stop batching or stop using cursors. Both are
reasons to stop saying "cheaper on the wire" when the saving is a round trip
or a store read.

## Alternatives rejected

**Wait for `examples/deployed` and measure bytes there.** What all five
caveats assumed. It is the right place for *latency* and the wrong place for
bytes: a deployed run adds MinIO, a network and a scheduler to a number that
is a property of the message, and the extra machinery could only add noise to
something that has none. It also means the measurement never happens here,
which is what five caveats deferring to it for two months demonstrates.

**Measure latency here anyway, with a spread.** Tempting, and this session has
already been burned: `2026-09-28-a-measurement-that-reversed-under-ci.md` is a
timing claim that reversed on a different machine. A loopback duration against
an in-memory store is mostly scheduling, and reporting one with error bars
would dress up the same non-measurement. The residual says latency is not
measured rather than pretending a number.

**Count response bytes too.** The interceptor sees the request; the response
of a server-streaming read arrives as an iterator the test would have to drain
and weigh. Worth doing and not done — the caveats are about what batching and
paging cost the *caller to send*, and mixing in a response size that scales
with the rows returned would make the batch comparison meaningless (the same
twenty rows come back either way).

**Assert the literal totals, 1382 and 1300.** The first draft did, with numbers
from a throwaway run at a different key range — 1402 and 1320 — and they were
wrong for this fixture. The totals move with the rows because the key is a
varint and the `note` is a fixed string; the *delta* did not move at all. So
the assertion is the formula and the totals are narration, labelled as such.
Catching that was the whole value of re-running the measurement inside the
real fixture rather than trusting the scratch one.

## Evidence

`cd clients/python && python3 -m pytest -q`: **349 passed**, up from 347. The
whole suite, not the one file — the interceptor is shared and `_count` changed
signature.

The tables above are the observed output of the committed tests, re-measured
inside the real fixture after the scratch numbers proved to be from a different
key range.

Ten mutation cases over three runs, eight of them distinct:
`ledger/mutations/20260929T064611-clients-python-tests-test-round-trips-py.json`,
`ledger/mutations/20260929T064700-clients-python-tests-test-round-trips-py.json`,
`ledger/mutations/20260929T064729-clients-python-tests-test-round-trips-py.json`.
The repeats are the envelope term, re-scored after the redundant assertion was
removed, and the paging bound, re-scored with `expect_survivor`.

| mutation | outcome |
| --- | --- |
| the interceptor weighs nothing | caught, both tests |
| `weight()` reads calls instead of bytes | caught, both tests |
| the envelope term dropped from the formula | caught |
| the per-statement term dropped | caught |
| the batch sends singles instead | caught |
| the cursor dropped from the paged request | caught |
| the batch direction assertion weakened | **survived — removed** |
| the paging range widened to admit equality | **survived — expected** |

The first survivor was a redundant assertion. `assert batched > singly` cannot
fail while `batched - singly == 4 * ROWS + 2` holds, because that is positive
for any `ROWS`. It was written to state the finding and the finding belongs in
the docstring, so it is gone and a comment says why.

The second is recorded with `expect_survivor`: loosening a bound cannot fail
against a value that already satisfies the tighter one. The bound's job is to
refuse *zero*, which is what a dropped cursor produces, and that shape was
mutated directly in the same run and was caught. Scoring a bound by loosening
it measures the fixture, not the rule.

## What this does not do

**It measures no latency, and that half is still blocked.** A duration on a
loopback socket against an in-memory store is mostly scheduling. The residual
on both narrowed caveats names it.

**It measures the request only.** Not the response, not the HTTP/2 framing,
not the headers, not the identity metadata. `ByteSize()` is the protobuf
payload. The class docstring says so rather than letting "bytes" imply "on the
wire", because the framing is real and this does not see it.

**Only the Python client weighs.** Go's `related_test.go` and
`round_trip_test.go` and TypeScript's `roundTrip.test.ts` count and do not
weigh, so the three-client comparison is still counts alone. Those two caveats
stay open and now say what is missing rather than deferring to MinIO.

**It does not explain the `4n + 2` from the wire format.** The arithmetic fits
five points exactly and the explanation — a tag byte and a length prefix per
nested statement — is read off the protobuf encoding rules rather than
demonstrated by decoding a message field by field. A different framing that
also produced `4n + 2` would be indistinguishable here.

**Nothing weighs a relation load, a join or an aggregate.** The two workloads
measured are the two the caveats named. The others have counts and no weights.
