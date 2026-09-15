# The docs caught up with four features, and a list of open problems turned out to be carrying five closed ones

- **Date:** 2026-09-15
- **Author:** Claude Code, working from a request to show the whole surface in
  the docs, the site and the examples
- **Touches:** `README.md`, `site/docs.html`, `site/workbench.js`,
  `docs/correctness.md`, `docs/performance.md`, and the two tests whose numbers
  those documents now quote
- **Kind:** docs

## What changed

Relationships, keyset pagination, conditional writes, exact decimals and the
migration runner all landed without any of them reaching a document a reader
sees. They are now in the README (five sections and eleven status entries), on
the docs page (five cards), and — for the one of them that is reachable from
the browser — in the workbench's example list. Two design notes gained the
findings that belong in them: `docs/performance.md` the two counted
measurements, `docs/correctness.md` the three defects and the five things they
left unproven.

The part that was not planned: `docs/correctness.md`'s "What is still not
proven" list was carrying five items that had been done or withdrawn — partial
indexes in the derive macro, grouped joins on the wire, grouping over a chain, a
grouped join's cost, and the head node's performance. Its own opening sentence
claimed the opposite ("everything that was on this list a round ago has moved
above it"). They are gone, and the section now says which five and why that
matters.

## Why

A feature nobody can find is not shipped, and the four newest ones were
reachable only by reading the crate. That was the request.

The stale list is the more interesting half, because it is the exact failure
this repository warns about in `CLAUDE.md` — *stale documentation is worse than
none, because it is read as current* — appearing in the document that says so
most often. A list of open problems is read as a to-do list. Five of eleven
entries being closed does not make it 45% wrong; it makes the whole list
untrustworthy, because a reader cannot tell which half they are looking at.

It also compounds: three of the five were closed by work that has its own ledger
entry saying so. The information was in the repository. Nothing carried it the
last hundred metres.

## Alternatives rejected

**Deleting the five stale entries quietly.** Cheapest, and it is what the
section would look like if this had never happened. Rejected because the way the
list rotted is worth more than the list: someone adding an item next month needs
to know that entries here have gone stale before, or they will assume the
absence of an item means the problem does not exist. The replacement text names
all five.

**Leaving `docs/correctness.md` and `docs/performance.md` alone and putting
everything in the README.** The README is the front door and the two design
notes are where a reader goes for the worked examples, so this would have put
the measurements in the least detailed place. It also would have left the stale
list untouched, since the only reason it was read at all was going there to add
something.

**Quoting the numbers the tests assert, rather than adding prints.** The
pagination test asserts `cursor_pairs * 10 < offset_pairs` and the relations
test asserts `cost <= 1` — deliberately loose bounds, for reasons each test's
comments give. Quoting a bound as though it were a measurement would be the
"never report a number you did not observe" rule broken in the most plausible
way. Both tests now print, and the documents quote the printed line. The
decimal test prints too, with a comment saying the digits are shown rather than
asserted because they depend on summation order.

**A browser-check case for the new workbench example.** Rejected against the
check's own stated reasoning: `site/check/workbench.py` clicks the kitchen sink
only, because "every example is executed by `crates/slate-wasm/tests/examples.rs`;
this is the one that has to survive the *click*". The new example is neither
multi-statement nor semicolon-bearing, so it adds nothing to that case.

**Adding the four features to the three client SDKs, so the docs could show
them everywhere.** That is not a documentation change. Three of the four want
the same protocol change — a cursor field, an expected-row field, a decimal
`Value` case — and doing it once is the right shape. The README says so under
"Not built" rather than implying the clients have them.

## Evidence

Every number now in the documents was printed by a run, not recalled:

```
PAGE 99 OF 100: offset read 495 pairs, cursor read 5
3 parents: batched 1 scan(s), per-parent loop 3
SUM decimal Decimal(1000) vs SUM f64 9.99999999999998
```

The five stale entries were checked against the code rather than against the
task list: `crates/slate-orm/tests/partial_indexes.rs` exists and its
`the_derived_partial_index_behaves_like_a_hand_written_one` passes;
`group_by_join` appears in `slate-server`'s `convert.rs`, `service.rs` and
`session.rs`; `group_by_chain` is in `slate-kernel/src/record.rs` and is
exercised over the wire in `slate-server/tests/multi.rs`; `slate-headbench` is a
crate and `docs/performance.md` has "The head node, measured". The costing item
was withdrawn rather than done, and
`the_per_row_term_is_symmetric_so_grouping_cannot_flip_the_algorithm` exists in
`grouped_chain_oracle.rs` to make the withdrawal fail loudly if it stops
holding.

The new workbench example's comment claims the Spec tab shows aggregates and no
`groupBy`. That was checked before it was written — a throwaway test printed the
spec, which has no `groupBy` key at all because the field is
`skip_serializing_if = "Vec::is_empty"` — and is now asserted in
`a_whole_table_aggregate_needs_no_group_by`. **Mutation:** removing that
`skip_serializing_if` makes `groupBy` serialize as `[]`; that named test fails;
restored and re-verified. The first draft of the comment said "`keys` is empty",
which was wrong in two ways at once, and would have shipped if the claim had not
been checked.

Checks run: `cargo test -p slate-wasm --test examples` (2 passed, and
`every_example_the_page_offers_runs` asserts the last statement of each example
returns rows, so the new one is executed and not merely parsed);
`cargo clippy --workspace --all-targets` clean; `python3 site/check/quickstarts.py`
(the three SDK snippets on the docs page each start a node and round-trip a row);
`python3 site/check/workbench.py` — 72 checks, 0 failures, against a wasm bundle
rebuilt for this change at 789 KiB gzipped.

## What this does not do

The client READMEs are untouched, because none of the four features is reachable
from a client and a README that described them would be the stale-documentation
problem in a fresh file.

`examples/explorer` is untouched for the same reason. Its `/api/aggregate`
requires a `groupBy` name, so even the whole-table aggregate — the one thing
here the wire already carries — would need a contract change, three adapters and
the frontend to demonstrate, which is a feature and not a doc update.

Nothing verifies that the documents stay true. `site/check/quickstarts.py` runs
the code on the docs page and the examples test runs the workbench's list, but
the prose in `README.md` and the two design notes is checked by nobody, which is
how five entries went stale in the first place. The numbers at least now come
from lines a run prints, so a change that moves them leaves the old figure
visibly disagreeing with a fresh run.
