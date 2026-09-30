# The page called "What it is not" did not mention the decimal scale

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `site/docs/limits.html`
- **Kind:** docs

## What changed

Two paragraphs on `site/docs/limits.html`: a decimal's scale is the client's to
get right and nothing checks it, and every latency this node reports is time to
the response head so a streamed read's rows are in none of them.

## Why

The triage that finished today gave the ledger's 1551 caveats a verdict each
and left 120 open. Sorting them by how badly a reader would want to know, the
top two are these, and neither was on the page whose whole job is to say where
this stops.

~~The scale one is the sharper. `Value::Decimal` carries units and the scale
lives on the column, so a client renders `1999` as `19.99` from the scale it
declared locally. The fingerprint deliberately does not hash a scale — a scale
addresses no column, so hashing it would refuse a request that reaches exactly
the right row — which means a client that believes a column is scale 2 where
the catalog says 4 prints every value a hundred times wrong, for ever, with no
error at any layer. Three separate ledger entries record it independently and
the docs site did not.~~

**Wrong, and withdrawn the same evening. The fingerprint has hashed the scale
since `35d9182` on 2026-09-18** — `crates/slate-serverd/src/fingerprint.rs`
calls `state.number(scale as usize)`, and its module docstring names the scale
as the deliberate exception to the addresses-a-column rule for exactly the
reason the paragraph above says it is not one. The five caveats were true when
written and were answered later the same day, by the entry
`2026-09-18-the-one-thing-in-the-fingerprint-that-addresses-no-column.md`.
The page was rewritten to say so; see
`ledger/2026-09-28-the-scale-hole-was-closed-ten-days-ago.md`, which is the
third time in one session I said this wrongly and the account of why. The
paragraph is struck rather than deleted because the wrong reasoning is the
subject of that entry.

The second paragraph of the page, on latency, was and is correct.

The second is the one `crates/slate-serverd/tests/ceilings.rs` measured this
afternoon: `request_timeout` bounds a request that *waits* and not one the
handler answers without pending, and every latency the node exports is to the
response head. A reader sizing a timeout from the page would have taken the
name at face value.

## Alternatives rejected

**Put them in `docs/` rather than on the site.** They are in `docs/` already,
in the entries that found them — and an entry is a dated record rather than a
statement of where the system stops today. The limits page is the one document
whose job is the second thing, which is why five of the six paragraphs around
these were written the same way.

**Wait until either is fixed.** The scale hole is a protocol decision with a
real cost either way, and the response-head latency needs the streaming path
instrumented. Both are open and both are months from being decided by anybody;
a limit that is real today belongs on the page today. `ledger/README.md`'s
whole argument is that stating a limit is cheaper than discovering it.

**Write the scale one as a warning rather than a limit.** It reads like a bug
and it is not: the protocol publishes no schema, and a client that must
declare a scale is the consequence. Filing it under "what it is not" says that
without implying somebody forgot a check.

## Evidence

`python3 site/check/docs.py`: the ten pages render in a browser and every
relative link resolves.

The measurement behind the second paragraph is in
`ledger/2026-09-28-a-request-timeout-does-not-bound-a-fast-request.md`: ten runs
of a live node, five at `1ms` and five at `0ms`, none of which cancelled a
two-row query — **on this container**. CI cancelled the same query at `0ms` on
the first try, so those ten runs are samples of a race and not a property; the
account is `ledger/2026-09-28-a-measurement-that-reversed-under-ci.md`.

## What this does not do

**It fixes neither.** Both stay open in `docs/caveat-status.json`, which is
where the work is tracked; this is the page a reader meets first saying what
the tracker already knew.

**The other 118 open caveats are not on the page**, and should not all be —
most are about a test that is missing or a number that is unmeasured, which is
this repository's business rather than a reader's. The two here were chosen
because each changes what a caller would write, which is the line the page's
other paragraphs are drawn on and is a judgement rather than a rule.
