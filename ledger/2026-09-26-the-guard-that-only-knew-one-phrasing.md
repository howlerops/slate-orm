# The guard that makes an entry cite the mutation run behind its claim missed thirteen entries, three of them written today, because each said "six run" rather than "six mutations". It was a guard on a word order.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `scripts/check_mutation_claims.py`, `scripts/test_check_mutation_claims.py`, thirteen ledger entries, `docs/caveat-status.json`
- **Kind:** fix, in a guard — and batch nine of the re-triage pass

## What changed

`check_mutation_claims.py`'s `COUNT` pattern required the number *before* the
noun: `\b(one|two|…|\d+)\s+mutations?\b`. It now also matches a count after the
noun ("Mutations, six run, six caught") and the evidence table every such claim
in this repository carries, whose header is the literal `| mutation |` at the
start of a line. Thirteen entries it had been passing now fail it, and all
thirteen have been given the citation they owed. Six new cases in
`scripts/test_check_mutation_claims.py`. Batch nine of the reading pass is
stamped alongside: nine still true, one closed.

## Why

Because I wrote three of the thirteen this session, and the guard said nothing.

`ledger/2026-09-26-ordering-a-chains-groups-by-what-it-computed.md` opens its
evidence with "**A mutation, run twice, caught both times**".
`…-the-stylesheet-had-nothing-dead-in-it.md` says "**Mutations: six run, six
caught**". `…-the-check-the-gitignore-asked-for.md` says "Six were run; the
first survived." All three ran real mutations against real code. All three had
records sitting in `ledger/mutations/`, written minutes earlier by `mutate.py`
itself. None cited one, and `scripts/check.sh` reported 59 passed.

The guard's own docstring explains, at length and correctly, why a count is
required and why ambient prose about mutation must not trigger it. What it did
not consider is that the same claim can be made with the number on the other
side of the noun, or with no number in the sentence at all because it is in the
table underneath. **A guard that recognises one way of saying a thing is a
guard on a phrasing, not on a practice** — and the phrasings that escaped were
not exotic, they were the ones a person reaches for when the table is doing the
counting.

Ten of the thirteen are from 2026-09-25, the earlier part of this same session,
which makes the failure rate over the guard's whole life close to total: 23
entries now count mutations under the rule, and it was catching 10.

## Alternatives rejected

**Rewrite the thirteen entries to use the phrasing the guard knows.** Fixes
today and guarantees tomorrow's failure, because the next person writes what
reads well rather than what the regex expects. It also edits dated records to
suit a tool, which is backwards.

**Drop the count requirement and demand a citation from any entry mentioning
mutation.** Every entry here carries policy prose — "a surviving mutation is a
missing test" — and roughly 245 mention the word. The docstring already
rejected this and was right to.

**Parse the Markdown.** A real table parser would find the evidence table
reliably and would be a dependency and a second thing to keep in step with how
these entries are actually written. The header is a fixed literal at the start
of a line, which is enough and is testable.

**Anchor the table arm loosely** — match `| mutation |` anywhere on a line.
Rejected because a mutation survived it and showed why: an entry *describing*
this guard quotes the header inline, in a sentence, as the one you read two
paragraphs ago does. Without the `^`, that quotation reads as a claim to have
run mutations and the entry is told to cite a run it never made. The case that
pins it was written after the survival, not before.

## Evidence

**The gap, measured.** Before: `ok 10 entries count mutations, all citing a
recorded run`. After widening, before the citations: **13 problems of 23
counted claims**. After the citations: `ok 23 entries count mutations, all
citing a recorded run`.

Every one of the thirteen had a record. They were matched to their runs by
subject file, timestamp and case names — `20260925T213846` is the run whose
survivor `…-or-in-having-too.md` describes, `20260926T004215` is the one whose
survivor `…-the-check-the-gitignore-asked-for.md` describes, and so on. Two
entries cite a run recorded as `interrupted`, which is what those runs were and
what those entries say.

**Mutations: five run, five caught**, each by a named case:

