# Every `closed` caveat now names something in the tree, and a check re-reads all 179 of them: the residual the audit recorded this morning, discharged rather than deferred

- **Date:** 2026-09-26
- **Author:** Claude, working from the standing instruction to address every caveat
- **Kind:** feature
- **Touches:** `scripts/{check_closed_caveats.py,test_check_closed_caveats.py,check.sh}`, `.github/workflows/ci.yml`, `docs/caveat-status.json`

## What changed

`scripts/check_closed_caveats.py` holds a **witness** per closed caveat — a
file that must exist, or a string that must occur in a named file — and fails
if any has gone. 179 of the 195 closed verdicts have one; the other 16 are in
`EXEMPT` with the reason each leaves no artifact. **Every closed verdict must
be in one list or the other**, so it is no longer possible to close a caveat
here without saying, in the tree, what closed it.

`scripts/test_check_closed_caveats.py` is 21 cases over real `git init`
repositories, and the guard is in `scripts/check.sh` (now 63 steps) and in CI.

This morning's audit ended with:

> **188 `closed` verdicts still rest on a task title.** The reading confirmed
> each names work the task list records as done and did not go to the tree for
> each one. That is the caveat's residual, it is unchanged, and this entry has
> now said so twice in two sections, which is the honest shape of a job that
> was bounded rather than finished.

It is finished. That caveat is `closed`, and
`2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md`'s "167 `closed`
verdicts rest on evidence of very uneven strength" is `narrowed`.

## Why

A `closed` verdict removes a caveat from every listing. `caveats.py` checks
that it names *something*; `check_caveat_citations.py` checks that a path in
that name resolves. Neither asks whether the thing named is still there — and
the entry that wrote most of those verdicts said exactly what that costs:

> Where the caveat was about a test existing, or a guard covering a case, I
> matched it against the completed task whose title names the same gap, which
> is good evidence and not proof. **A wrong `closed` is invisible.**

A task title is evidence about the past. A witness is evidence about **now**,
re-read on every run, and that is the difference the guard buys: not that the
closure was right when it was made, but that it has not since been undone. A
feature reverted, a guard deleted, a test removed — today each of those leaves
the verdict saying done and every listing hiding the caveat. From here each
turns the build red and names the caveat it falsified.

The witnesses are deliberately coarse. `("having", "…/front_end.rs")` passes on
any test mentioning `having`, not only the one the closure added. Tighter would
mean pinning test names, and a test name is renamed by ordinary work: the guard
would fail on a rename, somebody would loosen it under time pressure, and the
roster would have taught people to weaken it. **Coarse and kept beats exact and
switched off** — the trade `check_site_css.py` makes with substring matching,
argued at length there.

## Alternatives rejected

**Check each closure once, by hand, and write down the result.** That is what
the morning's audit did for the seven closures whose citation was greppable,
and it is why three of the four wrong details found today were in those seven.
It discharges the audit and nothing else: the 179th closure regresses next
month and the verdict still says done. Cost of the roster over the one-off:
about two hours of witness-picking and a 600-line file. Cost of the one-off:
the same work again, by hand, the next time anyone asks.

**Derive witnesses from the `by` prose.** A `by` that says
"`scripts/check_site_css.py`" could yield its own witness automatically, and
136 citations across the file already resolve under
`check_caveat_citations.py`. It fails on the 188 that name a task title instead
of a path — which is the entire population this exists for — and it would make
the witness a restatement of the citation rather than an independent claim
about the tree. Where a `by` does name a path, the witness here is usually the
same path, which is fine: the roster's value is that somebody chose it.

