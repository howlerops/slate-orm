# #287 built the records and closed by naming what it had not done: "an entry can still say 'four caught, none survived' with no run behind it, and nothing checks it." This checks it.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #290 (F6j)
- **Touches:** new `scripts/{check_mutation_claims.py,test_check_mutation_claims.py}`, `scripts/{check.sh,check_cited_docs.py}`, the #291 entry
- **Kind:** making an existing claim checkable, narrowly, and saying where the narrowness is

## What changed

An entry that **counts** mutations must cite the run:

```
ledger/2026-09-23-an-unverifiable-claim-is-refused-now.md counts mutations
and cites no run.
  `mutate.py` records every run under `ledger/mutations/`; name the one
  behind the claim, as `ledger/mutations/<file>.json`.
```

Two things are verified: the cited record exists, and — the half with teeth —
**an entry claiming every mutation was caught beside a record holding a
survivor is refused.**

It fired on exactly one entry when written: the one from #291, three hours
old, whose two runs were on disk and uncited. They are cited now.

## Why

A count is the part of an entry a reader trusts without checking, and until
today there was nothing to check it against. The records exist now, so the
claim can be tied to them — and #287's own closing paragraph is the
specification.

## Alternatives rejected

**Require it of every entry.** 245 mention mutation, 73 state a count, and
four records exist. The rule would fail 241 entries for predating the
mechanism that would satisfy them, which is a finding about nothing. It starts
at `FIRST_DAY` and everything before is exempt *by construction* rather than by
a list somebody prunes — the shape `check_table_provenance.py` already uses for
seventy-three frozen tables.

**Pair entries with records by inference**, on date and filename: an entry
written today claiming a mutation of `x.py` should have a record from today
touching `x.py`. Wrong in both directions. An entry legitimately cites
yesterday's run; several entries share one run; one task runs the same file
three times. "A record exists nearby" is not evidence that *this* claim came
from it, and demanding a same-day record fails honest entries. A citation says
which run, and nothing else does.

**Match any prose about mutation.** Every entry here carries policy sentences —
"a surviving mutation is a missing test" — and each would demand a citation. A
*count* is what distinguishes a claim from a statement of practice, and that
distinction is a test.

**Treat `survived-as-recorded` as caught.** Tempting, since an expected
survivor is a healthy outcome. Refused: an entry saying "all caught" beside a
run where one survived on purpose is still a sentence that does not describe
the run. The fix is to write the sentence accurately, not to widen what
"caught" means. That is a case.

**Let it validate `ledger/…` citations generally.** `check_cited_docs.py`
deliberately does not, and its reasoning is right: an entry cites what was true
on its day, and rewriting it destroys the record. The exemption this takes is
narrow and worth stating — mutation records are themselves dated, append-only
and never rewritten, so a link between two dated records cannot rot the way a
link into a moving tree does.

## Evidence

Seven mutations, **all caught**, recorded in
[`ledger/mutations/20260923T092527-scripts-check-mutation-claims-py.json`](mutations/20260923T092527-scripts-check-mutation-claims-py.json):
widening the date scope, moving the boundary by a day, accepting a missing
citation, accepting an absent record, narrowing what counts as a survivor,
never reporting the contradiction, and dropping the count requirement.

Eleven cases, including the boundary in both directions — an entry dated the
day before `FIRST_DAY` is exempt, an entry dated *on* it is not — and a
malformed record, which must neither crash the guard nor quietly read as "no
survivors".

Writing this entry demonstrated the guard on its author. The citation above
was typed before the run it names existed, so the next mutation run found the
suite already red — `the real ledger's claims are all cited` was failing, on
this entry, for citing a record that was not there. `mutate.py` recorded that
unscoreable run too, in
[`ledger/mutations/20260923T093203-scripts-check-mutation-claims-py.json`](mutations/20260923T093203-scripts-check-mutation-claims-py.json)
with `"outcome": "baseline-red"`, which is #287's mechanism doing exactly what
it was built for one task earlier.

`sh scripts/check.sh`: **48 passed, all of them**, the new one included.

Two guards objected and both were right. `check_cited_docs.py` read the
docstring's `ledger/<date>-a-slug.md` — the filename *shape* the guard matches
— as a citation to a file that does not exist; it is in that checker's fixture
list with a sentence saying why, which is how that list is meant to grow.

## What this does not do

**It does not verify that a cited run is the right one.** An entry can cite any
record that exists, including one from another task about another file.
Pairing them properly is the inference problem rejected above; this checks that
a run exists and that a clean sweep is not claimed over a survivor.

**It reads one shape of claim.** `N mutations`. "Every case was caught", "the
suite caught all of them", "no survivors this time" — all uncounted, all
invisible to it. The pattern was built from how entries here actually word it,
not from a survey of how they could.

**It cannot see a hand-run mutation**, which leaves no record. `CLAUDE.md`
already discourages those, and this adds a second reason: an entry describing
one cannot comply without citing a run that was never written.

**`FIRST_DAY` is a date in a file.** It is right today and nothing checks it
against when records actually began — that is one more constant a person keeps
true, which this repository has spent the week removing elsewhere. It is here
because the alternative, deriving it from the oldest record, would move the
boundary every time the directory is pruned.
