# `SCAN_ROW_COST` still said the scan disagreement was not understood, two tasks after it was — and fixing that found that nothing checks a source file's ledger citation.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #275 (F5u)
- **Touches:** `crates/slate-kernel/src/stats.rs`, `scripts/{check_cited_docs.py,test_check_cited_docs.py}`
- **Kind:** docs, and a guard

## What changed

Two doc comments on the planner's cost constants, and one guard.

`SCAN_ROW_COST` now says that its 8,000 rows per request is the **fully-cached
case** and carries the other three measured values. `POINT_READ_COST`'s closing
paragraph — *"until that is understood, moving `SCAN_ROW_COST` would be
calibrating against a measurement one of the two says is wrong"* — is struck
through, because #270 understood it.

`scripts/check_cited_docs.py` now treats `ledger/*.md` as a citation as well as
`docs/*.md`, and `main` takes a root so its never-fires branch can be tested.
`test_check_cited_docs.py` goes from 11 cases to 15.

## Why

#269 changed `POINT_READ_COST` from 3.0 to 1.0 and deliberately left the scan
side alone, saying so in the constant's own doc comment: the two examples
disagreed about a cold full scan of the same fixture, 58 requests against 205,
and half a recalibration against a self-contradicting measurement is the error
this area has already seen twice.

#270 resolved that. Neither example was wrong; they scan stores in different
cache states, and a *partially* populated block cache fragments a scan into
many small ranged reads. It updated `docs/performance.md` — which is current
and correct — and did not update the constant the argument is attached to.

So the source of truth a reader of the planner reaches first said an open
question was open when it had been answered, and gave the wrong reason for
leaving the constant alone. The real reason is better: there are now three
explained values spanning 7×, and choosing one is a decision about which cache
state a server should assume, which depends on its workload rather than on its
data. `CLAUDE.md`: stale documentation is worse than none, because it is read
as current.

The guard is the second half, and it is why this is one task rather than a
two-line edit. The corrected comment cites the ledger entry that did the work —
and nothing would have noticed if that name were typed wrong.
`check_cited_docs.py` exists for exactly that failure ("*code naming a document
that no longer exists — a comment or, worse, an error message that sends a
reader to a file that is not there*") and matched only `docs/`. **Eight source
files cite a ledger entry** — two in `slate-kernel`'s source, two more in its
and `slate-orm`'s tests, two in `scripts/` and two in `examples/retention/` —
and none of them was checked. All eight resolve today.

## Alternatives rejected

**Change `SCAN_ROW_COST` to one of the three.** The same refusal #270 made, and
it still holds; the comment now states it correctly. Setting it to the
post-`analyze` value (542 rows per request) would make the model's verdict on
`cost_at_scale`'s fixture come out a near-tie rather than 15× wrong — the index
issues 368 requests against the scan's 370 — but it would be tuning one number
until one benchmark agrees, and would move access paths on every workload whose
cache is warmer than that one. The oracle suites would have to run, and they do
not fit on this disk in one go.

**Leave the comment and note the correction only in `docs/performance.md`.**
Where it already is. Someone reading `stats.rs` to decide whether to touch the
constant does not read the doc first — that is the whole reason the rationale
lives in the source.

**Delete the struck-through paragraph.** `ledger/README.md` is explicit about
this: strike through what was wrong when written, leave it legible, put the
correction beside it. The paragraph was a correct statement of what was known
in #269; it is the passage of time that made it false, and the sequence is the
useful part.

**Exempt `ledger/` from citation checking, as the guard's docstring already
does.** Read carefully, that exemption is about which files are *searched* — a
dated entry's own citations are provenance and must not be rewritten — and not
about source naming an entry. The docstring now says which of the two it means.
An entry is also unusually stable, so this rule will rarely fire; it costs one
regex alternation and catches a typo on the day it is made.

## Evidence

`sh scripts/check.sh`: **40 passed, all of them.**
`python3 scripts/test_check_cited_docs.py`: **15 passed, 0 failed** (11 before).
`python3 scripts/check_cited_docs.py`: *ok    122 `docs/*.md` and
`ledger/*.md` citations, all openable* — 114 `docs/` and 8 `ledger/`, counted
separately over the same file list, so the eight are exactly what this adds and
every one resolves.

`cargo doc -p slate-kernel --no-deps` produces no warning naming `stats.rs`;
the sixteen it does produce are pre-existing and in `chain.rs` and `error.rs`.

**Mutations via `scripts/mutate.py`, nine.** Eight in the guard, one in
`stats.rs`; both runs exit 0. Each half of the citation pattern deleted
separately, each half of the searched-directory exclusion, the resolution check
removed, the fixture roster ignored, the `.md` requirement dropped, and the
never-fires guard deleted — plus, against the real tree, the ledger entry
`stats.rs` cites renamed to one that does not exist.

Two of those needed a test written before they could be caught, and both are
the same shape as yesterday's finding:

- **The never-fires guard survived.** It lives in `main`, which read `ROOT`;
  every test called `check` directly. Deleting the guard changed no verdict
  because this tree always has citations. `main` takes a root now.
- **The real-tree mutation could not be scored at all.** This guard's two
  sibling checks each end their suite with a case against the real tree; this
  one had none, so a broken citation here was reported only by
  `check_cited_docs.py`, which prints no line `mutate.py`'s dialects read —
  "the command reported no test results at all". With the case added, renaming
  the cited entry fails `every citation in this repository resolves`.

## What this does not do

**It does not change any constant.** No plan moves. The comment is now a
correct account of an open modelling question rather than an out-of-date
account of a closed measurement one.

**It does not check that a cited entry says what the citing code claims.** Only
that the file opens. A comment pointing at the wrong entry, or at one whose
conclusion has since been withdrawn in place, reads as current and is not
caught. The same limit applies to the `docs/` half and always has.

**It does not check citations in Markdown.** `site/check/docs.py` requires
every relative link in `docs/` to resolve; a `ledger/` entry citing another
entry is checked by nothing, deliberately — see the docstring.

**The eight existing citations were all correct.** A null result: this guard
found no broken citation on the day it was written, and is worth having for the
next one rather than for today.
