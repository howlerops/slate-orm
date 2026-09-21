# The two lookups a view's absence depends on are rostered, and the rosters have tests of their own.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #264 (F5j)
- **Touches:** `scripts/{check_handlers.py,test_check_handlers.py}`
- **Kind:** guard

## What changed

`scripts/check_handlers.py` grows rules 7 and 8.

Rule 7: every line building a `"no table named` refusal must be in a function
on `NAMES_A_MISSING_TABLE`. Three are — `Head::no_such_table`, which consults
the view registry, and the two converters in `convert.rs`, which rule 3 already
holds to authorising first.

Rule 8: every lookup written `.find(|x| x.name() == ..)` or `.any(|x| x.name()
== ..)` must be in a function on `FINDS_BY_NAME`. Three are — `resolve_relation`
finding a foreign key, and `slate-serverd`'s `views` and `lowered`, which
resolve a declared view's base table at load.

Both get the roster check, the never-fires guard and the staleness check the
five rules before them have, and — unlike rule 6 on the day it shipped — seven
cases of their own in `scripts/test_check_handlers.py`, which is now 35.

## Why

Two ledger entries this session made claims that nothing checked, and both are
claims about *absence*, which is the kind that decays silently.

The first is in `2026-09-21-a-view-reported-as-a-typo.md`: teaching one function
about views fixed the misleading `NOT_FOUND` on six handlers at once, because
`Head::table` is the only place a name-not-found `Status` is built for a
handler. That was true when written and stays true only while nobody adds a
fourth. A fourth would not be noticed — it is one `Status::not_found` in a
function that looks like every other converter — and its symptom is a demo
operator being told they mistyped a name they wrote themselves.

The second is in step 1's entry: a view kept out of the `Catalog` is refused by
every path that exists, *without a line written*, because `Catalog::table_by_name`
is the only way a name becomes a `TableDef`. `slate-serverd` already contains two
lookups that are not it. They are load-time and caller-less, so they are fine —
but "fine, and here is why" is a roster entry, and "fine" alone is how the count
goes from two to four without anyone deciding.

Neither rule stops anything by itself. What they do is make the *next* one a
diff somebody has to justify, which is the `EXPECTED_REFUSALS` idiom this
repository uses in four other places.

## Alternatives rejected

**Match on `Status::not_found` rather than on the wording.** It would survive a
reworded refusal, which is the one thing rule 7's never-fires guard exists to
catch. It would also fire on every not-found in the tree — a missing row, a
missing index, a missing tenant — and a roster of thirty entries is a roster
nobody reads. The wording is the narrow thing, and the guard says out loud that
moving it disarms the rule.

**Match rule 8 on the collection rather than the closure.** `catalog.tables()`,
a `&[TableDef]` threaded through three functions, a local `Vec` — the collection
is whatever the caller had, and a pattern keyed on it would miss the next one
for a reason that has nothing to do with the hazard. The closure is what
"resolving by name" actually looks like.

**Roster `Catalog::table_by_name` too, for symmetry.** It lives in
`slate-schema`, outside `SOURCES`, and it is the lookup the other two are
measured against. Putting it on the list would make the list say "here are the
three ways a name becomes a table", when the point is "here is the one, and
here are the two that had to argue their way in".

**Leave rule 7 out and rely on rule 3.** Rule 3 does hold both converters to
authorising first, which is why a view's name cannot reach either. But that is
a property of *today's* callers, established by a different rule, and the
refusal is reachable from anywhere the day someone adds a helper. The roster
records the argument next to the thing it is about.

**Skip the dedicated cases, since the fixture exercises both rules.** This is
exactly what rule 6 did, and its own entry records what that was worth:
breaking the roster check, the never-fires guard and the staleness check all
*survived*, because the shared fixture had been extended to satisfy the new
rule and every existing case passed through it. Extending a fixture to make a
suite green is not testing the thing the fixture now contains. Seven cases here,
written before the mutation run rather than after it.

## Evidence

`python3 scripts/check_handlers.py` against the real tree: **3 missing-table
refusals all rostered, 3 lookups by name all rostered**, alongside the six
counts it already reported.

