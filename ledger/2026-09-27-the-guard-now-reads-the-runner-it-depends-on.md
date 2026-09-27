# Rule 4 excluded server examples on an assumption about `run_examples.sh` that only a comment connected. A sixth rule reads the runner's control flow and says so.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_examples_roster.py`, `scripts/test_check_examples_roster.py`, `docs/caveat-status.json`
- **Kind:** correctness

## What changed

`check_examples_roster.py` gained a rule that reads the run loop between the
`handshake "$crate" "$name"` call and the `refuses "$crate" "$name"` call, and
requires a `continue` in that window. Three failure shapes, plus a never-fires
half.

`scripts/test_check_examples_roster.py`'s summary line is counted from the
cases run rather than summed from the case lists.

## Why

`ledger/2026-09-27-two-more-examples-ignored-a-mistyped-flag.md` excluded
server examples from the refusal roster in both directions and wrote down what
that rested on:

> **The exclusion of servers from rule 4 is a property of the runner, spelled
> in the guard.** `run_examples.sh` returns to the next example as soon as a
> handshake lands, so the refusal branch is unreachable for a server. If the
> runner ever grew a refusal probe for servers, this guard would keep
> demanding that their lines be deleted and nothing would connect the two
> files.

The failure that leaves is specific and quiet: somebody adds a refusal probe
for servers, adds `s3_server`'s roster line, and this guard reports "delete
it — the refusal branch is not reached for it", which is now false. A reader
follows a true-sounding message to the wrong file.

## Alternatives rejected

**Parse the shell properly.** A `case` arm and a `for` body are what this
already slices with regular expressions; a real parse would need a shell
grammar, and `scripts/check_guard_scope.py` and `check_examples_roster.py`'s
own `body()` both take the view that a narrow pattern with a loud never-fires
half beats a general parser nobody can debug.

**Move the refusal probe out of the loop, so there is nothing to assume.**
Restructuring the runner to probe every example uniformly would make the
exclusion unnecessary — and would also spend the wait budget starting each
server twice. The exclusion is right; what was missing was anything checking
that it stayed right.

**Assert that `run_examples.sh` contains a `continue` at all.** What the first
draft did, effectively, and a mutation reading the whole script rather than
the window survived every case. See below: the two agree today and stop
agreeing the moment the loop grows a `continue` anywhere else.

## Evidence

**Two survivors, and neither was a missing rule.**

The first: searching the whole script for `continue` rather than the window
between the two calls. It survived because the fixture's only `continue` was
the one each case removed, and the real runner has exactly one too — so the
correct and incorrect versions agree on every input that existed. The case
written for it has a loop whose server branch does *not* continue and whose
refusal branch does; the whole-file search reads that unrelated line and
passes.

The second: restoring the summary line's hand-summed total. That one is
recorded as an expected survivor, because there is no test to write — a case
asserting the printed number would be the same arithmetic twice. The fix was
structural instead: `ran` counts cases as they run, so the number cannot drift
from the run. It had drifted: adding a third case list left the file printing
`30 passed` for a run of 34, and `scripts/mutate.py` reads that line as the
score.

**Mutations.** Four runs, ten cases, one survivor closed by a new test and one
recorded. Against the guard:
`ledger/mutations/20260927T011831-scripts-check-examples-roster-py.json` (four
cases, three caught and the window one surviving) and
`ledger/mutations/20260927T011852-scripts-check-examples-roster-py.json` (the
same four after the new case, with the window one now caught).
Against the test file:
`ledger/mutations/20260927T011900-scripts-test-check-examples-roster-py.json`
(the summed total, surviving) and
`ledger/mutations/20260927T011935-scripts-test-check-examples-roster-py.json`
(the same mutation after the fix, recorded as an expected survivor).

| mutation | outcome |
| --- | --- |
| rule 5 never reports a loop that reaches the probe | caught, 2 cases |
| the order of the two calls is not checked | caught |
| rule 5's never-fires half never fires | caught |
| the window searched is the whole script | **survived**, then caught |
| the summary is summed from the lists again | survived, as recorded |

**Suites.** `scripts/test_check_examples_roster.py` 35 passed 0 failed (was
30). `sh scripts/check.sh` 69 of 69.

## What this does not do

**`check_mutation_claims.py` reads a per-run phrase as a claim about every
run an entry cites.** This entry's first draft used one of the phrases that
guard watches — the two-word one meaning every mutation was caught — about a
single one of its four records, and the guard reported three problems, one for
each of the other three, all of which do record a survivor, correctly and on
purpose. Writing the phrase *again* here to name it retriggered the guard, the
same self-reference `check_cited_docs.py` hit an hour earlier, so it is
described rather than quoted. The wording is gone and the entry says the same
thing without it, but the
guard's rule is "if the entry claims a clean sweep anywhere, no cited record
may hold a survivor", and an entry citing four runs cannot describe them
separately in the vocabulary it watches. Not fixed here: narrowing it means
associating a sentence with the record it is about, which is the prose-parsing
this repository keeps declining to do, and the false positive is loud and
immediate rather than silent.

**It reads control flow by looking for a keyword in a byte range.** A
`continue` inside a nested loop in that window, or one inside a comment or a
heredoc, would satisfy the rule while continuing the wrong loop. None occurs
today and a case would have to invent one; the honest description is that this
is a regular expression pretending to be a reachability analysis, and it is
worth having because the thing it is pretending about is four lines long.

**It does not check that the exclusion is still the *right* design.** If the
runner grows a refusal probe for servers, this reports that the two files
disagree. Which of them to change is a judgement, and the message says so
rather than picking.

**The summary fix is one file, and the class is one file wide.** Exactly one
other suite here sums rather than counts — `scripts/test_check_handlers.py`,
`total = len(CASES) + len(SCOPE_CASES) + 1` — and it is correct today: it
prints 43 and emits 43 result lines, checked by counting them. So there is no
second defect to fix, only a second place the same drift can start, and it
stays summed because changing a file this entry is not about to prevent a
number it is not yet getting wrong is how a small change becomes a sweep.
