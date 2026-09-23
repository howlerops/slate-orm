# The cost-prose guard read `crates/` and not `docs/` — so the one place the stale figure survived nine tasks was the page a reader actually opens.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #283 (F6c)
- **Touches:** `scripts/check_cost_prose.py`, `scripts/test_check_cost_prose.py`, `docs/correctness.md`, `docs/performance.md`
- **Kind:** widening a guard to the tree it was always about

## What changed

`check_cost_prose.py` now reads `docs/*.md` as well as `crates/**/*.rs`.
Markdown is chunked by paragraph rather than by line, fenced blocks are
skipped, and prose that trips the patterns without asserting a current value is
let through two ways:

* **`~~strikethrough~~`**, already supported, for a withdrawn figure. Preferred
  wherever the paragraph *also* states the live figure, because it leaves that
  one under the guard.
* **`<!-- not a cost-model claim -->`**, new, for a paragraph that is history
  or is about a different quantity. It covers the paragraph it sits in, and the
  next one when it stands alone — but not the one after that.

Claims under guard went from **19 to 24**. Five of the six things it found in
`docs/` were already correct and are now checked; six paragraphs were marked or
struck.

## Why

#278 recorded the reason in one sentence, in the doc itself: *"This sentence is
the last place the old figure survived, because `scripts/check_cost_prose.py`
reads `crates/` and not `docs/`."* The guard existed, the stale claim was in
the tree it did not read, and the tree it did not read is the one a reader
opens. That sentence is now false, and correcting it is part of this change.

The three-way split in `docs/` prose is the whole difficulty, and it is not
hypothetical — all three are on the page today:

| the sentence | what it is | how it is handled |
|---|---|---|
| "`n > 8000k` rather than `n > 24000k`" | history beside the live figure | strike the old one |
| "3,774 rows per request … 980 after 200 point reads" | a withdrawal quoting what is withdrawn | marker |
| "a point read costs three object-store GETs" | **a different quantity** | marker |

The third is the one worth naming. `POINT_READ_COST` is the planner's unit of
work; object-store GETs are what the store actually issues. In a build with the
cache compiled out they were 3 and 1.0 respectively, at the same moment, both
correct. A pattern matching "point read costs N" cannot tell them apart, and no
amount of tuning will teach it to.

## Alternatives rejected

**Detect past tense.** "cost" versus "costs", "was", "before", "rather than".
It reads well in the three passages that exist and would be wrong on the
fourth. #281 is four days of evidence for what happens when a text pattern is
tuned to the corpus in front of it: the lookahead that stopped `1,221` being
read as `1` immediately started reading `~3x` as `3`.

**Use strikethrough everywhere and add no marker.** One mechanism instead of
two. It renders a line through the text, which is right for a withdrawn figure
and wrong for a sentence that is meant to be read — "a point read costs three
object-store GETs" is a *finding*, not an error, and striking it would
misrepresent it as one.

**Mark the whole file.** `correctness.md` narrates this history at length, so
excluding it wholesale is tempting and would have taken one line. It also holds
two live crossover claims (`n > 8000k`, twice) that are exactly what the guard
is for. Marking paragraphs keeps them.

**Reword the prose to dodge the patterns.** Changing "rows per request" to
"rows for each request" would silence the guard without changing what is true.
Shaping prose around a checker is how a checker stops meaning anything.

**Widen to `README.md`, `site/` and crate-level docs too.** The same argument
applies and they are the honest next step; they are also a different corpus
with different conventions, and #279 is a fresh lesson in widening to the wrong
tree in one go. Recorded below as not done.

## Evidence

**Six mutations, six caught**, each by a named case: docs not read at all, the
marker not excusing its own paragraph, the marker excusing everything after it
for ever, a fenced figure counting as prose, a markdown paragraph not joined
across lines, and `main` ignoring the docs tree it was handed.

**The widening found no stale claim today, and that is the honest result.** All
six hits in `docs/` are legitimate history or a different quantity. The one
genuinely stale sentence this was aimed at — *"a point read costs about 3"* —
was found and struck by #278 by hand. The value here is preventive: the next
one is caught, and the paragraph that recorded "this guard cannot see me" no
longer can.

**Two fixture defects, both of the #281 class, both found by running.**
`check` gained a `docs` parameter with a real default, and the never-fires case
— an empty tree, which must fail — started *passing*, because `main` was
reading the repository's five real doc claims over the fixture's zero. A
parameter a test cannot override is a parameter the test is not exercising.
`main` now takes it too, and the fixture passes `None` explicitly rather than
letting anything default.

**The suite's own total was stale.** Adding eight markdown cases moved the
printed count by **zero**: it was `len(CASES) + 4`, a second copy of the
roster's size. That is #268's drift, here rather than in the conformance
runner. It counts what ran now — 39 reported lines, 39 counted.

`python3 scripts/check_cost_prose.py`: ok, 24 claims, all current.
`python3 scripts/test_check_cost_prose.py`: 39 passed, 0 failed.
`sh scripts/check.sh`: 46 passed, all of them. `ruff` and `ty` clean.

## What this does not do

**It reads `docs/` only.** `README.md`, every `site/` page and every crate-level
`README` can still carry a stale figure. `site/` in particular restates
performance numbers for visitors and is the next tree to take.

**The marker is coarse.** It excuses a paragraph, so a paragraph carrying both
a historical figure and a live one loses the live one — which is why
strikethrough is preferred where it fits. One place accepts that cost knowingly:
`performance.md`'s withdrawal paragraph says "`SCAN_ROW_COST` is unchanged at
8,000 rows per request", and that claim is no longer checked *there*. It is
checked at two other sites on the same page, so no coverage is lost overall,
but the marker did not know that — I did.

**Fenced blocks are unread.** A stale figure inside a pasted transcript is
invisible. That is deliberate — pasted output is a record of what a run
printed, not a claim about now — but it does mean a hand-written "example"
inside a fence is a blind spot.

**Nothing checks that a marker is still deserved.** A paragraph rewritten from
history into a live claim keeps its marker and goes unchecked. The roster-style
answer — report markers that no longer sit above a matching claim — would catch
it; this does not do it.