`scripts/test_check_handlers.py`: **35 passed, 0 failed**, up from 28. Seven new
cases — for each rule an unrostered function reported *by name*, a never-fires
fixture, and a stale-entry fixture; plus one for the `any` arm of rule 8's
pattern, which the `find` cases would not have covered.

Nine mutations via `scripts/mutate.py --dialect python`, **all caught**:

| mutation | caught by |
| --- | --- |
| rule 7's roster check skipped | `a function building a missing-table refusal itself is reported` |
| rule 7's pattern reworded | the four exit-0 fixtures, via staleness |
| rule 7's never-fires guard removed | `a tree that builds no missing-table refusal fails` |
| rule 8's roster check skipped | `a lookup resolving by name() is reported` |
| rule 8's never-fires guard removed | `a tree that resolves nothing by name() fails` |
| `any` dropped from rule 8's pattern | `an any that resolves by name() is reported too` |
| rule 7's message de-f-stringed | `a function building a missing-table refusal itself is reported` |
| rule 8's message de-f-stringed | `a lookup resolving by name() is reported` |
| both rosters dropped from the staleness loop | both stale-entry cases |

Two of those deserve their caveat. The **pattern** mutations (rows 2 and 6) are
caught, but by the staleness check rather than by a case written for the
pattern: with the wording moved, the functions that were matching stop being
`seen` and their roster entries read as stale. That is a real failure with a
named test, and it is a less direct diagnosis than the others — a reader would
be told "delete this entry" when the truth is "your regex moved". It is also
why row 6 needed its own case at all: `mutate.py` prints only the first four
failing names, so the `any` case's failure was invisible in the summary and had
to be confirmed by running the mutation by hand.

The two **message** mutations are there because rule 2 shipped with the literal
`{owner}` in its output for as long as it existed — an f-string prefix missing
from a continuation line — and rule 6 arrived with the same bug on its first
run. Asserting the function's name in the expected output is what makes that
bug fail a test rather than cost a reader a grep.

**The `check_handlers.py` rosters corrected their own author, again.** I wrote
`join_from_proto` for the refusal at `convert.rs:2667`; it is
`aggregate_from_proto_query`, and the first run of rule 7 said so. That is the
second time this file has caught a claim I made from reading, and it is the
argument for a roster of names over a paragraph asserting the same thing.

`sh scripts/check.sh`: **34 passed, all of them.**

## What this does not do

**Neither rule sees a value that was resolved elsewhere.** A function handed a
`&TableDef` that some rostered lookup produced is invisible to rule 8, exactly
as rule 1 is blind to a `&TableDef` passed along from an authorised site. Both
make *adding* a resolution a failure rather than a silence; neither traces one.

**Rule 8's pattern is a spelling, and its never-fires guard is the honest
admission of that.** `filter(..).next()`, a named closure argument, a
`position()` — none match. The guard fires when the count reaches zero, which
catches a wholesale rewrite; it cannot catch a fourth lookup written in a shape
the pattern never saw while three others keep the count above zero. That is the
same limitation rules 3, 5 and 6 carry and it has no cheap fix.

**The fixture's ordering is now load-bearing and nothing checks it.** Three
cases are built by cutting a slice out of `PREAMBLE`, so each trimmed function
must sit inside the slice named for the rule it disarms — which is why `lowered`
moved above the view functions it lives beside in the real tree. A comment says
so. A reordering that broke it would show up as a case failing for the wrong
reason, which is a cost paid by whoever next edits that string.

**The never-fires fixtures report more than they test.** Trimming the view
functions also strips `no_such_table`'s refusal, so rule 7's roster goes stale
in the same run; rewording the refusal strips both converters the same way. The
assertions name the never-fires message, which no other check produces, so the
cases still fail for their own reason — but the output a reader sees carries
two or three findings, not one.

**Nothing checks the rosters' *reasons*.** An entry whose justification has
quietly stopped being true — a converter that gains a context, a load-time
lookup that starts running per request — keeps passing, because the check is on
the name. That is a judgement a person makes on the diff, and the reason text
is written to make it possible rather than to make it automatic.
