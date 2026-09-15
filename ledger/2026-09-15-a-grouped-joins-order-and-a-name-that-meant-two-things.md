# A grouped join can be ordered, a join can be offset, and an unqualified name that both tables have now says which one it meant

- **Date:** 2026-09-15
- **Author:** Claude, working from "I don't want any gaps. Please address those"
- **Touches:** `crates/slate-wasm/src/sql.rs`, `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/datetime.rs`, `site/workbench.js`, `site/style.css`, `site/check/workbench.py`, `site/docs.html`
- **Kind:** feature

## What changed

Three things in the SQL front end, and a fourth that was withdrawn.

**`ORDER BY` on a grouped join.** It was refused with "ORDER BY is not
supported on a join yet" — true of both shapes and explanatory of neither.
`JoinSpec` gains `sort`, which lowers onto `Grouping::sort`: a group is
`[key, aggregates...]`, so `ORDER BY count(*) DESC` is a sort key on the
group's second slot. An *ungrouped* join still cannot be ordered, because
`Join` has no sort field — the kernel orders groups and not joined rows — and
the refusal now says that rather than "not yet".

**`OFFSET` on a join.** Not a parse error before; there was simply no field. It
was already expressible in the kernel (`Join::offset`, `Grouping::offset`) and
worked on a single table, so the difference was in the *front end* and read as
a difference in the engine.

**An ambiguous unqualified name warns.** On a join a bare name resolves against
the left table first, so `SELECT id ... FROM trips JOIN zones` answers about
`trips.id`. That rule was documented in the parser's source and nowhere the
reader could see it — which is the same as not having one, since the reader is
the person who typed the ambiguous name. `parse` now returns
`Parsed { statement, warnings }`, the binding carries the warnings onto its
result, and the workbench renders them above the table as a caution. Said once
per name, not once per mention.

## Why

All three were recorded gaps rather than discoveries, and the ambiguity one is
the one that matters. `trips` and `zones` both have `id`. `SELECT id, count(*)
FROM trips JOIN zones ... GROUP BY id` returns 260-odd groups of trip ids where
the reader almost certainly meant a handful of zones, and every number in it is
correct. Refusing is not the fix — refusing every ambiguous name would refuse
`SELECT hour(pickup_time)` on any schema where both tables happen to have a
`pickup_time`, which is the reason the left-first rule exists. Saying which one
was chosen costs nothing and is what a reader needs.

## Alternatives rejected

**Refuse the ambiguous name, as Postgres does.** It is the stricter and more
defensible rule in a full SQL engine, and wrong here: this parser has no
aliases and no `FROM a, b`, so a reader cannot always qualify their way out,
and the schema decides whether a name is ambiguous rather than the query.
Postgres can refuse because it gives you every tool to disambiguate.

**Put the warning in the status bar.** One line of code, and the status bar
sums a whole buffer — three statements, one of them ambiguous, and the line is
about all of them. The caution belongs above the answer it is about, which is
also where a reader is looking.

**Fold warnings into the existing `message` field.** `message` is what the
write path reports and the plan pane prints. A caller wanting to render a
caution differently from a status line would have to parse one string to tell
them apart; a list costs a field.

**Order an ungrouped join by cloning the joined rows and sorting them in the
binding.** It would work, for small results, and it would be a second
execution path — the thing this front end exists not to be. The kernel's
position is that a window or an ordering over an unordered join is not a
meaningful request until someone asks for it with a reason; the refusal now
says so.

**Give `Parsed` a `warnings` field on each `Statement` variant** rather than
beside it. A warning is about the *text*; the statement is what the text meant.
Five variants would each carry a field only one code path reads.

## Evidence

**Four new tests in `tests/datetime.rs`,** against the 100,000-trip sample:
ordering by `count(*)` in both directions (ascending is asserted to be the
descending list reversed, which rules out an ordering that drops the
direction), ordering by the key, ordering by a computed key, the three refusals
(no `GROUP BY`, an aggregate the query does not compute, a column that is not
the key), the window on both a grouped and an ungrouped join, and the warning
— including that it is absent for an unambiguous name and absent again once
the name is qualified.

**Mutation testing, four mutations:**

| mutation | caught by |
| --- | --- |
| `grouping.sort` never assigned | `a_grouped_join_can_be_ordered_by_its_aggregate`, `ordering_a_grouped_join_names_the_key_or_an_aggregate` |
| the sort direction ignored (always ascending) | the same two |
| the ambiguity warning never pushed | `an_ambiguous_name_on_a_join_warns_and_still_answers` |
| `join.limit`/`offset` set even when grouped | **survived** — see below |

**Three new browser checks**, so the caution is verified where a reader would
see it rather than only in the JSON: a grouped join ordered by an aggregate
comes back descending in three rows, the ambiguous name renders exactly one
`.caution` naming `trips.id`, and qualifying it renders none.

**A withdrawn hypothesis.** `join.limit` was set whether or not the read was
grouped, which looked like the mistake the single-table path is careful to
avoid — a window over the rows going *into* a grouping answers a different
question. I wrote that up as a bug, guarded it, and then mutation-tested the
guard by putting the bug back. Nothing failed. `narrowed_join` in the kernel
already clears a grouped join's `limit` and `offset`, and explains at length
why: honouring them made the answer depend on which join algorithm won, which
was a real defect once, found by two hash build sides disagreeing. The guard
stays for legibility and changes no answer; the claim that it was a live bug is
withdrawn, in the code comment and in the test's own docstring.

**A latent wrong width, labelled rather than claimed.** The outer-join padding
in `joined_spec` used the *left* table's width for both sides, so an unmatched
right side would have produced the wrong number of cells whenever the tables
differed. Nothing reaches it — `JoinSpec` carries no join type, so every join
this binding runs is inner — so it is corrected with a comment saying it is
latent, not presented as a fixed bug.

Suites: `slate-wasm` all green (38 tests in `datetime.rs`), `cargo clippy -p
slate-wasm --all-targets` clean, `python3 site/check/workbench.py` all checks
pass against a fresh wasm build, `python3 site/check/quickstarts.py` passes.

## What this does not do

**Chains are still not in the SQL front end.** `FROM a JOIN b JOIN c` is a
parse error: `JoinSpec` is `left`/`right`/`left_key`/`right_key` by
construction, and generalising it to a list of inputs cascades into the panel's
dropdowns, the spec pane, the explanation rendering and the browser check. The
kernel, the wire and all three clients do chains; this front end does not, and
that is its own piece of work rather than a line in this one.

**`ORDER BY` on an ungrouped join is a refusal, not a feature.** It needs a
sort on `Join`, which the kernel does not have and which nobody has asked for.
The refusal explains itself, which is the whole of what changed there.

**One group key per join, still.** `GROUP BY a, b` works on a single table and
not on a join, because `JoinSpec::group_by` is one `Option<u32>`. The
`ORDER BY` resolver handles that shape and would need the same widening.

**The warning is the only warning.** Nothing else in this parser reports a
"true but worth saying" fact, and there are candidates — a `LIMIT` with no
`ORDER BY` on a grouped read, a filter that cannot become a scan bound. The
mechanism is now there; the list is one item long.

**No test asserts what the caution *looks* like.** The browser check counts it
and reads its text; nothing checks that it is visually distinguishable from a
refusal, which is the reason it has its own class.
