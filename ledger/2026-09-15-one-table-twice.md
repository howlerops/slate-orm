# Table aliases, so a chain can read `zones` twice — and the browser check stops running last hour's kernel

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/rust-orm-record-layer-gswxlu`
- **Touches:** `crates/slate-wasm/src/sql.rs`, `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/{sql,taxi}.rs`, `site/workbench.js`, `site/check/workbench.py`
- **Kind:** feature

## What changed

`JOIN zones AS pickup ... JOIN zones AS dropoff` parses, plans and answers.
The parser carries `Vec<Input>` — a `TableDef` plus the name this query calls it
by — where it carried `Vec<TableDef>`, so two inputs may hold the same table.
`JoinSpec` gained `left_alias`/`right_alias` and `ChainInputSpec` gained
`alias`, each empty when the query used the table's own name, so a spec without
aliases serialises exactly as it did before. The workbench has the example this
schema was always for, and the browser check drives it.

Two things fell out on the way, one a bug and one a guard:

- The `WHERE` split re-resolved each column through `condition`, which matches a
  qualifier against the *table's* name. `WHERE pickup.borough = 'Manhattan'`
  came back as "`pickup.borough` is not a column of `zones`".
- `site/check/workbench.py` ran against whatever `site/slate_wasm_bg.wasm` was
  on disk, with no check that it was built from the current sources. It now
  refuses a bundle older than its sources, the way `slate-serverd` has since
  task #140.

## Why

The chain work (the previous entry) left the front end able to express three
tables and unable to express a useful one. `trips` reaches `zones` through both
`pickup_zone` and `dropoff_zone`, and "the borough it started in and the borough
it ended in" is the three-table question this dataset exists for. Without an
alias the second `zones` was a refusal — correctly, since every column reference
resolves against a name and two inputs called `zones` make `borough`, and
`zones.borough`, ambiguous with no way to say which end was meant.

So the only chain the schema could express was `authors JOIN books JOIN zones`
on `books.id = zones.id`: well-formed, and a meaningless question. That is why
the previous entry has no workbench example and no browser check. This one has
both.

The kernel needed nothing. Whether it would read one table twice in a `Chain`
was the load-bearing unknown, so it was asked before anything was planned
around the answer: a three-input chain of `zones` on `zones.id`, and a two-table
self-join, both through the binding's own entry points. Both returned correct
rows and a sane plan. Aliases are a front-end and spec change only.

## Alternatives rejected

**Aliases on single tables too.** `SELECT b.title FROM books b` is a thing
people write, and it is refused here with a message saying a single table has
nothing to be told apart from. Rejected because an alias on one table buys only
a second spelling of one name, and supporting it means threading that name
through every single-table helper — `conditions`, `condition`, `column`,
`value_ordinal`, `group_ordinal`, `having_condition`, `aggregate` — so that
`b.title` resolves. That is a wide change for no disambiguation. *Ignoring* the
alias would be worse than either: `b.title` would then fail with "not a column
of `books`", which is true and explains nothing.

**A global alias→table map on the parser**, consulted by `resolve`, rather than
per-input names. One field, one lookup, and it handles the single-table case for
free. Rejected because it cannot handle the case aliases exist for: `pickup` and
`dropoff` both map to `zones`, so `resolve_side` would have two candidate inputs
and no way to choose — which is the exact ambiguity being removed.

**Only `AS x`, not a bare `x`.** Fewer ways to write the same thing and no
keyword guard needed. Rejected because the bare form is what people write, and
the guard is a nine-word list. But see the typo below: the bare form on the
first table needed one more condition than the list can express.

**Renaming the new example to dodge the selector collision.** The browser
check's `has-text("Manhattan")` broke the moment a second example title
contained that word. Rejected in favour of tightening the *selector* to
`has-text("hour by hour")`: a substring match over a list that grows is a
collision waiting to happen, and moving the title would leave the next one to
find it again.

**A content hash rather than `mtime` for the staleness guard.** Exact, and
immune to a clock. Rejected because the wasm bundle is not reproducible byte for
byte across toolchains, so a hash needs a manifest `build-wasm.sh` does not
write. The failure being guarded against is minutes or hours old; `mtime` is
coarse and sufficient.

## Evidence

Nine new tests. Five are in `crates/slate-wasm/tests/taxi.rs` rather than in
`sql.rs`, deliberately: that file loads all 100,000 real trips, so the checks
are over real pickup and dropoff zones rather than the meaningless books chain.

- `one_table_twice_names_its_halves_apart` — 11 + 4 + 4 columns, headed
  `pickup.*` and `dropoff.*`, and each half's `id` equals the trip column it was
  joined on. That last assertion is the one that matters: a chain that joined
  `pickup` to both steps would pass every check about the header.
- `grouping_by_one_end_agrees_with_the_two_table_join` — first asserts the
  chain returns as many rows as `trips` has, so the second step dropped nothing
  (every `dropoff_zone` is a real zone id), and *then* compares counts per
  pickup borough against the plain two-table join. An independent query as the
  oracle, and the precondition checked rather than assumed.
- `the_two_ends_are_not_the_same_end` — Manhattan pickups grouped by dropoff
  borough. If both aliases resolved to one input every group would be
  Manhattan; the test requires more than one borough and that Manhattan is not
  all of it.
- `a_bare_alias_works_and_reads_the_same_rows` — `zones p` and `zones AS p`
  return identical rows and differ only in the header.
- `an_alias_is_refused_where_it_cannot_mean_anything` — five refusals, including
  the typo below.

Every suite in the crate is green with `--no-fail-fast`, and
`cargo clippy --workspace --all-targets` is clean.

### The two failures worth writing down

**`WHERE pickup.borough` was not a column of `zones`.** The `WHERE` branch
rewound a token and called `condition`, which resolved the name again and
replaced the ordinal already computed. Harmless while every input was a bare
table; wrong the moment one had an alias, because `condition` matches a
qualifier against the table's own name. Found by
`the_two_ends_are_not_the_same_end` on its first run. The fix removes the second
resolution rather than teaching it about aliases — the ordinal was already in
hand, and only the operator and the literal were still to read.

**`FROM books WERE id = 1` became an alias.** `FOLLOWS_A_TABLE` keeps `JOIN` and
`WHERE` from being read as bare aliases, but a *misspelled* keyword is by
definition not in the list, so the typo was read as an alias for `books` and
reported as "`WERE` aliases a single table" — burying the actual mistake under a
rule the reader has never heard of. The old message was "unexpected `WERE`",
which is the right one, and no list of keywords gets it back. So a bare alias on
the table after `FROM` is taken only when a `JOIN` follows it. `FROM books AS b`
still reaches the single-table refusal, because `AS` says plainly what was
meant. Caught by the existing refusal table, which is the argument for having
one.

### The stale bundle

The browser check's three new assertions failed against a wasm bundle built
ninety minutes earlier, before any of the day's work. The failure read as
"aliases do not work"; the truth was "you are running last hour's kernel". The
bundle is gitignored, the browser loads whatever is on disk, and the only
enforcement was a line in a docstring asking for `sh site/build-wasm.sh`.

The dangerous direction is the other one. A check that *passes* against a stale
bundle is a green run that exercised nothing it claims to — the same shape as
the workflow that had never fired, the Python suite that skipped itself, and the
deployed run that compiled against another example's build output. So
`stale_sources()` now compares the bundle's `mtime` against every `.rs` and
`Cargo.toml` under the four crates it is built from, and a stale bundle is a
hard error naming the files.

After rebuilding, all three alias checks pass in the browser.

### Mutation testing

Eight mutations, each applied alone, against `--test sql --test taxi`:

| mutation | verdict |
| --- | --- |
| a bare alias may swallow a keyword (`FOLLOWS_A_TABLE` ignored) | caught, 17 tests |
| a bare first alias needs no `JOIN` after it | caught, 2 tests |
| a qualifier is matched against the table, not the alias | caught |
| two inputs collide on their table, not their name | caught, 2 tests |
| a spec always carries an alias, never empty | **survived** — see below |
| headers ignore the alias and use the table's name | caught |
| an ambiguous grouped key is never qualified | caught |
| the single-table alias refusal never fires | caught |

**The survivor was the backward-compatibility claim**, which is written into
`ChainInputSpec::alias`'s own doc comment two paragraphs above and was tested
nowhere: a query that names no alias serialises exactly as it did before aliases
existed. Making `alias_of` always return the name changed no answer, no header
and no refusal — every spec simply grew a field. Invisible to every other test
here, and visible in the workbench's Spec tab, in the JSON a reader is being
told is what the SDKs send.

`a_query_with_no_alias_carries_no_alias_in_its_spec` closes it, in both
directions: a plain join and a plain chain carry no `alias`, and an aliased one
carries the name the reader gave it — so the test cannot pass by never writing
the field at all. Re-applying the mutation now fails that test by name.

That is twice in a row a mutation pass has found a missing test rather than
confirming the ones that exist, which is the argument for running it even when
the tests already look thorough.

## What this does not do

**The panel still has no alias control.** It is a two-dropdown join form; an
alias can only be written in SQL. The spec pane shows the `alias` field, and
`chain()` accepts one, so nothing is unreachable — but the form does not offer
it.

**A single-table alias is a refusal**, as argued above. `SELECT b.title FROM
books b` is not accepted, and the message says why and where aliases do work.

**Nothing outside the wasm crate and the site.** The three SDKs build joins and
chains from typed builders where an input is a `TableDef` the caller already
holds, so there is no name to disambiguate and no alias to add. If a client ever
grows a SQL surface this becomes their problem too.

**`GROUP BY` on a join or chain still takes one key**, unchanged from the
previous entry and still recorded as its own piece of work.
