# A guard's docstring now has to agree with the paths beneath it, because the last one that did not was found by a person reading during unrelated triage.

- **Date:** 2026-09-26
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_guard_scope.py`, `scripts/test_check_guard_scope.py`, `scripts/check.sh`, `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** process

## What changed

A new guard, `scripts/check_guard_scope.py`, reads every `scripts/check_*.py`
and compares two things its docstring can say about scope with the paths the
file actually builds:

1. a tree the docstring calls **out of scope** must not be reached, and
2. a tree the docstring says it **reads** must be.

Both directions, twenty-one guards, in `check.sh` and in CI's `scripts` job.

## Why

`ledger/2026-09-24-a-guard-said-it-did-not-read-the-tree-it-reads.md` recorded
that `check_cost_prose.py` walked `docs/` while one of its own paragraphs said
`docs/` was out of scope — #283 had widened the tree, corrected the paragraph
that introduces the scope, and left the one forty lines below that restates it.
The file said both, and the wrong one was the one a reader hits looking for the
answer. That entry's own caveat named this:

> **Nothing checks a guard's account of itself.** This was found by a person
> reading, prompted by an unrelated triage. The class — a docstring that
> contradicts the code beneath it — is not covered by any check here, and the
> three other guards widened in #288 have not been read the same way.

Most of a docstring is prose a machine cannot judge. Scope is the exception:
"which trees does this read" is a claim with an answer in the file.

## Alternatives rejected

**Check the whole docstring against the code.** What the caveat asks for
literally, and not a thing that can be written: "this pattern is shaped this
way because a mutation survived" is unfalsifiable by a script. Scope is the
part that is comparable, and saying so is better than a guard that claims to
cover the class and covers one rule of it.

**Import each guard and inspect its module attributes.** Simpler than an AST
walk, and it would follow a path this cannot — a root built by a helper, a
constant computed at import. Rejected because it runs twenty scripts' worth of
module-level code to answer a question the text answers, and one of those
scripts builds a virtualenv on first use. A read-only check that has side
effects is a check people are afraid to run.

**Keep rule 2 out.** It has never had an instance here, and in its first form
it reported four contradictions, all four false. That is the argument for
deleting it — a guard whose findings need sorting is a guard people stop
reading — and the reason it stayed is that all four came from the *reader*
being too narrow, not the rule: three from a root spelled `REPO`, one from this
file's own sentence about what another guard reads. Both were fixed, and it now
reports nothing on this tree. The measurement is in **Evidence**; if it starts
producing noise, delete it and say so.

**Match a tree named without a trailing slash.** `check_cited_tests.py` says
"the ledger is out of scope by principle", meaning a ledger entry's claims are
not what it verifies — and it reads `ledger/` constantly. A guard that could not
tell that sentence from a statement about a directory walk would report a
contradiction that is not one, on the first file it read.

## Evidence

**It finds the defect it was written for.** The case *"a guard that says a tree
is out of scope and reads it is reported"* writes `check_cost_prose.py`'s old
shape — an out-of-scope sentence over a path into that same tree — and it is
reported. The fixture names `site/` rather than `docs/` on purpose:
`scripts/check_retired_claims.py` refuses the withdrawn sentence's exact words
anywhere in the tree, and it failed this file on the first run, which is that
guard doing its job. A reader grepping for a retired claim should not find it
alive in a fixture.

**The tree is clean.** 21 guards, 3 making a scope claim, every claim matching.
So this found no present defect, which includes the three guards widened in
#288 that the caveat named as unread: they are read now, and they agree.

**Two measured corrections during the writing, both to the reader rather than
to the rules.** The first version looked for a module-level `ROOT / "…"` only.
Of the 21 guards here, **10** build a tree path that way; **13** do under the
four spellings `ROOT`, `REPO`, `root`, `repo`. The three in the difference
rebuild their paths from a `root` *parameter* inside a function — this
repository's convention for a root a test can override — and against the
narrower reader, rule 2 reported three contradictions in `check_site_claims.py`
that were nothing but a name. The fourth false positive was this file's own
docstring: `"…it reads `ledger/` constantly"`, a sentence about
`check_cited_tests.py`, matched by a pattern that took any "reads" as a claim
about the file it was in. Four reports, four false, zero after both fixes.

**Mutations.** `ledger/mutations/20260926T233912-scripts-check-guard-scope-py.json`
— nine cases, nine caught, no survivors:

| mutation | outcome |
| --- | --- |
| rule 1 never reports | caught, 2 named cases |
| rule 2 never reports | caught |
| a bare tree name counts as a claim | caught |
| a claim may reach across a full stop | caught |
| any sentence with "reads" is a claim about this guard | caught, incl. the real tree |
| only `ROOT` is followed, not `REPO` or a parameter | caught, incl. the real tree |
| each of the three never-fires halves, deleted | caught, one case each |

The last two rows are the two corrections above, put back as mutations: both
are caught by the fixture cases *and* by the real-tree case, which is the
cheapest evidence that the corrections were about this repository rather than
about a fixture.

**And two of this repository's own guards failed the first `check.sh` run over
this change**, both correctly: `check_retired_claims.py` on the fixture above,
and `check_caveat_citations.py` on a verdict whose `by` field wrote
`scripts/check_*.py` — a glob, which it read as a path and could not open.
Neither is a defect in this change; both are the reason it took two runs rather
than one to be sure.

**Suites.** `scripts/test_check_guard_scope.py` 13 passed 0 failed.
`sh scripts/check.sh` 67 of 67 (two new steps). `scripts/test_check_sh.py`: 102
CI steps, all accounted for.

## What this does not do

**It reads `scripts/check_*.py` and nothing else.** A `test_check_*.py`, a
`site/check/*.py`, `mutate.py`, `caveats.py` and the two shell runners all carry
docstrings with scope claims in them, and none is read. Deliberately: the
pattern `roots` relies on — a root-ish name divided by a string literal — is a
Python convention, and the two shell scripts would need a second reader for a
third of the files. Widening to the other Python is a one-line change to
`GUARDS` and was not made today because no claim in those files is currently
wrong, and a widening with nothing behind it is how the `~~` exemption got
coarse.

**A tree reached by string concatenation, or from a root under a fifth name, is
invisible.** Rule 1 can then miss a contradiction; it cannot invent one, because
an unseen path only shrinks the `read` set. Rule 2 is the direction that *can*
go wrong that way, and the four false positives above are what that looks like
— which is why its pattern demands the sentence's subject be the file itself.

**Scope is the only part of a docstring compared.** Everything else — a comment
claiming a mutation was caught, a pattern's stated reason, a count of cases —
is unchecked here. Two of those have their own guards
(`scripts/check_mutation_claims.py`, `scripts/check_cited_docs.py`); the rest is
prose, and the caveat this closes asked for the class without a way to cover
it.