**Require a witness with no exemptions.** Some closures genuinely leave
nothing: a measurement that came back null changes no file, a review re-read
under a wider frame produces an entry and no artifact. Forcing a witness there
would produce a *plausible* one — the same failure as forcing a `by` to name a
file, which invented `scripts/check_unverifiable_claims.py` four hours ago.
`EXEMPT` names three kinds with a sentence each, and a rule ("exempt anything
closed by a measurement") was rejected for the reason every roster here gives:
a rule absorbs the next closure that should have had a witness and was not
given one.

**Key the roster by entry alone.** Shorter, and wrong: an entry has several
caveats and they close for different reasons. Keyed by `(entry, first 60
characters)`, the same key `caveats.py` uses, a reworded caveat orphans its
witness row and is reported — so it is read again rather than silently keeping
a witness chosen for a different claim.

## Evidence

`python3 scripts/check_closed_caveats.py`:

```
ok    some verdict is recorded closed at all  (195 closed verdicts)
ok    every closed verdict names a witness or an exemption
ok    every exemption used is one EXEMPT explains
ok    every witness named is one WITNESS defines
ok    no witness row outlives its closed verdict
ok    every witness is still in the tree  (179 checked)
```

**It refused its own first new closure.** Moving "188 closed verdicts still
rest on a task title" to `closed` made the check fail — `every closed verdict
names a witness or an exemption` — until a witness row was written for it, and
then failed again on `every witness named is one WITNESS defines` because the
`WITNESS` entry had not landed. That is the roster property working twice
before the entry describing it was written.

**`check_caveat_citations.py` refused the two new `by` strings** for citing
`ledger/2026-09-26-a-witness-for-every-closure.md` before this file existed.
Guard written four hours ago, catching the thing it was written for, on the
commit that closes the caveat it half-closed.

**Witness selection was probed, not assumed.** All 108 witnesses were run
against the tree before the roster was written; one was wrong on the first pass
(`window` in the Python client's `__init__.py`, which has no window surface —
the surface is `Window` in `clients/go/slate/query.go`) and was corrected by
looking rather than by loosening.

**Mutation run**, `ledger/mutations/20260926T021240-scripts-check-closed-caveats-py.json`
and the run after it — **eleven cases, eleven caught**, each by a named test,
after **three survivors** found and fixed:

| survivor | cause | what was done |
|---|---|---|
| `if not (root / path).exists(): return False` → `if False:` | redundant code — `git grep` on a pathspec matching nothing exits 1 *quietly*, so the branch could not change an answer | deleted, with the finding in a comment |
| `name.startswith("=") or name not in WITNESS` → `name not in WITNESS` | redundant code — an exemption's name begins `=` and is never a `WITNESS` key | collapsed to one condition, with the finding in a comment |
| the `JSONDecodeError` arm's `[]` → a fabricated row | **missing test** — no case wrote an unparseable status file | `malformed()` added; the behaviour was already right and nothing held it there |

Two of three survivors were redundant code and one was a missing test, which is
the split `scripts/mutate.py`'s survivor message names. A twelfth mutation
(`if name not in WITNESS: continue` → `if False:`) was refused by the runner as
**NOTHING RAN** rather than scored: it raises `KeyError` on the next line, so
the suite died instead of failing, which is the second lie the runner exists to
refuse.

`sh scripts/check.sh`: **63 passed, all of them.** `ruff` and `ty` were both
red on the new files first — `RUF005` twice, and an `invalid-assignment` where
the test's stub `WITNESS` was narrower than the real one — and are clean now.

## What this does not do

**A witness proves presence, not aptness.** It is the same limitation
`check_cited_docs.py` and `check_caveat_citations.py` carry: a witness can be
present for another reason, and a feature can exist while the caveat's real
complaint stands. That is not hypothetical — this morning's audit found exactly
one such closure ("the 762 is still not audited", where the work happened and
answered the easier half), and no witness would have caught it. What the roster
catches is the closure *regressing*, which is the direction a roster can catch.

**The witnesses are one person's choices, and 179 of them.** A witness picked
too loosely passes for the wrong reason and nothing says so; the check reports
179 as a count and cannot report how many are load-bearing. The cheapest audit
would be to break each closure in turn and confirm its witness fails — 179
reverts, which is a day and was not done.

**16 closures are exempt and are checked by nothing.** `EXEMPT` gives the kind
and the reason, not a per-caveat justification: three sentences cover sixteen
rows. A closure filed under `=read` because writing a witness was hard looks
exactly like one filed there because no witness exists.

**It says nothing about the 196 open caveats.** The tracker's bookkeeping is
now as checked as this repository knows how to make it, and the backlog it
describes is the same size it was this morning. Bookkeeping was never the work.
