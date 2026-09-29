# The one aggregate that returns money was compared by nobody

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `examples/explorer/backends/{go,node,python}`,
  `examples/explorer/conformance/conformance.py`,
  `examples/explorer/CONTRACT.md`, `examples/explorer/web/src/api.ts`
- **Kind:** feature

## What changed

`/api/aggregate` returns a `total` beside every group's `count`: `SUM(books.price)`,
a decimal, on all twelve groupings and in all three adapters. `sort` takes
`"total"` as well as `"count"` and `"key"`, and two new conformance cases group
authors both ways. 137 cases to 139.

Measured on the demo's data, exact counts of cents: Le Guin 4144, Banks 3249,
Lem 3025, Butler 2625. The two sort orders genuinely differ — Le Guin and Lem
have three books each and hers are worth more — which is what the new
`MUST_DIFFER` pair compares.

## Why

Two entries, written four days apart, said the same thing:

> **`price` is not in any aggregate case.** SUM over a decimal is exact and
> that is the claim worth comparing across three clients; it is the obvious
> next case and is not there.
> — `ledger/2026-09-18-the-three-sdks-compared-on-a-decimal.md`

> **No `sum(price)` anywhere in the corpus.** `/api/aggregate` returns counts
> only, so the one aggregate that returns a decimal is compared by no client
> case.
> — `ledger/2026-09-18-three-clients-and-the-integer-they-would-all-have-reached-for.md`

Every aggregate this corpus compared returned a `u64`. A count is the one
aggregate where three clients cannot disagree about the *type* of the answer,
so twelve groupings compared twelve ways of naming a group key and one way of
counting rows. A sum over a decimal is the opposite case: the kernel adds
counts of the column's smallest unit and never a float, so the claim is that
the answer is exact — and a client that reached for a float anywhere on that
path is wrong by a cent, which is the kind of wrong that ships.

## Alternatives rejected

**A `metric` flag, so the caller picks `count` or `sumPrice`.** It would keep
the answer shape as it was and cost one new case. Rejected because the point is
to compare the sum on *every* grouping: the groups differ per grouping and so
do the sums, so a field on the existing answer buys twelve comparisons where a
flag buys one — and the aggregate list is shared with
`/api/explain-aggregate`, so the plan's `decodes` grows too and is compared as
well.

**Leave it as a field and skip the `sort`.** The shorter change, and the one
that would have been quietly worthless. `total` inside the answer is invisible
to "three clients agree" in the way that matters most: all three dropping the
second aggregate produces groups with a count, no `total`, and perfect
agreement. Sorting by it makes it load-bearing — `Agg(1)` with no second
aggregate is refused by the *server*, and the runner reports a case that is
supposed to answer and did not. This is the `EXPECTED_REFUSALS` mechanism
doing work it was not written for.

**Sort by the total in `having` too.** `having` reads `Agg(0)`, the count, in
all three adapters and stays that way. A HAVING over money is a different case
rather than the same one with a field added, and the existing HAVING cases are
about the *group condition* rather than about which aggregate it reads.

**A second decimal column at a different scale, so the sum crosses scales.**
That is the sibling caveat — "the corpus compares renderers at one scale" —
and it needs a schema change to the demo's `books`, which every generated
declaration in three languages is pinned to. Left where it is, open.

## Evidence

- `examples/explorer/run.sh --conformance`: **139 cases, the three SDKs agree
  on all of them. 139 passed, 0 failed.** Up from 137.
- The sums, read off a running stack:

  | author | books | total |
  | --- | --- | --- |
  | Ursula K. Le Guin | 3 | 4144 |
  | Iain M. Banks | 2 | 3249 |
  | Stanisław Lem | 3 | 3025 |
  | Octavia E. Butler | 2 | 2625 |

  Sorted by count descending the order is Lem, Le Guin, Banks, Butler; sorted
  by total it is Le Guin, Banks, Lem, Butler. Different, which is the pair.
- The plan changed as predicted. `/api/explain-aggregate` on `author` now
  reports `decodes=[1,7]` for `books` — ordinal 7 is `price` — and its
  `display` reads `Group by [authors.name] computing [count(*),
  sum(books.price)]`. Read off the running stack, not off the planner's source.
- **Three mutations, all caught.** Two against the Go adapter
  (`ledger/mutations/20260929T023019-examples-explorer-backends-go-handlers-go.json`):
  asking for no decimal sum → *four cases disagree*, including both new ones
  and the two decade cases; and sorting by `Agg(0)` when asked for the total →
  *authors by what their books are worth: the adapters disagree*, with the pair
  no longer comparable. One against the corpus
  (`ledger/mutations/20260929T023058-examples-explorer-conformance-conformance-py.json`):
  the money case asking for the count → *returned the same 'groups'*, which is
  the pair catching exactly what it is for.
- `examples/explorer/run.sh --e2e`: 28 passed, 0 failed.
- `examples/explorer/web && npm test`: 16 passed, 0 failed.
- `python3 scripts/caveats.py`: 1632 caveats, 111 open, 72 narrowed, 404
  closed, 912 deliberate, 0 untriaged.
- `sh scripts/check.sh`: 72 passed, all of them.

## What this does not do

**One scale, still.** Every `total` here is at scale 2, because that is what
`books.price` declares. A client whose decimal handling breaks at scale 0, or
on a negative, is invisible to this — the sibling caveat that says so is
untouched and stays open.

**It compares the sum, not the rounding.** Nothing in this corpus divides a
decimal, so the question of what `AVG` over money should return is not asked
here. `SUM` is exact by construction and that is the whole of the claim.

**The demo's UI does not show it.** `GroupRow.total` is in the frontend's types
and the chart still draws the count. A second series would be a chart change
arguing for itself, and the endpoint's own caveat about what the UI shows is a
separate one.

**`having` still reads the count.** A group condition over money would exercise
a different path — the server compares a decimal against a bound rather than a
`u64` — and it is not here. The three HAVING cases are unchanged.

**Two of the twelve groupings are refusals or near it.** `badPrice` is refused
at plan time and carries no groups at all, so the sum is compared on eleven
groupings rather than twelve. Worth knowing when reading "every grouping".
