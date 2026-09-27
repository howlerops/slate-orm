# The guard against a correction reaching one copy could not read the site

## What changed

Three things, and the first is the smallest.

1. `site/docs/roadmap.html`'s P2 callout said many-to-many and nested loading
   "have *not* crossed the wire, which is why they are the first item of the
   next plan below". They crossed in N1. It now says so — a `path` resolved
   level by level in one round trip — with a second paragraph naming what it
   genuinely stops at: no predicate, ordering or limit per level, and a
   fan-out bounded by nothing.

2. The retired wording is registered in `scripts/retired_claims.json`, so the
   sentence cannot come back.

3. **`scripts/check_retired_claims.py` now reads `.html`.** It did not, and
   that is the finding. Its `ROOTS` has included `site` since it was written;
   its `SUFFIXES` did not include the extension every page on that site is
   written in. The guard built to stop a correction reaching one copy of a
   claim was blind to the copy a reader actually opens.
   `scripts/test_check_retired_claims.py` gains both halves — a page stating a
   retired claim is reported, a page stating nothing retired stays silent.

## Why

`ledger/2026-09-23-a-withdrawn-claim-is-not-stated-anywhere-as-current.md`
built that guard after three corrections in one day each reached one copy. Its
docstring is explicit that the registry is the whole mechanism and that a claim
nobody declares cannot be seen. What it did not say, because nobody had
checked, is that a claim declared *and* restated on a documentation page could
not be seen either.

That is worse than the gap it documents. A missing registry entry is a known
cost, paid knowingly. A suffix set that omits the format of the tree it walks
is a guard reporting "none restated in 622 files" while skipping the thirteen
files most likely to hold the survivor — `site/` was in `ROOTS`, so the guard
looked like it covered the site, and the number it printed was confidently
wrong rather than absent. **A check that cannot fail on the thing it is for is
the same shape as a check that never fires**, which `CLAUDE.md` calls a check
nobody has debugged.

The registration in (2) is what turned this up. Registering the phrase and then
re-running the guard should have failed on the page it was still on; it passed,
and the pass is what sent me to read `SUFFIXES`. Had I registered the phrase
*after* fixing the page and not re-checked, the entry would have claimed a
protection that did not exist.

## Alternatives rejected

**Fix the page and stop.** What the question "are the docs updated?" literally
asked for, and it would have left the mechanism that exists for this exact
failure still unable to see the site. One stale sentence is a defect; a guard
that cannot fire is the reason there will be a next one.

**Generate `roadmap.html` from `docs/orm-comparison.md`.** The real fix for two
copies of one claim, weighed properly. The page is not a rendering of the note:
it reorders the audit for a reader arriving cold, drops the per-item *Build* and
*Test* paragraphs, and carries the site's callout and navigation structure.
Generating it means a templating layer reproducing all of that, or flattening
the page into the note's shape and losing what the page is for. They are two
documents sharing facts, not one document in two formats.

**Strip HTML tags before matching, so a phrase interrupted by `<em>` matches.**
Tempting while already in the file, and rejected: stripping tags makes the
scanner an HTML parser, and a phrase that matches only after tags are removed
matches text no reader sees as one run. The cheaper discipline is to register
the longest run of *plain* words a sentence contains, which is what the new
comment on `SUFFIXES` says to do. A guard that needs a parser to be right is a
guard with a second thing that can be wrong.

**Widen `SUFFIXES` to everything and skip by extension instead.** An allowlist
that must be edited to cover a new format is the failure this just had; a
denylist is the failure one step later, when a minified bundle or a generated
`.json` produces a match nobody can act on. The allowlist stays, with a comment
saying why `.html` was missing so the next omission is edited rather than
rediscovered.

**Leave it: the "— done" three sections down already corrects the page.** That
is why this went unnoticed for eight days, not a reason to keep it. A reader
checking whether nested loading works reads the gap list and stops.

## Evidence