| mutation | caught by |
|---|---|
| the number-after-the-noun arm dropped | `a count after the noun is a claim` |
| the evidence-table arm dropped | `an evidence table is a claim even with no count in the prose`, `a table alone, with no sentence about mutations at all` |
| the table arm's `^` anchor dropped | `an entry quoting the table header inline is not claiming a run` |
| `re.MULTILINE` dropped | `an evidence table is a claim even with no count in the prose` |
| the original number-before-the-noun arm dropped | 4 cases |

The third survived its first run — the case meant to pin the anchor used a
table whose *cell* said "a mutation would be nice", which `\|\s*mutation\s*\|`
never matched either way. The replacement quotes the header in prose, which is
the thing the anchor actually protects against.

**The runs themselves**, as `mutate.py` recorded them:

- `ledger/mutations/20260926T005312-scripts-check-mutation-claims-py.json` — the run with the surviving anchor case
- `ledger/mutations/20260926T005339-scripts-check-mutation-claims-py.json` — all five, after the case was rewritten
- `ledger/mutations/20260926T005701-scripts-check-mutation-claims-py.json` — the two arms re-run after `ruff format` rewrote the file, because a moved anchor is the first way a mutation run lies

**`scripts/test_check_mutation_claims.py`: 16 passed, 0 failed** (11 before).

**And it caught this entry, immediately.** The two record names above were
written from memory before the files were listed, and both were wrong —
`005530` and `005739` against the real `005312` and `005339`. The guard
answered `2 problem(s) of 24 counted claims` and named them. That is the
citation-to-a-missing-record arm, which had only ever fired in a test, firing
on its author within a minute of the pattern being widened, and it is the same
mistake this session already made once with four invented `ledger/…` filenames.
A guard whose first real catch is the person who just edited it is a guard
worth having.

**Batch nine, nine still true, one closed:**

| caveat | checked against | verdict |
|---|---|---|
| no mutation spec is committed anywhere | `ledger/mutations/` holds **80** records, each carrying the file, dialect, command and every case's `old`/`new` — the spec, committed | **closed** |
| the conformance runner compares behaviour, not formatting | two mentions of "format" in `conformance.py`, both about float rendering and the runner's own output | still true |
| `SKIP_PARTS` is a hand-written roster | `scripts/check_cited_docs.py:49`, still a literal `frozenset` | still true |
| no way to inventory what is blocked | no tool; `is_blocked`/`why_blocked` are per-plan | still true |
| nothing checks `EXPLAIN` over an expanded view | no view-aware explain check anywhere | still true |
| array elements decode dynamically | `read_array` at `crates/slate-tuple/src/codec.rs:714` reads each element's own tag | still true |
| the array design note's two questions are open | unchanged | still true |
| no `#[derive(Record)]` array support | no `Array`/`List` in `crates/slate-derive/src/` | still true |
| four fingerprint implementations, held together by the live suites | Rust, Python, TypeScript and Go each have their own; no guard compares them | still true |
| the testserver cannot represent a decimal | no `decimal` in `crates/slate-testserver/src/` | still true |

## What this does not do

**Nothing re-runs the committed specs.** This is the residual of the caveat
closed above, and it is the half that was always the larger one: the caveat
said "nothing re-runs today's mutations tomorrow. A committed spec per module
that CI drives is the obvious next step." The specs are committed and CI does
not drive them. Doing so needs the suites — `cargo test --workspace` does not
fit on this container, and in CI it would be a job of its own — so it is a real
piece of work rather than an oversight, and it is recorded here as a fresh
caveat rather than left inside a closed one.

**A fenced code block containing the table header at the start of a line still
counts as a claim.** The `^` anchor protects inline quotation and nothing
strips fenced blocks, so an entry showing the table shape in a ``` block would
be told to cite a run. No entry does this today, and the fix — stripping code
blocks, as `check_site_claims.py` does — was not worth adding for a case that
has never occurred.

**The thirteen citations were reconstructed, not recorded at the time.** Each
was matched by subject file, timestamp and case names, and in every case the
match is unambiguous. But it is a reconstruction: the entry did not say which
run it meant, which is the whole defect, and nothing proves the run I chose is
the one whose numbers were transcribed. Two entries ran the same file twice
within a minute, and there I cited both.

**It still cannot tell whether the numbers in the table are the record's
numbers.** The guard checks that a cited run exists and, if the entry claims a
clean sweep, that the run had no survivors. An entry claiming "six caught" over
a record with four cases passes.
