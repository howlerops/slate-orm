# `check_cost_prose.py` walks `docs/` and its docstring says it does not. Found while triaging caveats, not while looking for it.

- **Date:** 2026-09-24
- **Author:** Claude Code, working #301 (F7b)
- **Touches:** `scripts/check_cost_prose.py`, `scripts/retired_claims.json`, `docs/caveat-status.json`
- **Kind:** a stale claim inside a guard, and the second batch of caveat triage

## What changed

One paragraph of `check_cost_prose.py`'s docstring said:

> `docs/` is still out of scope, and deliberately

It has not been out of scope since #283. `docs` is walked at three call sites —
`check`, `freeze` and `main` each take `docs: Path | None = DOCS`. The
paragraph twenty lines above it describes the widening correctly, which is what
makes this hard to see by reading: the file states both, forty lines apart, and
the wrong one is the one a reader hits looking for the scope.

It now says what is actually out of scope — `site/` — and keeps the
historical-narration reasoning where it belongs, as the explanation for why
`NOT_A_CLAIM` exists rather than for an exclusion that ended.

The phrase is registered in `scripts/retired_claims.json`, so it cannot come
back.

**Also, the second batch of caveat triage.** The 22nd's 51 bullets: 22 open, 8
closed by later work, 21 deliberate. Running total **765 caveats: 48 open, 9
closed, 33 deliberate, 675 untriaged.**

## Why

Three of the eight closures were verified in code rather than inferred from a
task title that matched. That is how this was found: checking whether
"it reads `docs/` only" was still true meant reading what the guard reads, and
the docstring disagreed with itself.

The failure is yesterday's exactly — #283 widened the tree, updated the
paragraph that introduces the scope, and left the paragraph that restates it.
A correction reached one copy of the claim. That this one lived inside a guard
built *to catch stale claims* is the part worth recording: the guards check
prose about cost constants, and nothing checks a guard's account of itself.

## Alternatives rejected

**Delete the paragraph.** Shortest fix. Rejected because the reasoning in it is
still true and still load-bearing — a guard that cannot tell "it costs three"
from "it cost three until #269" really would fire on every paragraph of
`correctness.md`. What changed is that `NOT_A_CLAIM` answered that objection.
Deleting the paragraph would delete the reason the marker exists.

**Mark it historical with `NOT_A_CLAIM` and move on**, which the file's own
convention allows. Rejected: the sentence is not narration, it is a false
statement of current scope. The marker exempts a claim from checking; it does
not make a wrong claim right.

**Leave the caveat `open` and fix it later.** It was found *because* of the
triage and would have been recorded as closed by #288 without the read. Closing
a caveat on a guard whose docstring is wrong about the same subject would have
put a wrong verdict in the registry.

## Evidence

`scripts/check_cost_prose.py` walks `docs`: `scripts/check_cost_prose.py:401`,
`:431`, `:545`, each `docs: Path | None = DOCS`, with `DOCS = ROOT / "docs"` at
`:96`. The claim is false by reading three lines.

`check_cost_prose.py` itself: **25 prose claims about the cost constants, all
current.** `check_retired_claims.py`: 4 retired claims, none restated.
`scripts/check.sh` at **53 passed, all of them.**

No mutation was run: the change is one docstring paragraph and three registry
rows. There is no branch to break.

## What this does not do

**Nothing checks a guard's account of itself.** This was found by a person
reading, prompted by an unrelated triage. The class — a docstring that
contradicts the code beneath it — is not covered by any check here, and the
three other guards widened in #288 have not been read the same way. That is the
obvious follow-up and it is not done.

**Eight closures, three verified in code.** The other five were matched against
a completed task whose title names the same gap, which is good evidence and not
proof. A wrong `closed` is invisible, as the previous entry recorded.

**675 caveats remain untriaged**, down from 723. At this rate the backlog is
many sessions of work, and the tracker is what makes that survivable rather
than what makes it fast.