**The guard was blind, demonstrated before and after.** The retired sentence was
injected back into `site/docs/roadmap.html` (anchor asserted unique, restore in
a `finally`, nothing through a shell) and the guard run:

```
before:  exit 0   ok    5 retired claims, none restated in 622 files
after:   exit 1   site/docs/roadmap.html:190 states a retired claim —
                  `nested loading were built afterwards in Rust and have`
         1 stale of 5 retired claims
```

Same injection, same registry, same command; the only difference is `.html` in
`SUFFIXES`. File count 622 → 635.

**Two mutations, both caught**, in
`ledger/mutations/20260927T045459-scripts-check-retired-claims-py.json`:

| mutation | caught by |
| --- | --- |
| `.html` removed from `SUFFIXES` again | `the claim is found on a documentation page, not only in source` |
| `SUFFIXES` narrowed to `{".html"}` alone | `the claim is found on one line`, `the claim is found when it wraps` |

The second is there because the first alone would pass against a guard that
read *only* HTML, which is the mirror-image mistake and the one a hasty fix
makes. `scripts/test_check_retired_claims.py`: 14 passed, 0 failed.

**The site's own checks were green on the stale page, and still are:**

- `python3 site/check/docs.py` — *the docs site holds together* (12 pages).
- `python3 site/check/pages.py` — *10 documentation pages render, each with a
  full sidebar, a contents derived from its own headings, and a pager*.
- `python3 scripts/check_site_claims.py` — *the site's checkable claims hold
  across 27 pages, and 4 are listed as unchecked*.
- `python3 site/check/quickstarts.py` — the Go, Python and TypeScript snippets
  each insert a row and read it back through their own head node.

All four passed before this change. None of them can see a true sentence that
has stopped being true; that is what the retired-claims registry is for, and
what it could not do here.

## What this does not do

**It registers one phrase, and the registry is still the whole mechanism.**
`check_retired_claims.py`'s docstring already says a claim nobody declares
cannot be seen, and widening the file set does not change that. What changed is
that a declared claim on a page is now caught; an undeclared one on a page is
exactly as invisible as it was.

**A phrase interrupted by a tag still will not match.** `SUFFIXES` grew; the
normalizer did not. `LEADER` strips comment markers and `EMPHASIS` strips
Markdown `*_\``, and neither touches `<em>` or `<code>` — so
`have <em>not</em> crossed` is not one run of words to this guard. The new
comment says to register the longest plain run instead, which is a convention
held by whoever writes the next entry and by nothing else.

**Nine other items in the audit have a counterpart paragraph on the page and
were not read.** I checked the three this session's corrections touched. The
registry now catches a *reintroduction* of the one phrase found; it says nothing
about the eight paragraphs nobody has compared.

**The fan-out sentence added to the page is a new claim with no check behind
it.** "A path's fan-out is bounded by nothing" is true today —
`max_returned_rows` caps a predicate write's answer, not a read's — and it is
now on a user-facing page where it was only a ledger caveat before. It goes
stale on the change that *fixes* it, which is the direction a registry entry
cannot help with.

**`.html` was added to one guard, and the sweep for others found none.** Every
`scripts/check_*.py` was checked for the pattern that produced this — a guard
that reaches `site/` without reading its pages. Five deal with the site and
four already handle `.html`: `check_site_claims.py`, `check_site_css.py`,
`check_cost_prose.py` and `check_closed_caveats.py`. The rest match "site" only
as prose or as the phrase "call site". So this was one guard's omission rather
than a habit — which is the less interesting answer and the one the grep gave.

What the grep cannot say is whether a guard *should* read the site and does not
reach it at all. `check_cited_docs.py` validates `ledger/…` and `docs/…`
citations in source and prose and does not walk `site/`, so a page citing a
renamed ledger entry would be unchecked. Today no page cites one — a grep for
`ledger/` across `site/**/*.html` returns nothing — so the gap is latent rather
than live, which is why it is recorded here instead of fixed. It becomes real
the first time a page links into the ledger.
