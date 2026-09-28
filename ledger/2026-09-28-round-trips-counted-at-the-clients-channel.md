# Round trips, counted at the client's own channel

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `clients/python/tests/test_round_trips.py`, `docs/caveat-status.json`
- **Kind:** test

## What changed

`clients/python/tests/test_round_trips.py`: a `grpc` interceptor on the
client's channel, counting calls per method, and three tests over a real head
node.

| what the caller does | calls |
| --- | --- |
| twenty inserts, one at a time | 20 × `Insert` |
| the same twenty in one `Batch` | 1 × `Batch`, 0 × `Insert` |
| four keyset pages of five | 4 × `Query` |

Two `open` caveats are narrowed against it. Both asked for the same thing and
said so in the same words — "no measurement from a client", "nothing measures
the cost on the wire" — and both had been answered only by a Rust wire test or
a kernel figure.

## Why

The inherited number was 9.4×, a *ratio of durations* from a Rust test. The
caveat's own objection was that the clients add per-request work the Rust test
does not, so the ratio does not transfer. Timing the clients would have
answered it and would have been the wrong instrument: earlier today a timing
claim from this container reversed on a CI runner
(`ledger/2026-09-28-a-measurement-that-reversed-under-ci.md`), and the lesson
was that a count has no spread. Twenty writes are twenty requests or they are
one; no runner load changes that.

So this counts, and says plainly what counting cannot see. The caveat's other
half — "the clients add per-request work … and that work is not saved by
batching — it is per operation either way" — is *still true* and is now the
recorded residual. Encoding twenty rows costs twenty encodings in a batch too.

**The instrument needed nothing from the client to exist.** `Client` already
takes `channel=`, for callers who want TLS; an intercepted channel goes in the
same door. That it was this cheap is the reason the caveat stood for twelve
days: nobody looked.

## What the instrument taught, which is the part worth reading

Three things came out of running it that reading could not have given:

**The RPC names are not the method names.** A single insert is `Insert`, not
`Write`; a keyset page is `Query`, not `Page`. Both assertions were written
against the wrong name and failed with `Counter({'Insert': 20})` — a failure
message that hands you the answer. A test written from the client's API surface
guesses at the wire; one that counts the wire reports it.

**It broke five tests in another file.** The first version left forty rows in
`docs`, and `test_streaming.py` — which counts rows rather than naming them —
went red. The file passed alone and the suite failed, which is exactly the shape
`CLAUDE.md` warns about under *running one file of a suite is not running the
suite*. The fixture now deletes its whole key range in teardown.

**The paging assertion was self-fulfilling and a mutation caught it.** It read
`counter.calls["Query"] == pages`, where `pages` was incremented by the loop
doing the paging — so it held for any number of pages, including one. The
mutation that shrank the loop survived, which is the survivor rule doing its
job. Both sides are literals now, and the test additionally asserts it saw
`PAGES * PAGE_SIZE` rows, which is what makes shrinking the loop fail.

That third one is the general lesson: **an assertion whose expected value is
computed by the code under test is not an assertion.** It passed, it read
well, and it tested nothing.

## Alternatives rejected

**Scrape the server's `slate_requests_total`.** The daemon exports it and its
own suite scrapes it, so this looked free. Rejected twice over: the Python
suite's server is the testserver, which serves no `/metrics`; and a server-side
count answers "how many requests arrived", which equals "how many the client
sent" only if the client sent what it thinks it sent. The caveat is about the
client, so the instrument belongs in the client.

**Time it after all, with many repetitions.** More repetitions do not fix a
between-machine difference, which is what today's reversal was. And a ratio of
durations on a loopback socket against an in-memory store is mostly scheduling
— the thing the original 9.4× was, and the reason it did not transfer.

**Instrument all three clients now.** Go and TypeScript both have interceptor
equivalents and the same test would fit. Not done here because one client
settles whether the claim is true of *a* client, which is what the caveat
disputed, and three settles whether the clients agree — a different question,
and the conformance runner's job. Recorded as the residual rather than implied.

## Evidence

- `python3 -m pytest clients/python/tests/test_round_trips.py -q`: 3 passed.
  The counts in the table are the assertions.
- `cd clients/python && python3 -m pytest -q`: **338 passed**, the whole suite.
  Run because the first version of this file failed exactly here — five
  failures in `test_streaming.py` with this file passing alone.
- `python3 scripts/mutate.py`, record
  `ledger/mutations/20260928T224305-clients-python-tests-test-round-trips-py.json`:
  three mutations, two caught and one survivor.
  - Dropping `self._count` from the unary interceptor → both write tests fail.
  - Sending the batch's rows one at a time instead → the batch test fails.
  - Shrinking the paging loop from four to one → **survived**, which is how the
    self-fulfilling assertion was found.
- `python3 scripts/mutate.py`, record
  `ledger/mutations/20260928T224441-clients-python-tests-test-round-trips-py.json`:
  three mutations after the fix, two caught and one expected survivor.
  - The same shrunken paging loop → now caught.
  - Dropping `self._count` from the stream interceptor → the paging test fails.
  - Making the janitor delete nothing → **expected survivor**, recorded with
    its reason: the damage is to another file's tests, so the suite catches it
    and this command cannot.

## What this does not do

**It counts calls, not work.** A client that made one `Batch` call carrying
twenty separately-encoded rows passes every assertion here, which is correct —
that *is* one round trip — and is also the whole of the caveat's surviving
half. Nothing here measures encoding cost.

**Only Python is instrumented.** The claim "batching helps *the clients*" is
now shown for one of three. Go's `grpc.WithUnaryInterceptor` and TypeScript's
interceptor option would each take an afternoon; whether the three agree is the
conformance runner's question and it does not ask this one.

**It says nothing about latency or bytes.** Four calls for four pages is the
count; what those four calls cost over a network, and how many bytes they
carry, is `examples/deployed`'s to answer and it still does not exercise
paging. The README's 495-against-5 key-value figure remains the only
measurement of what a page reads from the *store*, and this does not touch it.
