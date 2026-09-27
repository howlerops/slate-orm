# The claims guard read one page; the fixture's size is stated on seven. Widening it produced two false positives, and one of them was the guard demanding that a correct number be wrong.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `scripts/check_site_claims.py`, `scripts/test_check_site_claims.py`
- **Kind:** process

## What changed

`check_site_claims.py` now reads every page under `site/`, `README.md` and
`docs/*.md`, not only `site/index.html`. Three checks are new:

- **every stated trip count is the fixture's**, wherever it is stated;
- **every stated zone count is the CSV's**;
- **every keyword the parser refuses by name is named in the docs**, with the
  keywords read out of `sql.rs`'s module documentation rather than from a list
  here.

Two floors go with them: the trip pattern must match on more than one page and
the zone pattern on at least one, because a narrowed pattern that matches
nothing satisfies "every match agrees" perfectly.

## Why

The guard's own entry from an hour ago recorded "**It reads one page**" as its
first caveat. The fixture's size turned out to be stated **seven** times —
`site/index.html`, `site/workbench.html` three times, `site/docs/index.html`,
`site/docs/storage.html` and `README.md` — and the pages nobody checks are
where a stale number lives longest.

The refusals check is a different shape and the more interesting one. It
completes a chain: the parser refuses `UNION` → `sql::grammar::the_refused
_keywords_are_all_named` checks the module documentation names it → this checks
the site names it. Three links, each decidable, and no hand-written list in the
middle. Adding a sixth refusal to the parser now fails in the crate until the
module docs say so, and then fails here until the site does.

## What widening it found

**Two false positives, and the second is the one worth writing down.**

1. `site/index.html: 4,837 trips`. The hero code block prints `132  4,837`
   above a line reading `Table Scan on trips`, and collapsing whitespace — which
   the pattern must do, because the landing page wraps its claim across a line
   — put them side by side. **Sample output is not a claim**; `prose()` strips
   `<pre>`, `<code>` and fenced blocks before anything reads a page.

2. `docs/performance.md: 2,964,619 trips`. Not a stale number: the whole of
   January 2024, loaded deliberately, which is the point of that measurement.
   **The guard was demanding that a correct number be wrong.** A check whose
   failure mode is "make the true thing false" is worse than no check, and the
   only reason it did not ship is that it fired on the first run.

The fix is that the pattern names the *fixture* rather than the noun: a match
needs "New York" beside it, or the `N-trip sample` phrasing `storage.html`
uses. That excludes the month-scale figure by construction rather than by a
special case, and `a different trip count that is not the fixture's is left
alone` is the case that keeps it excluded.

## Alternatives rejected

**Exempt `docs/performance.md` by path.** It would work today and would be a
roster of files where the guard is switched off — the thing this repository
spends entries removing. Naming what the pattern means is smaller and does not
rot.

**Check every number on every page.** The generalisation is tempting and
undecidable: the pages carry version numbers, port numbers, byte counts,
percentages and years, and a guard that tried to source them all would be a
guard with an exemption list longer than the pages.

**Check that the two pages' *prose* agrees, which is the other half of
`2026-09-14-the-home-page-is-a-workbench.md`'s caveat.** Comparing prose to
prose is weaker than comparing both to the repository, and that is what this
does: two pages can agree and both be wrong. The prose-to-prose half stays
open, and the entry says so.

## Evidence

**Mutations**, six run, six caught, no survivors:

| mutation | caught by |
|---|---|
| the widened trip check compares nothing | `a second page with a stale trip count is reported` |
| the widened zone check compares nothing | `a second page with a stale zone count is reported` |
| a pattern matching nothing counts as a pass | `a pattern that has stopped matching is reported, not passed` |
| code blocks are not stripped | `a trip count in a code block is not a claim` |
| the refusals check never reports | two cases, including the one that adds a keyword to the parser's own sentence |
| `claim_pages` returns only the landing page | four cases |

The last is worth naming: reverting the widening breaks four cases, which is
the check that this change is load-bearing rather than decorative.

**Suites.** The guard's own: **18 passed, 0 failed** (11 before), every case
over a tree it writes. Against the repository: **18 checks, all holding**, over
**27 pages**, with the fixture's size found in 7 statements and the zone count
in 2. `sh scripts/check.sh`: 55 passed, all of them.

**The runs themselves**, as `mutate.py` recorded them:

- `ledger/mutations/20260925T234738-scripts-check-site-claims-py.json` — scripts/check_site_claims.py

## What this does not do

**It still checks numbers and names, not sentences.** The prose half of
"nothing verifies the two pages agree" is untouched and unverifiable by this
means.

**`docs/*.md` is read for claims and not for its own numbers.** The design
notes are full of measurements, and those have their own guards
(`check_table_provenance.py`, `check_cost_prose.py`). What this adds for them
is only the fixture's size and the refused keywords.

**The refusals chain has a weak link this does not fix.** It reads a *sentence*
in `sql.rs` by the phrase "are refused by", and a rewording that keeps the
meaning breaks the read — which fails loudly rather than silently, because
`the parser's module docs name some refusals` asserts the list is non-empty.
That is the best available: the parser refuses those keywords in three separate
places, and a regex over Rust control flow is a parser for Rust.

**Nothing checks the *examples* on the pages run.** `site/check/quickstarts.py`
runs the quickstart code and nothing runs the fragments in `features.html` or
`limits.html`, several of which are now SQL this front end accepts and could
be executed.
