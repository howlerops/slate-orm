# `OVER` in the workbench's SQL front end, and the ordinal that is not known until the end

- **Date:** 2026-09-21
- **Author:** Claude Code, closing F2a
- **Touches:** `crates/slate-wasm`, `site/workbench.js`, `site/check/workbench.py`, `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

The SQL front end parses `f(…) OVER (PARTITION BY … ORDER BY …)` in a
single-table select list, lowers it onto a new `QuerySpec.window`, and the
binding turns that into the kernel's `window::Window`. A sort key may name a
window; the grid heads the extra column with the call. `OVER` on a join or a
chain, beside a `GROUP BY`, and a call inside the `OVER (…)` are each refused
by name.

This replaces a refusal. The message it replaces said the kernel had the
operator, the gRPC clients could ask for one, and only this editor could not —
which was true when it was written and is the last third of F2a.

## Why

A feature the three SDKs can reach and the demo cannot is a feature nobody
reading the site can try. The workbench is the page where a claim in the docs
becomes something a visitor runs, and "window functions" was the one row in the
comparison table that pointed at a refusal.

The work is small in the parser and not small in the ordinals, which is the
part worth writing down.

## Alternatives rejected

**Resolving a window's ordinal when the select list is walked.** The obvious
shape, and wrong: a window lands at `columns + compute.len() + i`, and
`compute.len()` is not final until the statement ends — `ORDER BY round(year)`
registers a computed column *after* the list has been read. A window resolved
eagerly would keep an ordinal one too small, which does not fail: it reads the
computed column and prints it under the window's header. So a window reference
is parked at `WINDOW_SLOT + i` and rewritten once, at the end, by
`resolve_window_slots`.

The alternative to the sentinel was walking the select list twice — once to
count the computed columns, once to resolve. Rejected because the two walks
would have to agree about which items register a computation, and the
find-or-add that makes `hour(t)` in two clauses one column means the second
walk is not a pure function of the first. One rewrite pass over two fields is
smaller and cannot disagree with itself.

**Extending `SortSpec` with a `kind` so a sort key could say "the `i`th
window".** That is how the *wire* names one, and it is right there, because the
gRPC `ColumnRef` addresses a `Row` split into three lists. It is wrong here:
the kernel's own `Query` appends window values to the same flat ordinal space
as computed ones, and `slate-wasm` talks to the kernel directly. A second
addressing scheme in between would be a translation layer with nothing to
translate.

**Refusing a window beside a `GROUP BY` only in the parser.** The parser is not
the only way to reach a spec — the panel builds one, and so can a JSON literal
— and the kernel's `narrowed` *drops* a window on the way into a grouped read.
That is correct there (a window over the rows a fold discards is a value nobody
sees) and silent here. So `build` refuses it too, which is the second statement
of one rule and is deliberate: the parser's version says which clause to
delete, and the binding's catches a spec that never passed through the parser.

**Allowing a call inside `OVER (…)`.** `PARTITION BY hour(pickup_time)` is
expressible in the spec — a computed column has an ordinal like any other — and
is not parsed here. Refused by name rather than left to fail at the `(`,
because "expected `)`" sends a reader away believing the database cannot
partition on a computed value.

## Evidence

**17 tests in `crates/slate-wasm/tests/windows.rs`**, replacing three that
asserted the refusal. The numbers are checked against `GROUP BY` through the
same playground (`a_partition_aggregate_agrees_with_the_same_group_by`), and
the tie in the fixture — authors 2 and 6 both have a book from 1965 — is what
lets `RANK` and `DENSE_RANK` disagree and what makes the peer half of the
default frame visible. `rank_leaves_the_gap_that_dense_rank_closes` asserts the
years first, so a fixture change that removed the tie fails loudly rather than
making the test vacuous.

**A finding, from the mutations: the binding's own refusal had no test.**
Making `build`'s window-beside-a-grouping check unreachable broke nothing,
because every test reached it through the parser — which refuses first. The
binding's copy exists precisely for the spec that did *not* come from the
parser, and nothing exercised that path.
`the_spec_path_refuses_a_window_beside_a_grouping` goes in through
`Playground::run` with a JSON spec, and catches it.

**A second finding, about the harness rather than the code.** One mutation
reported `the text to replace occurs 0 times`: `cargo fmt` had rewrapped the
`return Err(…)` it anchored on between writing the spec and running it. That is
failure mode 1 in `mutate.py`'s own docstring, and a hand-rolled `sed` would
have run the suite against unmutated code and scored the mutation a survivor —
costing a test that did not need to exist.

**Eight mutations, all caught, each by a named test:**

| where | mutation | caught by |
| --- | --- | --- |
| `sql.rs` | a window's ordinal ignores the computed columns | `a_window_shifts_when_a_later_clause_computes_a_column` |
| `sql.rs` | a sort key keeps its parked reference | `a_sort_can_name_a_window_and_names_the_same_one` |
| `sql.rs` | the same window written twice is registered twice | `a_sort_can_name_a_window_and_names_the_same_one` |
| `sql.rs` | a ranking function is allowed an argument | `a_ranking_function_with_an_argument_is_refused` |
| `lib.rs` | `lag` and `lead` swapped | `lag_and_lead_step_through_the_partition` |
| `lib.rs` | the header is looked up past the columns only | `a_window_shifts_when_a_later_clause_computes_a_column` |
| `lib.rs` | a window beside a grouping is lowered | `the_spec_path_refuses_a_window_beside_a_grouping` |
| `lib.rs` | the header list stops at the computed values | four window tests |
| `lib.rs` | the window's `PARTITION BY` is dropped | three window tests |
| `lib.rs` | the window's own `ORDER BY` is dropped | four window tests, and `every_example_the_page_offers_runs` |

**Suites:** `cargo test -p slate-wasm --no-fail-fast` — 15 binaries, all green.
`sh scripts/check.sh` 34 of 34. `python3 site/check/docs.py` green.
`python3 site/check/workbench.py` green, in Chromium against the built wasm.

**A browser case and an example**, because the wasm boundary is the one place
neither side's tests can see: `site/check/workbench.py` runs a window in
Chromium and asserts the header, the five row numbers, the `window` field in
the spec panel and the grouped refusal; `site/workbench.js` gains a "Where each
trip ranks in its zone" example, which `crates/slate-wasm/tests/examples.rs`
executes. The header assertion compares lowercased, because the grid uppercases
its headers in CSS — the first version compared exactly and failed against a
page that was rendering the right thing.

## What this does not do

**A window over a join or a chain is refused, not missing-and-unsaid.** The
kernel refuses one on a join *input* for its own reason — a join has no order,
so which rows a window saw would be whichever ones the chosen algorithm
yielded — and this front end has no shape for a window over the joined row
either. The refusal gives the kernel's reason; building it would mean deciding
what a window over an unordered join means, which is a design question and not
an implementation gap.

**`PARTITION BY` and the window's `ORDER BY` take a bare column.** Not a
computed value, not an expression, and not a qualified name. The spec
underneath takes ordinals and would carry any of them.

**No explicit frames.** `ROWS BETWEEN 3 PRECEDING` is not offered here because
it is not offered by the kernel; the reason is in `window.rs` and is about
eviction, not about parsing.

**The three SDKs and the SQL front end are still not compared against each
other.** `examples/explorer/conformance` sends one case to all three adapters
and has no window case, because the contract and all three adapters would need
an endpoint first. That is the same gap the client entry recorded and this
commit does not close it.

**Nothing measures what a window costs in the browser.** The plan panel says
the read does not stream, which is a fact about the plan rather than a
measurement, and the example's comment says the same in words. A window over
100,000 trips runs fast enough that nobody noticed; that is an observation, not
a number.
