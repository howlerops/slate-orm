# A read answers 141 times what it asks, and until now nothing had put the two numbers together

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-serverd` (`tests/observing.rs`, `Cargo.toml`),
  `docs/caveat-status.json`
- **Kind:** feature

## What changed

`a_read_answers_far_more_than_it_asks` weighs a `QueryRequest` with
`prost::Message::encoded_len()`, runs it against a real node, scrapes
`slate_response_bytes_total{method=".../Query"}`, and compares. It is the first
thing anywhere that reads a request weight and a response weight together.

`prost` joins `slate-serverd`'s `[dev-dependencies]` for `encoded_len()`. It is
already in the graph through `slate-server`, so nothing new compiles.

## Why

This closes the first caveat of
`ledger/2026-09-29-a-slow-reader-makes-a-response-later-not-larger.md`, which
was blunt about it:

> **Nothing compares a request with its response.** Both are measured now, in
> the same unit, in different processes, and no test or harness puts the two
> numbers side by side. That comparison is the reason this was worth doing and
> it has not been done.

Two counters in two processes that nobody subtracts are two facts, not a
finding. The whole argument for weighing the response was that it made a
comparison possible; leaving the comparison undone would have made that
argument decorative.

## Alternatives rejected

**Compare inside a client suite instead.** The natural home — Python's
`Counting` already weighs requests — and it cannot reach the other number: the
Python harness runs `slate-testserver`, which serves no `/metrics`. Doing it
there means giving the testserver a scrape endpoint, which is a second server
to keep in step, the reason
`ledger/2026-09-18-a-request-id-a-caller-can-grep-for.md` gives for not giving
it a request log either.

**Assert a literal, like the 1382/1300 the batching measurement pins.** That
worked there because the delta was a formula (`4n + 2`) and the totals were
narration. Here both numbers move with the fixture's column widths and with
`rows_per_message`, and neither is a formula. A literal would pin `docs` rather
than the asymmetry and would go red on a column added to the fixture.

**Weigh the request with an interceptor, matching the clients.** Symmetric and
wrong for this: the clients weigh at their channel because that is where their
message is, and here the message is in hand before it is sent. An interceptor
would add a layer to observe something `encoded_len()` already answers exactly.

**Include the identity metadata in the request weight.** It would make the
ratio closer to what a link sees, and it would compare a request *with* headers
against a response *without* them, because `slate_response_bytes_total`
excludes trailers by construction. Two different units, and the difference
would read as a finding. Recorded as a limit instead — see below.

## Evidence

Observed, deterministic across three runs with no spread:

| | bytes |
| --- | --- |
| `QueryRequest` for a whole table, `encoded_len()` | **8** |
| `slate_response_bytes_total` for that Query, 60 rows | **1134** |
| ratio | **141×** |

Protobuf lengths have no run-to-run variation, which is why three identical
runs are enough and why this is a measurement rather than a benchmark.

**The 8 is the interesting number, not the 1134.** A `QueryRequest` naming a
table and nothing else is eight bytes because `Identity::on` puts the principal
and tenant in gRPC *metadata*: the body carries the table name and empty
fields. So the ratio is payload against payload and overstates what a link
would see — the request's real cost is mostly headers this counter cannot see,
and the test says so where a reader meets it.

1134 bytes over 60 rows is about 19 bytes a row for a `(u64, short string)`,
which is the right order for the encoding and is not asserted.

`cargo test -p slate-serverd --test observing --no-fail-fast`: **15 passed**,
up from 14.

Two mutations, record
`ledger/mutations/20260929T200135-crates-slate-serverd-src-observe-rs.json`:

| mutation | outcome |
| --- | --- |
| the response weight counts frames rather than lengths | caught, this test and `a_scrape_weighs_what_the_node_answered` |
| the exported response weight is always zero | caught, both |

Both are aimed at the *counter* rather than at the test, which is the right
target: this test's job is to notice when the number it reads stops meaning
what it says. A mutation of the assertion itself would only show that the
assertion is load-bearing, which the threshold makes obvious.

## What this does not do

**The ratio is one shape at one size.** A whole-table read of 60 narrow rows.
A point read answers less than it asks; an aggregate answers a handful of bytes
whatever it scans; a write inverts the direction entirely. None of those is
measured, and "a read answers far more than it asks" is true of *this* read.

**It compares payloads, not calls.** Both sides exclude HTTP/2 framing and
headers, and on the request side that is most of the bytes. The number a
capacity plan wants is larger on the request and larger again on the response,
and nothing here measures either.

**It is one direction of one client.** The Rust test builds the request the
harness builds. The three SDKs each weigh their own requests and none of them
can see the server counter, so the comparison exists in the daemon's suite and
nowhere a client author would look.

**The threshold is ten against an observed 141.** Deliberately slack, so a
fixture change does not turn it red for the wrong reason — which also means it
would not notice the ratio falling to eleven. It answers "does the response
dwarf the request" and not "by how much".
