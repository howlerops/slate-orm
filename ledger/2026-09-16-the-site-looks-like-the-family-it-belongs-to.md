# The site got a front door, a design language it shares with its siblings, and docs that are a directory

- **Date:** 2026-09-16
- **Author:** Claude Code, from a request to make the web UI mimic
  [ironrain.app](https://ironrain.app) and the docs resemble
  [slatedb.io](https://slatedb.io/docs/get-started/introduction/)
- **Touches:** all of `site/` — new `theme.css`, `home.css`, `fonts/`,
  `workbench.html`, `docs/` (nine pages, `nav.js`, `docs.css`), new
  `check/docs.py`; `style.css` halved; `README.md`, `site/README.md`,
  `CLAUDE.md` and three workflows
- **Kind:** refactor

## What changed

Three things, and the third is the one with teeth.

**A design language, adopted rather than invented.** `theme.css` carries the
token set from ironrain.app — dark first, a gold accent, Instrument Serif over
DM Sans with Fira Code for the wordmark, a sticky frosted header on a 1080px
column — and every page consumes it. `style.css` lost its own palette and its
half that served the old docs page; what remains is the workbench application,
with a block of aliases mapping its original names onto the shared tokens so
four hundred lines did not have to be rewritten to adopt the theme.

**A front door.** `index.html` is now a landing page: a hero, what it does, and
a section on where it stops. The workbench moved to `workbench.html`, unchanged
except for its header and its palette.

**Docs that are a directory.** `site/docs.html` — one 641-line page — became
nine pages under `site/docs/`, with a sidebar, a per-page table of contents and
previous/next. Every word of the old page survives; what changed is which page
it is on and what is above it.

## Why

The request was to look like the family. The interesting decisions are the ones
underneath it.

**The workbench was the front page for a documented reason, and the reason had
two halves.** The half that held: a landing page with a query panel two thirds
of the way down gets nobody to the panel, and the claim this project most wants
checked — that the planner picks an access path, and not the obvious one —
cannot be checked by reading. The half that did not: every visitor downloaded
roughly 690 KB of gzipped WebAssembly on arrival, *including* the ones who
wanted to know what the project was. The landing page answers that in text and
puts "Open the workbench" first, twice, above the fold and again in the next
section. `site/README.md` keeps both halves rather than quietly rewriting
history.

**The sidebar is the only thing that could go stale, so it is the only thing
generated.** Nine pages share a `<head>` and a `<header>`, which is real
duplication and the usual argument for a static-site generator. It is
boilerplate: it encodes no fact. The navigation does — a link that stops
matching the page it points at is invisible until somebody follows it — so it
lives once in `docs/nav.js` and is rendered into every page.

## Alternatives rejected

**Keeping the workbench at `index.html`.** Offered and declined. It would have
been the smaller change and would have preserved the deploy URL, at the cost of
the site having nowhere to say what the project is.

**A static-site generator.** The honest answer to nine pages sharing a shell,
and rejected for the trade this repository has already made twice: a toolchain
to install, upgrade and eventually migrate, against templating a `<head>`. The
trade flips when the content wants to live in Markdown, and it does not yet.

**Writing the sidebar into all nine pages.** No JavaScript, real URLs, and
exactly the failure `CLAUDE.md` warns about most. Rejected — but note that the
`<noscript>` fallback *is* a second copy of it. That duplication is allowed only
because `site/check/docs.py` compares the two in order, link for link and label
for label, which is the difference between a duplicate that is checked and one
that is hoped about.

**One `docs.html` with a sidebar and anchors.** Offered and declined; it would
have kept `quickstarts.py` untouched and given up per-page URLs.

**Loading the fonts from Google, as ironrain does.** This is the one the tools
decided rather than the argument. `<link rel="stylesheet"
href="fonts.googleapis.com/...">` went into every page, and the browser check
failed on the first run: `ERR_CERT_AUTHORITY_INVALID`, because this sandbox
inspects TLS. That is a local quirk — but it made the real point visible, which
is that a site making *no* third-party requests had just started making one on
every page view, and one that also fails offline. Four woff2 files, latin only,
138 KB, are committed instead.

**Fonts fetched in CI rather than committed.** They would have to come from
Google at build time, which is the same third-party dependency moved somewhere
less visible, plus a network failure that breaks a deploy.

## Evidence

The generator that produced the nine pages got two things wrong, and both were
caught by machinery rather than by reading:

1. **It indented inside `<pre>`.** Every body line got four spaces, including
   the quickstart's Python. `site/check/quickstarts.py` extracts those panels
   and *executes* them, so this would have shipped a broken quickstart. Found by
   diffing the four panels against the old page before running anything: `ts`
   and `toml` were identical, `py` and `go` differed. The dedent in the
   extractor had the same bug in the other direction. Both are now `<pre>`-aware
   and all four panels are byte-identical to what `docs.html` shipped.
2. **A stray `</div>`** closed the snippet wrapper early. It renders *nearly*
   right, because a browser repairs it silently.

The second one is why `site/check/docs.py` exists and what it checks:

```
nav.js declares a sidebar at all
every page the sidebar lists exists
every page on disk is in the sidebar
<page>: the noscript sidebar matches nav.js          (× 9)
<page>: loads the shared theme and nav               (× 9)
<page>: every element closes the one it opened       (× 11)
every relative link resolves to a file
```

**Mutations**, all caught:

| mutation | outcome |
|---|---|
| a label changed in `nav.js` only | caught, 9 pages disagree with it |
| `../workbench.html` → `../workbenchh.html` | caught, link rot |
| an extra `stray.html` in `docs/` | caught, orphan |

It runs in under a second and needs no browser, so it goes first in CI.

Also run: `site/check/quickstarts.py` — all three snippets start a node and
round-trip a row from the new page. `site/check/workbench.py` — **72 checks, 0
failures**, against `workbench.html` at its new URL with the self-hosted fonts.
An HTML nesting pass over all eleven pages, which is now part of `docs.py`
rather than a thing I ran once.

Two numbers on the landing page are printed by a run rather than written from
memory, and the first draft had both wrong. It claimed
`Index Scan on trips.by_pickup (rows=100000 cost=41.2 index-only)`; the planner
actually chooses `Table Scan on trips (rows=100000 cost=13.50 decodes=[1])` for
that query — wrong in kind, not only in the digits. The three rows shown
(132/4,837 · 161/4,832 · 237/4,789) are the real top three.

## What this does not do

**Nothing was seen.** There is no browser here that renders to something I can
look at. The checks establish that the pages load, that their markup is
balanced, that every link resolves, that the workbench still works and that the
quickstart still runs. They establish **nothing about whether it looks right** —
the spacing, the type scale, the gold against the card background, whether the
hero breathes. That needs eyes.

**The landing page's claims are not checked by anything.** `docs.py` checks
structure and `quickstarts.py` checks code; the prose asserting "three clients
that must agree" and "no unsafe anywhere" is checked by nobody, exactly as the
old page's was. Both are true today.

**No search.** slatedb.io has one; nine pages do not need it, and adding it
means an index to build, which is the build step this site does not have.

**No mobile navigation to speak of.** Under 820px the sidebar becomes a
horizontal row of links above the article. That is honest rather than good: a
real docs site would have a drawer.

**`site/style.css` still carries rules for elements the workbench may no longer
use.** The obviously-dead docs half is gone, but the JavaScript-generated
classes (`fs-*`, `bk-*`, `tree-*`) were not audited against what
`workbench.js` actually emits.
