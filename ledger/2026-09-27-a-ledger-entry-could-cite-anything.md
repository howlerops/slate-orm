# A ledger entry could cite any filename and nothing would open it. Seven invented citations in a week, two of them sitting in committed prose.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_cited_docs.py`, `scripts/test_check_cited_docs.py`, `ledger/2026-09-25-the-grammar-block-is-checked-now.md`, `docs/caveat-status.json`
- **Kind:** correctness

## What changed

`check_cited_docs.py` reads `docs/**/*.md` and `ledger/**/*.md` as *sources*,
not only as the trees a citation points at. A new roster, `NOT_A_FILE`, exempts
the nine citations in prose that are deliberately not files — illustrative
paths, an elided filename, the template's `YYYY-MM-DD-slug` — one `(file,
citation)` pair and one sentence each, with its own never-fires half.

One dead citation was a defect and is corrected.

## Why

The guard skipped both trees, with a reason in its docstring:

> `ledger/` cites paths that were right on the day, which is what a dated
> record is for. An entry saying "moved to `docs/x.md`" is provenance even
> after `x.md` moves again, and rewriting it destroys the thing a ledger is.

That is a true statement about ledgers and it does not cover the case that
matters. A citation that resolved when it was written and has since moved is
provenance; a citation that **never** resolved is a fabrication. The two are
told apart by asking whether the file ever existed, which is what a roster does
one sentence at a time.

The distinction is not academic. This repository has recorded **seven**
invented citations in a week, always the same shape — a plausible filename
recalled instead of looked up — and every one was caught because the claim
happened to sit in a tree something opened. Two were not, and both were in
prose:

- `ledger/2026-09-25-the-grammar-block-is-checked-now.md` named a `sql.md`
  under `docs/` that has never existed. `docs/` has no single SQL page at all,
  which is itself part of why nothing audits it — the caveat was about that
  gap and named a file to represent it.
- The entry that produced this guard cited a ledger entry that has never
  existed, and `check.sh` printed `ok`. Found by `ls`.

## Alternatives rejected

**Leave it, and rely on reading.** What was in place. Seven for seven says
reading does not catch this class: the citation is plausible, the sentence
around it is correct, and re-reading confirms a name the writer already
believes. Every previous catch was a *tool* opening the path.

**Exempt whole files, as `FIXTURES` already does.** Simpler, and wrong here.
`2026-09-26-the-citation-nobody-could-follow.md` contains three illustrative
names *and* real citations to other entries; exempting the file stops checking
the real ones, which is the exemption that just cost two fabrications.
`(file, citation)` is the narrowest key that works.

**Require an entry's citations to be rewritten when a file moves.**
The alternative the old exemption was protecting against, and it is still the
wrong thing to do — a dated record that gets edited is not a record. The
roster handles it instead: when a genuinely-moved citation appears, it gets a
row saying what it used to be. Measured today: there are **none**. All nine
rostered rows are illustrative-by-intent, not moved-since.

**A pre-commit check on the entry being committed, rather than the tree.**
Would catch a fabrication at the moment it is written and would never catch
one already committed — which is exactly the two found here. The whole-tree
check finds both and costs a second.

## Evidence

**The measurement that justified the roster's size.** Before the widening:

```
439 citations inside docs/ and ledger/; 14 dead, 10 distinct
```

Nine distinct are deliberate and rostered. One was the defect. Zero were the
"right on the day, moved since" case the exemption existed for — which is why
a roster is affordable rather than a second full-time list.

**What the guard reads now.** 194 citations before, **624** after: source plus
prose, with the nine rostered pairs excluded from the count rather than folded
into it, so the never-fires guard cannot be satisfied by the roster alone.

**The roster's own never-fires half found nothing and cost fifteen cases.**
`NOT_A_FILE` names real files, so over a fixture tree every row reads as
"exempting a citation that is gone", and the first run failed fifteen of
sixteen cases with the same nine lines burying whatever each case was about.
`check` takes the roster as an argument now, defaulting to empty for a fixture.
That is the **fourth** time this session a parameter with a real default made a
test read this repository instead of its own tree — `check_cost_prose.py`'s
`docs`, then its `site`, then `check_handlers.py`'s `root` — and the first
where the offending default was a roster rather than a root.

**Mutations.** One run, seven cases, no survivors:
`ledger/mutations/20260927T010250-scripts-check-cited-docs-py.json`.

| mutation | outcome |
| --- | --- |
| prose is not read as a source | caught, 4 named cases |
| only `ledger/` is read, not `docs/` | caught |
| the roster exempts a whole file rather than one citation | caught, 2 cases |
| a rostered citation is counted as seen after all | caught, 3 cases |
| a stale roster row is not reported | caught |
| `prose_files` reads every suffix, not only `.md` | caught |
| `check` falls back to the real roster over a fixture | caught, 4 cases |

The last is the one worth the row: it is the bug that fifteen failing cases
came from, written back in as `if roster is None or not roster`, and it is
caught by four of the oldest cases in the file rather than by anything new.

**Suites.** `scripts/test_check_cited_docs.py` 20 passed 0 failed (was 16).
`sh scripts/check.sh` 69 of 69.

## What this does not do

**It checks existence, not that the entry says what the citation claims.** The
limitation `check_cited_docs.py` has always had and `check_caveat_citations.py`
inherits word for word. A citation naming a real entry about something else
passes.

**A file under `docs/` or `ledger/` that is not `.md` is read by neither
pass.** `source_files` skips anything in those trees and `prose_files` reads
only Markdown, so a script living in either would be unread. There is none
today — scripts live in `scripts/` — and there is a case named for it so that
the day one appears, the test is what a reader finds.

**`FIXTURES` still has no never-fires half.** `NOT_A_FILE` reports a row whose
citation is gone; the older whole-file roster does not, so a file listed there
that stopped containing a fixture path keeps its exemption forever. It hides
less than a path-level row does — the file is still named and still has to be
justified — which is why it is recorded rather than fixed here.

**Nothing catches the eighth invented citation before it is committed.** This
catches it on the next `check.sh` run, which in practice is before the commit
because the hook and the script are both run then. A citation invented and
committed with `--no-verify` is caught on the run after.
