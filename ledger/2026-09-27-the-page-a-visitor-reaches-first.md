# The cost-prose guard's fourth and last tree. `site/` was excluded for a reason that was not true: neither site check reads a cost figure.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_cost_prose.py`, `scripts/test_check_cost_prose.py`, `docs/caveat-status.json`
- **Kind:** process

## What changed

`check_cost_prose.py` reads `site/*.html` and `site/docs/*.html` as prose, the
same way it reads `docs/*.md` and the READMEs. HTML pages go through the
Markdown path deliberately: the chunking is by blank line either way, and
`<!-- not a cost-model claim -->` is native there.

## Why

Two caveats, one sentence apart in intent:

> **It does not read `site/`** — `2026-09-22-the-stale-claim-was-in-the-file-everyone-reads-first.md`

> **It guards `crates/`, `docs/` and the READMEs** — `2026-09-23-an-unverifiable-claim-is-refused-now.md`

And the guard's own docstring said the exclusion was deliberate: *"`site/` is
out of scope, and deliberately: `site/check/docs.py` is that tree's guard."*
That is not what `site/check/docs.py` does — it checks the pages hold together
— and `scripts/check_site_claims.py`, the one that *does* read the site's
prose, checks counts, lints and directory names, not cost figures. So the
paragraph was a reason that had never been true rather than one that had gone
stale, which is a different and worse thing for a docstring to contain.

This is the fourth tree and the third time the same widening has been made one
directory at a time: `crates/` alone, then `docs/` (#283), then the READMEs
(#288), now `site/`. Each time the excluded tree was the one a reader actually
reaches.

## Alternatives rejected

**Leave it, because nothing there states a cost figure.** Measured first, and
true: `grep` over `site/` for every shape the guard knows finds nothing. That
argues against urgency, not against the guard — the same was true of `docs/`
before a `correctness.md` sentence said "about 3" for nine tasks. A guard added
when the first stale figure appears is a guard added after somebody read it.

**Strip HTML tags before matching.** Would make the patterns robust to
`<em>three</em> requests`, and it is the obvious next thing. Rejected today
because a tag stripper is a parser and this file is three hundred lines of
regular expressions on purpose; the hole is recorded below rather than papered
over with a `re.sub` that would silently join words across a `<br>`.

**Give `site/` its own guard, beside the two in `site/check/`.** The symmetric
answer, and it would put the cost-model patterns in two files. The patterns are
the hard part and they are already written; the tree is a glob.

## Evidence

**The tree is clean, and that is the finding.** 25 prose claims over four
trees, all current. `site/` contributes none — the run says so rather than the
grep alone.

**Mutations.** Two runs, five cases:
`ledger/mutations/20260927T001746-scripts-check-cost-prose-py.json` (four cases,
three caught, one survivor) and
`ledger/mutations/20260927T001819-scripts-check-cost-prose-py.json` (the
survivor re-run after its test was written: caught).

| mutation | outcome |
| --- | --- |
| site pages are not read | caught, 4 named cases |
| only the site root is read, not `site/docs/` | caught |
| `check()` stops passing the site tree through | caught, 4 named cases |
| an HTML page is chunked as code rather than as prose | **survived**, then caught |

The survivor is the useful one. Chunking HTML with the *code* chunker left all
four site cases passing, because the patterns find the sentence either way —
the flag's only observable effect is whether `NOT_A_CLAIM` is honoured. A page
whose historical paragraph carries the marker is the case that tells the two
chunkers apart, and it is the case that was missing.

**Five fixtures were passing `site` by default.** `run`, `run_markdown`,
`run_readme` and the two `main` cases all called `check` with four arguments,
so the new fifth defaulted to this repository's own `site/`. It changed no
count today, because that tree states no claim — which is exactly how the same
bug hid in `docs` and `readmes` before it, and the note in `run`'s docstring
already said so. All five now pass `None`.

**Suites.** `scripts/test_check_cost_prose.py` 65 passed 0 failed (was 60).
`sh scripts/check.sh` 67 of 67, including `check_guard_scope.py`, which would
have failed on the docstring if the old "out of scope" sentence had been left
above a path into `site/`.

## What this does not do

**Tags are not stripped.** `A point read costs <em>about three</em> requests`
matches nothing, and a figure inside any inline tag is invisible. Every claim
on this site today is plain text inside a `<p>`, which is why the hole is
recorded rather than closed: closing it means a parser, and the guard's whole
shape is an argument against one.

**Only `site/*.html` and `site/docs/*.html`.** The generated bundle, the
snippets and the data directory are not read, and a claim in `site/workbench.js`
— a comment, a label — is not prose this sees. The two globs are where the
pages a visitor reads live; a third would be a guess about where the next one
goes.

**It still reads two constants.** Unchanged from #278 and #288 and now
conspicuous in a fourth tree: a figure derived from anything but
`POINT_READ_COST` or `SCAN_ROW_COST` is not a claim this knows how to check,
wherever it is written.
