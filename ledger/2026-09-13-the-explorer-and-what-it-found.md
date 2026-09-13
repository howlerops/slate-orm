# The explorer's backend: three SDKs, one contract, and the first test that
# compares them

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `examples/explorer/` (new), `.gitignore`
- **Kind:** feature

## What changed

Three HTTP adapters over one head node — Go, Node and Python, one per client —
implementing a single contract, plus a conformance runner that sends 31 requests
to all three and requires byte-identical JSON. The frontend is not in this
commit.

## Why

The demo asked for a backend where each SDK serves the same API and the UI can
swap between them. That framing turns out to be worth more than a demo: three
ledger entries this week end with some version of "nothing here compares the
clients against each other; each is tested against the same server, which
catches one being wrong and not two being wrong the same way".

A shared contract is exactly that comparison, and it is cheap once the adapters
exist.

## What the conformance runner found on its first run

Every one of these was invisible to all three clients' own test suites.

1. **The three adapters sorted joined rows by three different keys.** Go used
   `fmt.Sprintf("%v")`, which renders a map differently from `json.Marshal`, so
   the outer joins came back in three different orders for the same rows. Mine,
   not the clients'.
2. **`Query.sort` in the Python client replaces the ordering rather than
   appending.** A loop of `query.sort(key)` calls keeps only the last one, and
   the answer arrives in primary-key order — indistinguishable, from the
   outside, from the server ignoring the sort entirely. The client is behaving
   as documented ("Replaces any earlier sort"); the adapter was wrong, and the
   failure mode is quiet enough to be worth recording.
3. **An unknown table produced two different errors.** Go has no catalog so it
   passed the name to the server and got `not-found`; the other two hold a
   schema, cannot build a request without one, and refused locally. Now all
   three refuse locally with the same words, because two of them have no choice.
4. Four `JoinedRow`/`Table`/`insert`/`delete` API misuses in the Python adapter,
   each of which returned a plausible-looking error rather than a wrong answer.

## Alternatives rejected

**One process routing to three SDKs.** Not possible across three languages, and
pretending otherwise would mean a gateway that is itself a fourth
implementation.

**Letting each adapter shape its own JSON.** Then the frontend needs three
decoders and the conformance runner has nothing to compare. The contract fixes
the encoding to the point of pinning float formatting to six decimal places,
because Go, Python and JavaScript disagree on the last digit of a
shortest-round-trip float and the runner compares text.

**JSON numbers for 64-bit integers.** `JSON.parse` rounds above 2^53. Every
integer is a tagged string.

**Untagged values.** `1` as an `i64` and `1` as a `u64` are different values to
this database. Untagged, the three adapters could disagree about which they sent
and the runner would not see it.

**Hand-bucketing "group by decade" in the adapters.** The obvious way to make
the chart's third grouping work, and it would be the adapter doing the
database's job — three times, identically, or the runner fails. Refused with a
message saying the client cannot declare a computed column yet, which is true.

**A framework in each adapter.** Six endpoints and no middleware. `net/http`,
`node:http` and `http.server` cost nothing to read and nothing to install.

## Evidence

31 conformance cases pass across all three adapters. The cases were chosen to be
ones the clients could plausibly disagree about — every value type, filter
lowering including negation and `IN`, all four join types, group ordering with a
deliberate tie, the plan's text, and five refusals. A case whose answer is the
same no matter what the client does proves nothing and is not in the set.

Checked by hand against a live stack: `app` sees 11 books, `reader` sees 9 (a
row policy hides two published before 1960), `stranger` is refused,
`reader`'s `EXPLAIN` is refused, and the transaction demo reports
`visibleInside: true` with `visibleAfter` following the commit flag.

## What this does not do

No frontend yet — this is the backend and the contract.

The runner is not wired into CI and needs a running stack, so nothing runs it
automatically. It is a script someone has to invoke.

`groupBy: "decade"` is a documented refusal rather than a feature, and the
computed-column support that would close it exists in the kernel and not in
these clients.

The adapters are not a benchmark and the README says so: they add a JSON round
trip the SDKs do not have, over an in-memory head node holding eleven books.
