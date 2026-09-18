# The README, the comparison and the site say what decimal arithmetic does, and the site's roadmap stops being two items behind

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `README.md`, `docs/orm-comparison.md`, `site/docs/features.html`, `site/docs/roadmap.html`
- **Kind:** docs

## What changed

The two README open items W4 closed — "`price * quantity` is not expressible"
and "`WHERE total > 19.99` parses as a float" — are gone, replaced by the one
they did *not* close: nothing checks a client's declared scale against the
server's. The README's decimal section gained the expressible/refused table and
the SQL example; `docs/orm-comparison.md` gained a W4 section beside W1 and W3;
`site/docs/features.html` gained the same rule in prose with a link to the
workbench, where a reader can type it.

`site/docs/roadmap.html` had no W3 entry at all and no W4. Both are there now.

## Why

Three of the four documents described a feature the code no longer had, in the
direction that matters: they said something was *missing* that had shipped. A
reader deciding whether this layer can hold money would have concluded it
cannot do arithmetic on it.

The site roadmap missing W3 entirely is the worse one, because it is the
outward-facing list and it had been wrong since W3 shipped — not stale by a
turn of phrase but by a whole item. It was found by going to add W4 beside it.

## Alternatives rejected

**Leave the README items and mark them done.** The list is of things that are
*not* built; a `[x]` line there is for something notable that was, and both of
these are ordinary closed work now described in three other places. Keeping
them would have made the open list a changelog.

**Write the site entries by summarising the ledger.** Tempting, and it is how
a roadmap page usually gets written. The site entries are written from the
same three findings the ledger entries record rather than from the entries
themselves, because a summary of a summary drops exactly the concrete thing —
the 819 cents, the plan that explains and cannot run — that makes the claim
worth reading.

**Add a decimal to the quickstart.** Rejected: the quickstart's job is to get
a row in and out in twelve lines, its code is executed by
`site/check/quickstarts.py` against three real clients, and a decimal column
would mean each of the three snippets growing a scale to restate. The feature
page and the workbench are where a decimal belongs.

## Evidence

`python3 site/check/docs.py`: every element closes the one it opened and every
relative link resolves, including the new `../workbench.html`.

The claims in the new prose are the ones the two preceding commits' tests
assert: `the_float_shortcut_loses_a_cent` for 819 cents,
`explaining_an_inexpressible_decimal_is_refused_too` for the plan that could
not run, `money_times_a_decimal_literal_is_refused_not_nulled` for the column
of nulls, `a_decimal_compares_at_the_boundary_not_near_it` for `19.830`.
Nothing here states a number that is not asserted somewhere.

## What this does not do

- **No check ties the docs to the code.** `site/check/docs.py` checks the HTML
  holds together and that links resolve; nothing notices when a feature page
  describes a refusal the kernel no longer makes. The quickstarts *are*
  executed, which is why they are the one part of the site that cannot go
  stale, and extending that to prose would mean a different kind of check
  entirely.
- **The README's closed-items list was not audited.** Two entries were
  retired because this work closed them. Whether anything else in that list is
  also stale is a question this did not ask.
- **`docs/correctness.md` and `docs/performance.md` are untouched.** Neither
  mentions decimals, and W4 produced no measurement worth putting in
  `performance.md` — the plan-time check runs once per query over an expression
  tree of single digits, which is not a number anyone needs.
