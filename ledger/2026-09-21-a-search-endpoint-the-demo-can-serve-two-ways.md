# A search endpoint in the demo, and what three SDKs comparing to each other cannot see

- **Date:** 2026-09-21
- **Author:** Claude Opus 5, working F6b
- **Touches:** `examples/explorer/head.toml`, `CONTRACT.md`, the Go, Node and
  Python adapters, `conformance/conformance.py`
- **Kind:** feature

## What changed

`POST /api/search`: full-text over `books.title`, served by index or by table
scan on the caller's choice, answering with the rows and the plan's access
path. The demo's catalog gains its first index — `by_title_text`, inverted —
and the three-SDK conformance corpus gains eight cases and two `MUST_DIFFER`
pairs. 118 cases became 126.

## Why

F6a put `contains` on the wire and in all three clients, each tested against a
node it started itself. The demo — the one place where the three SDKs answer
the *same* server and are compared to each other — could not search at all.

The endpoint offers both access paths on purpose. A text index is not chosen
by cost at eleven books (`docs/full-text.md` measures the crossover near one
row in 24,000), so an endpoint that did not hint could only ever demonstrate a
table scan. Offering both makes the hint the thing under test, and makes the
two answers comparable: same rows, different plan.

## Alternatives rejected

**A new table with prose in it, rather than an index on `books`.** The index
adds no column, so `books`'s fingerprint is unchanged and none of the three
adapters' declarations moved; a new table would have cost three more
declarations and a seed. The seeded titles are already a usable corpus — "the"
in six, "storage" in none, "games" in one — which is more than a synthetic
table would have offered.

**No hint, and let the planner choose.** It chooses the scan, so the endpoint
would demonstrate nothing about the index and the `MUST_DIFFER` pairs would
have nothing to compare.

**Refuse the search when the caller cannot `EXPLAIN`.** This was the first
implementation and the conformance runner rejected it: `reader` has no
`explain` grant, so `a reader's search` came back `PERMISSION_DENIED` from all
three adapters. Granting `reader` explain was the wrong fix — EXPLAIN is
privileged deliberately, because a plan is costed against statistics covering
rows the caller's policy hides, and widening it would undo that. So `access`
is `null` for such a caller and the rows are served. All three adapters
swallow `PERMISSION_DENIED` from the explain and nothing else.

## Evidence

Conformance: 126 cases, the three SDKs agree on all of them.

Two mutations of the Go adapter, both caught:

- ignoring `path` and always scanning → four cases disagree;
- truncating the search text at three characters → four cases disagree.

**And one that survived, which is the finding.** Removing `text = true` from
`head.toml` — so `by_title_text` is an *ordinary* index on `title` — left
every case green and both `MUST_DIFFER` pairs satisfied. Measured against the
running demo rather than reasoned about, because reading `match_index`
predicted the opposite:

| catalog | `path: index` | `path: scan` |
|---|---|---|
| `text = true` | `Index Scan using by_title_text`, 6 rows | `Table Scan`, 6 rows |
| `text = false` | `Index Scan using by_title_text`, 6 rows | `Table Scan`, 6 rows |

An ordinary index accepts the same hint, and the plan summary carries the
index's name but not its key range — so "six entries under one term" and
"eleven entries, then filter" render identically. No cross-SDK comparison
could catch it either, because all three adapters would be equally wrong. It
is written down in all three places a reader might otherwise assume otherwise:
the catalog, the contract, and beside the cases.

That measurement also corrected this work's own contract text, which said
`"the"` finds five titles. It finds six.

## What this does not do

- **The demo cannot demonstrate the inverted shape**, per the table above. That
  claim lives in `slate-serverd`'s `schema.rs` tests (`key_sets` returns one
  list per term) and the kernel's `fulltext.rs` (the planner's range), which is
  where a claim about storage belongs.
- **No `CONTAINS` in the SQL front end and no search box in the web UI.** Still
  open, still F6b.
- **No case covers a search that matches nothing *through the index*** beyond
  the prefix case, and none covers a multi-term search where the terms are in
  different rows — `"solaris cyberiad"` would be a sharper conjunction test
  than `"the games"` and is not there.
- **`access` is compared only for equality between two cases.** Nothing asserts
  its content, so a server that renamed every plan would keep the corpus green.
