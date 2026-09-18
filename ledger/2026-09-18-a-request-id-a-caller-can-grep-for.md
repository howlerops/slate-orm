# A request id, so a caller holding a failure can find the server's line

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5), working the second six of `docs/orm-comparison.md`
- **Touches:** `crates/slate-server/src/auth.rs`,
  `crates/slate-serverd/src/observe.rs`, all three clients,
  `site/docs/deployment.html`, `docs/orm-comparison.md`, `README.md`
- **Kind:** feature

## What changed

A `slate-request-id` metadata key, defined once in `slate-server` beside the
identity keys it is deliberately not one of. Every call from every client
carries a fresh one — a UUID's randomness in hex — and every error each client
raises carries the same value. The daemon's request log prints it, filtered, as
a field of its own: `id=3f2504e0… status=0 head=1.4ms`, or `id=-` when a call
sent none.

## Why

The README has had this as an open item since the clients were written, and
recorded it as *blocked*: `slate-serverd` logged its startup and its warnings
and nothing per request, so an id sent from a client would have had nothing to
be correlated against. The per-request log built earlier today removed the
blocker and left the item, which made this the last open row in the
comparison's "smaller, and owed" list.

Without it, a caller with a failed call and an operator with a log of ten
thousand lines have no way to find each other.

## Alternatives rejected

**A field in the proto.** The obvious place, and the wrong one: it belongs to
the *call*, not to the query, and there are nineteen request messages. Adding
one field to each to say one thing is a schema change, a regeneration in three
languages and nineteen places to forget it. A header is what gRPC has for
per-call metadata, and this repository already has three `slate-` ones.

**Letting the server generate one when a caller sends none**, and returning it.
Then every line is correlatable, which is strictly better for an operator. It
also means response metadata, three clients reading trailers, and a decision
about a value the caller did not choose. `id=-` and a client that always sends
one gets the same coverage for none of that. If a caller who does not use these
clients ever needs it, this is where it goes.

**A session-wide or connection-wide id.** Cheaper: one `uuid4` per session
rather than per call. It also names every line the session wrote, which is
what a caller already has — the point is to name *one* line.

**`math/rand` in Go, or a counter in any of them.** A counter collides across
processes in exactly the log somebody is reading to tell two processes apart,
and Go's global `math/rand` source is seeded per process, so two started in the
same instant generate the same sequence. `crypto/rand` is not for secrecy here
— the value is a label the server trusts nothing about — it is for not
colliding.

**Escaping the id rather than filtering it.** An escape is a second encoding
for a reader to get wrong, and there is no value in round-tripping a label
nobody but its sender chose. Dropped characters are dropped.

**Refusing an over-long id rather than cutting it.** Refusing loses the
correlation entirely over a formatting opinion. Cut at 64, a caller with a
verbose scheme still gets a prefix that matches.

## Evidence

**A measurement that corrected a guess.** The filter's first comment claimed
`HeaderValue` lets a tab through and refuses a newline, which was half right.
Probed directly, `http::HeaderValue::from_bytes` **refuses** CR, LF, FF, VT,
NUL and DEL, and **accepts** tab, space, `"`, `'`, `=` and any high byte. So a
caller cannot forge a log *line* — the transport stops that — but can forge
*fields*: an id of `x status=0 head=0.0ms` gives a reader, and anything
splitting on whitespace, two `status=` to choose between. That is the attack
the allowlist actually stops, and it is the one a filter written against
newlines alone would have missed. The comment and the test now say what was
measured.

**Ten mutations, eight killed outright, two survived and were then killed.**

| mutation | outcome |
| --- | --- |
| the server's filter removed entirely | KILLED (3 tests) |
| the 64-character cap removed | KILLED `a_long_id_is_cut_rather_than_refused` |
| the header never read | KILLED (4 tests) |
| an empty id becomes `Some("")` rather than `None` | KILLED |
| the id not printed on the line | KILLED (3 tests) |
| Go: a call site sends the id and reports the outer context | KILLED `TestEveryCallKindNamesItsRequestID` |
| Python: the header is not appended to the metadata | KILLED (2 tests) |
| Python: the id is not put on the error | KILLED |
| **Go: the id is generated and never sent** | **SURVIVED** → killed |
| **TypeScript: the id is generated and never sent** | **SURVIVED** → killed |

The last two are the finding. Every client test read the id off an *error*, and
the id on an error is generated client-side — so deleting the line that puts it
in the outgoing metadata left both suites entirely green. A client that mints an
id, reports it on every failure and never sends it would look perfect from the
inside and be useless, because the server's log would say `id=-` for every call.
Python caught it only because its test happened to inspect the metadata
directly. `TestTheHeaderIsActuallySent` and its TypeScript twin now read the
metadata a call would carry, and both mutations fail them.

The Go one is worth its own note: the id travels in the `context`, so a call
site that passes `s.ctx(ctx)` inline to the RPC and the *outer* `ctx` to
`fromRPC` sends an id and reports none. Nothing in the compiler catches that —
both are valid contexts. Three transaction call sites were exactly that shape
after the mechanical conversion. `TestEveryCallKindNamesItsRequestID` has a row
per RPC family for this reason, and the mutation above confirms it fires.

**End to end**, through a real socket:
`observing.rs::the_log_carries_the_id_the_caller_sent` sends
`slate-request-id: checkout-7f3a` and asserts the daemon's stderr contains
`/slate.v1.Records/Query id=checkout-7f3a status=0`. That is the one test that
proves the whole chain rather than either half.

**Three cross-language guards.** Each client reads
`crates/slate-server/src/auth.rs` and compares the constant against its own
spelling. A mismatch is otherwise *silent*: the server ignores metadata it does
not recognise, so a misspelled key means every call carries an id nobody ever
sees and every line says `id=-`, and no test on either side alone can see it.
All three assert rather than skip when the file is missing — and the TypeScript
one earned that immediately, failing on a path counted from the source tree
when the suite runs compiled out of `dist-test/`.

Green: `cargo test -p slate-serverd` 210, `-p slate-server` 132, Python 245,
Go all, TypeScript 135. `cargo fmt --all --check`, `clippy --workspace
--all-targets` under `-D warnings`, `gofmt`, `go vet`, `tsc --noEmit`, `mypy`
and `ruff` all clean.

Disk hit ENOSPC twice during this (the linker error `CLAUDE.md` describes); the
ledger's dedup snippet plus dropping `target/debug/{incremental,examples}`
freed enough both times.

## What this does not do

- **The server never generates one.** A caller that sends nothing gets `id=-`,
  and only these three clients send one. See the rejected alternative.
- **Nothing is returned to the caller.** The id a client reports is the one it
  sent, not one the server echoed, so a failure raised before the call left the
  process still carries an id — which is deliberate and documented, because an
  id with no matching log line is the useful signal that the call did not
  arrive.
- **No propagation across a call chain.** Each client mints its own per call.
  A service calling a service gets two unrelated ids, and there is no
  `traceparent` handling or anything else that would tie them together. That is
  distributed tracing and it is a different item.
- **It is not authenticated and must not be trusted.** Two callers may send the
  same value, and any caller may send any value. It is a label for finding a
  line, never an input to a decision — which is why the constant is documented
  as not being one of the identity keys it sits beside.
- **The Python suite does not prove the server logs what Python sent**, because
  it runs against `slate-testserver`, which has no request log. That half is
  the Rust test above, and the Python test says so where somebody will read it.
