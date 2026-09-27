# Nothing in CI opened a documentation page, and `nav.js` builds the navigation on all ten of them

- **Date:** 2026-09-27
- **Author:** Claude Code, closing an open caveat from
  `ledger/2026-09-16-what-the-other-orms-have-that-this-does-not.md`
- **Touches:** `site/check/pages.py`, `.github/workflows/ci.yml`,
  `scripts/test_check_sh.py`, `scripts/check.sh`, `CLAUDE.md`
- **Kind:** a check, and one stale number

## What changed

`site/check/pages.py` opens all ten documentation pages in Chromium and checks,
per page: no page error and no console error; the sidebar holds one link per
page `nav.js` itself lists; exactly the current page is marked
`aria-current="page"`; the contents holds one entry per `h2` the article
carries, or the whole column is gone when there is only one; the pager holds
two links, or one at each end of the reading order; the document has a title.

A CI job of its own, `pages`, which needs node, Playwright and the checkout —
no wasm, no cargo, no server. `scripts/test_check_sh.py` records it as one
`check.sh` cannot run, which takes `ci.yml` to 111 steps.

And `CLAUDE.md` said **seventeen jobs**. There are twenty-one, and were twenty
before this. Corrected, with the one-line command that answers it, because the
sentence has no check and drifted silently.

## Why

`site/check/docs.py` reads the pages as text and is good at it — every element
closes the one it opened, every relative link resolves, every `<noscript>`
sidebar matches the one `nav.js` renders. What it cannot see is anything that
happens after the file is served, and all of the navigation happens then. The
caveat put it exactly:

> `nav.js` builds the navigation for all ten pages and a JavaScript error would
> empty the sidebar site-wide with every static check still green.

`site/check/workbench.py` already does this for the one page that *runs*. The
nine that only describe had nothing, and they are cheaper to check: no wasm, no
trip file, no kernel.

## Alternatives rejected

**Add a step to the `playground` job**, which already installs Playwright. One
job instead of two and no second `npm ci`. Rejected because `playground` first
installs a wasm toolchain, runs `cargo install wasm-bindgen-cli` and builds the
wasm bundle — so any of those failing means this step never runs, which is the
"a check that never fires is a check nobody has debugged" shape `CLAUDE.md`
names and which this repository has already been bitten by. The docs pages do
not depend on the kernel and their check should not either.

**Assert the sidebar has ten links.** The number is right today and would be
wrong the first time somebody adds a page — and then somebody updates the
number rather than asking why it moved. Every count here is derived: the
sidebar against `nav.js`'s own `SECTIONS`, the contents against the article's
own `h2`s, the pager against position in the reading order. The only hard-coded
thing is the *shape* of a `SECTIONS` entry, and a `SECTIONS` rewritten into
another shape makes `pages()` find zero pages and fail loudly rather than
silently checking nothing.

**Check the rendered prose.** Tempting while a browser is open, and it would
duplicate `docs.py` and `check_claims.py` — two checks failing on one defect,
and a reader working out which is the real one.

**A screenshot comparison.** Catches far more, including every restyle, and is
the check people switch off. `check_site_css.py` makes the same argument for
substring matching over anything exact.

## Evidence

- `python3 site/check/pages.py` — `ok    10 documentation pages render, each
  with a full sidebar, a contents derived from its own headings, and a pager`.
- Five mutations of `site/docs/nav.js` via `scripts/mutate.py`, recorded as
  `ledger/mutations/20260927T024129-site-docs-nav-js.json`; every one was
  caught, and each by the assertion meant for it:
  - a `ReferenceError` before any rendering → *page error*, plus an empty
    sidebar, no current page and an empty contents;
  - `SECTIONS.slice(1)` → *the sidebar has 7 links and nav.js lists 10 pages*;
  - never setting `aria-current` → *the sidebar marks [] as the current page*;
  - `headings.slice(1)` → *the contents has 2 entries and the article has 3
    h2*;
  - dropping the next link → *the pager has 1 links, expected 2*.
- `python3 scripts/test_check_sh.py` — 4 of 4, 111 steps accounted for.
- `sh scripts/check.sh` — 71 of 71.

## What this does not do

**It runs in Chromium and nowhere else.** `workbench.py` has the same limit and
records it; a module that parses in Chromium and not in Safari — which
`workbench.js` already has a note about, for browsers before 16.4 — would pass
here. A second browser is another Playwright download per CI run for a page
that has no JavaScript in it but the navigation.

**It asserts that the navigation rendered, not that it is right.** The sidebar
having ten links does not mean they point anywhere useful; `docs.py` resolves
them against the filesystem, which is a different claim from the browser being
able to follow one. Nothing here clicks a link.

**The job count in `CLAUDE.md` is still a hand-maintained number.** It is
correct now and has exactly the property that made it wrong: adding a job does
not touch it. A guard is possible — count `jobs:` keys, compare against the
prose — and it would be the third guard in this repository holding a sentence
to a file. It is not here, and the command that answers it is written beside
the number instead, which is weaker and cheaper.
