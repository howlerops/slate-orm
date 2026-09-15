# Every position in a joined query resolves in the joined row, and a grouped join can finally select its own key

- **Date:** 2026-09-15
- **Author:** Claude (Opus 5), with jacob.beck.018@gmail.com
- **Touches:** `slate-wasm` (`lib.rs`, `sql.rs`, `datetime.rs`, `playground.rs`),
  `site/` (workbench examples, docs page, browser check)
- **Kind:** feature, and one fix for a query that was refused for being what it was

## What changed

`JoinSpec` resolved each kind of position against a *fixed* side: a computed
column and the group key against the left table, an aggregate against the right.
Every one of them now resolves in the joined row, with the side named explicitly
by `ComputeSpec::input` and `AggregateSpec::input`, and a qualified name —
`zones.id` — picking its own side in the SQL front end.

So `SELECT hour(pickup_time), avg(fare) FROM trips JOIN zones ... GROUP BY
hour(pickup_time)` runs. The previous entry recorded exactly that query as not
expressible and gave the reason: "a join's computed column reads the left table
and its aggregates read the right", so a query wanting both from `trips` had
nowhere to land. The workbench example filtered on the right side and counted
instead.

Along the way, a defect with nothing to do with sides: **a grouped join refused
to select its own group key.** `SELECT borough, count(*) ... GROUP BY borough`
came back as "`borough` is not the group key". The check was
`spec.group_by.is_some()` and never compared the two ordinals, so every bare
column was refused whether or not it was the key — the most ordinary shape of a
grouped query, wrong for as long as the join path has existed.

## Why

The limitation was in the browser's own query spec and never in the database.
`Join::compute` is evaluated over the joined row and has always been able to
read either side; a grouping's aggregates are ordinals in the joined space like
any other. The spec narrowed both, and then the SQL front end's error messages
reported the narrowing as a property of the query — "`hour()` on a join reads a
column of `trips` (the left side)" — which reads like a rule rather than like a
gap.

The select-list defect went unnoticed because every test of a grouped join keyed
on a *computed* column, which takes a different branch. A fixture that never
does the ordinary thing does not test the ordinary thing.

## Alternatives rejected

**Keeping the implicit sides and adding a second, explicit form.** Two ways to
say the same thing, one of them a trap. The implicit ones were also *opposite* —
left for compute, right for aggregates — which is what made the missing case
hard to see: each rule looks reasonable alone.

**Defaulting `AggregateSpec::input` to 1**, to keep every existing spec meaning
what it meant. It would have made the JSON contract carry the old asymmetry
forever, in a field whose whole purpose is to remove it. The cost of defaulting
both to 0 is one test's hand-written JSON, which is what caught it.

**Refusing an ambiguous unqualified name.** Stricter, and what some SQL dialects
do. Rejected because `trips` and `zones` both have an `id` and `SELECT
hour(pickup_time)` would still resolve fine — refusing every ambiguous name to
catch the rare confusing one would break the common case. Left wins, qualified
overrides, and the workbench example for `zones.id` shows the difference.

**Resolving a joined name through the kernel's `JoinSchema` rather than by hand.**
The right shape, and `JoinSchema::at` exists for it. It would mean the browser
binding holding a schema object per query where it currently holds two
`TableDef`s; `joined_ordinal` is six lines and says the same thing. Worth
revisiting if a third table ever reaches this path.

## Evidence

**The recorded query runs, and agrees with a fold.**
`the_hour_and_the_average_fare_can_come_from_the_same_table` compares 24 hourly
averages against a fold over the decoded trip file joined to the zone table by
hand — nothing in the oracle goes near the kernel's join, its grouper or its
scalars. The tolerance is `1e-6` on fares that run from about 3 to 250, so a
wrong column does not fit inside it.

**Eight mutations, each caught by a named test:** a right-side aggregate not
shifted, a right-side compute not shifted, `compute_scalar` ignoring its base, a
right-side group key not shifted, a qualified right name resolving left, an
aggregate always claiming the left side, a right-side name claiming input 0, and
the select list no longer checking the group key.

**Two of those survived the first pass**, and both gaps were real: nothing
grouped by a bare *right-table* column (every test used a computed key, which is
shifted by construction), and nothing selected a bare column that was *not* the
key. Writing the first of those is what exposed the select-list defect.

**One test caught the JSON default changing**, which is the outcome that
justifies the default: `{"kind": "min", "column": 3}` used to mean `books.year`
and now means `authors` column 3. The assertion failed with `1929` against
`1968` — Le Guin's birth year against Earthsea's publication — which is as legible
as a failure gets.

**The old refusal became a better one rather than disappearing.**
`hour(borough)` on a join was "reads a column of `trips` (the left side);
`borough` is not one" — a scope error standing in for a type error. It is now
"hour() needs a timestamp, and borough is Str". A name on *neither* table is
still a scope error and now names both tables.

**Suites:** the `slate-wasm` suite (26 in `datetime`, 23 in `playground`, and the
rest), the docs quickstarts, and the browser check at 62 assertions including
two new ones that click the two new examples and check the values are boroughs
and plausible fares rather than zone ids and durations.

## What this does not do

**Chains are untouched.** `JoinSpec` is two tables, and a third input reaches the
kernel only through the clients. A chain's own `compute` has no spelling in this
spec or in the SQL front end.

**`ORDER BY` is still refused on a join**, as it was, so a grouped join cannot be
ordered from SQL — only from the panel's spec. That is unrelated to sides and was
already recorded.

**An aggregate's ambiguous unqualified name resolves left with no warning.** If
both tables have a `total`, `sum(total)` silently means the left one. The
qualified form is available and nothing suggests using it.

**The demo's HTTP contract and its three adapters do not expose any of this.**
They gained a computed `decade` in the previous change; the sides are a browser
concern and the adapters build their joins in typed clients where `At(input,
ordinal)` already says which.
