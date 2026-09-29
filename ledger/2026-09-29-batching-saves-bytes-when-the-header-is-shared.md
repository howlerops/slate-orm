# Batching saves bytes when the header is shared, and costs them when it is not

## What changed

Two tests in `clients/python/tests/test_round_trips.py`:

- `test_a_batchs_framing_cost_is_four_bytes_a_statement_and_two_for_the_envelope`
  — the `4n + 2` taken apart message by message, with no server.
- `test_loading_a_relation_for_three_parents_saves_bytes_as_well_as_calls`
  — one `Related` for three parents against three for one each.

## Why

Two caveats from the last two entries. *"It does not explain the `4n + 2` from
the wire format"* — the arithmetic fitted five points exactly and the
explanation was read off the protobuf encoding rules, so a different framing
producing the same slope would have been indistinguishable. And *"Nothing
weighs a relation load, a join or an aggregate"*, in two entries at once.

**The decode.** Building the messages and subtracting:

| message | bytes |
| --- | --- |
| `InsertRequest` | 25 |
| `BatchOperation(insert=…)` | 27 |
| `BatchRequest`, no operations | 2 |
| `BatchRequest`, one operation | 31 |
| `BatchRequest`, two operations | 60 |

Two bytes for the `oneof` that wraps the insert in a `BatchOperation`, two for
the `repeated` field that holds it in the `BatchRequest`. That is the four,
and it is two things rather than one. The "envelope" turns out not to be
framing at all: `pb.BatchRequest()` with nothing set weighs **zero**, and the
two bytes are the `atomicity` enum — a field the caller sets and a single
`Insert` has no equivalent of. The earlier entry called it an envelope, which
was the right number for the wrong reason.

**The relation, and the finding.** One `Related` for three parents is **53
bytes**. Three `Related` calls, one parent each, are **147**. Batching saves
64% of the bytes here — against costing 6% on the write path, from the same
word.

The two together are the shape neither number has alone:

> **Batching saves bytes when the requests share a header, and costs bytes
> when they do not.**

A write batch shares nothing: each statement carries its own table, its own
rows and its own schema claim, so folding them together adds framing and
saves only the round trips. A relation load shares everything — one table, one
relationship name, one schema claim — and varies only the parent keys, so
folding them saves two whole copies of the header.

That is a rule a caller can use, and it is not what "a batch is a round trip"
or "cheaper on the wire" says.

## Alternatives rejected

**Assert the relation's 53 and 147.** The sibling tests already learned this
one: the totals move with the fixture and the *relation* between them is the
finding. `together * 2 < apart` is a factor rather than a literal, and it is
loose enough to survive a header changing size and tight enough to refuse the
equal case that sending them apart produces.

**Weigh the join and the aggregate too, as the caveat names.** Attempted and
abandoned, which is why both caveats are narrowed rather than closed. The
join's fair comparison is one `Join` against two `Query` calls the caller
folds itself, and the aggregate's is one grouped request against a full scan —
and on the *request* side the second of those is uninformative, because both
requests are small and the whole saving is in the response this instrument
does not see. The join is worth doing and the aggregate probably is not; the
residual says so rather than implying the work is uniform.

**Explain the four bytes in prose and leave the tests as they were.** What the
previous entry did, and the caveat it wrote against itself. Prose that reads
the encoding rules correctly and prose that reads them plausibly look the
same on the page. Subtracting two messages does not.

## Evidence

`cd clients/python && python3 -m pytest -q` — **351 passed**, up from 349.
The whole suite: `_count` and the fixture are shared.

The tables above are the output of the committed tests. The relation numbers
were printed once from the committed test itself, not from a scratch copy —
the last entry's literals came from a scratch run at a different key range and
were wrong.

Seven mutation cases over two runs:
`ledger/mutations/20260929T071316-clients-python-tests-test-round-trips-py.json`
and
`ledger/mutations/20260929T071409-clients-python-tests-test-round-trips-py.json`.

| mutation | outcome |
| --- | --- |
| the oneof and the repeated field conflated into one total | caught |
| the slope not checked against a second statement | caught |
| the relation load sent apart rather than together | caught |
| the envelope read as framing (`== 2` weakened to `>= 0`) | **survived — test strengthened** |
| the empty `BatchRequest` weighs two rather than zero | caught, after |
| the saving asserted as merely non-worse | **survived — expected** |

The first survivor was worth acting on. Asserting only that the envelope
weighs two leaves "two bytes of framing" and "two bytes of atomicity"
indistinguishable, and `>= 0` passes either way. The test now also asserts
that a `BatchRequest` with nothing set weighs **zero**, which is the
falsifiable form of the claim — and the mutation of *that* is caught.

The second is the loosening class recorded with `expect_survivor` for the
second time today: a threshold weakened cannot fail against a value that
already satisfies the tighter one. The factor's job is to refuse the equal
case, and that shape was mutated directly and caught.

## What this does not do

**No join, no aggregate.** Named above with the reason. Both caveats are
narrowed, not closed.

**The relation comparison is against the same RPC, not against hand-rolled
queries.** Three `Related` calls for one parent each is the honest
no-batching alternative and shares the fixture's declaration, which is what
makes the subtraction clean. A caller who wrote three filtered `Query` calls
instead would send something different again, and that is unmeasured.

**Three parents, one shape.** The saving is roughly the header times `n - 1`,
so it grows with the fan-out, and nothing here measures the slope the way the
write path's five sizes did.
