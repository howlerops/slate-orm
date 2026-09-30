# Four caveats that turned out to have a check in them, and one read instead

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `scripts/check_outside_premises.py`, `scripts/prebuilt.py`, `scripts/test_prebuilt.py`, `scripts/test_check_toolchain_pins.py`, `scripts/read_deliberate.py`, `docs/caveat-status.json`
- **Kind:** process

## What changed

Four of the caveats this session set out to close were each one rule short of
being closed, and the rule was small in every case.

**An exemption has to name the path it is about.** `INSIDE_AFTER_ALL` in
`check_outside_premises.py` excuses a caveat a marker fires on whose subject is
this tree after all. Every exemption makes the same argument — the subject is a
file or a directory here, not a registry or a service — so the reason now has
to name a path that exists. One of the two did not, and its reason was widened
from "a devDependency in this repository" to
`examples/explorer/web/package.json`.

**Two of the three `manual` recheck recipes became `tree` recipes.** A third
kind: a needle counted in a named file, run on every invocation with no
network. Both converted recipes named their evidence in this repository and
nobody had noticed — "CI's three `npx playwright install --with-deps chromium`
steps are the standing evidence" is three lines of `ci.yml`, and
"`enablement: true` is kept" is four of `pages.yml`. One recipe is still
`manual`, because it wants a CI job log.

**`prebuilt.py` refuses a `SLATE_*` variable it does not know.** It checked a
hard-coded pair and said nothing about a third, which is how a second prebuilt
binary would be run against nothing. Any other `SLATE_*` variable whose value
is an executable file now fails the check by name.

**A case's expectation has to be about one report.**
`test_check_toolchain_pins.py` `in`-tests a case's expectation against the
guard's report, so an expectation broadened to a common fragment passes against
a different report — demonstrated in
`ledger/2026-09-30-the-never-fires-halves-are-a-never-fires-hazard.md` by a
mutation that changed one to `go install` and left every check green. A case
now fails if its expectation also matches a report that case never produced.
Four expectations were under-discriminating and were lengthened.

And one caveat was closed by reading rather than by a rule: *"It does not check
the session's other scripted edits for the same shape."*

## Why

The session's remaining caveats read as decisions and four of them were not.
Each said *nothing checks X*, and in each case something could, cheaply —
which is the difference between a caveat that records a limit and one that
records a thing nobody got to.

The two `manual` recipes are the sharpest of the four.
`ledger/2026-09-30-a-premise-nobody-here-can-falsify.md` wrote them off with
*"no URL answers them"*, which was true and was the wrong test: their evidence
is not on the network, it is in `.github/`. A recipe kind that counts a needle
in a file was never considered because the two kinds that existed were "fetch"
and "ask a person", and the thing in between was not named.

## Alternatives rejected

**Requiring an exemption's reason to be checked as prose**, rather than to
name one resolving path. That is the sentence-classifying problem every roster
in `scripts/` declines. What is enforceable is the *one* claim these reasons
are allowed to make, and a rule that holds them to making it is not a
spell-check: an exemption arguing anything else cannot satisfy it.

**Making `prebuilt.py` check an unknown `SLATE_*` binary** rather than refuse
it. What to compare a second binary against is a judgement — which sources, in
which tree — and a guessed comparison that passes is the silent failure the
whole file exists to prevent. Refusing by name costs the person one line in
`KNOWN` and buys that they had to think about it.

**Making the toolchain-pin cases match their report exactly**, which is the
obvious fix for a substring idiom. Rejected because the reports carry paths,
versions and counts a case has no reason to restate, so every case would
become a copy of the guard's format string and would go red on a reworded
message. What makes an expectation *about* its case is that it does not also
match a different case's report, which is checkable without pinning the text.

**Two cases observing one report is not the failure**, and the first draft of
that rule said it was: it flagged the never-fires case that legitimately sees
both halves. The rule compares against reports *outside* this case's own,
which is what "would also pass against something else" means.

**Leaving the scripted-edit caveat open** and re-reading nothing. The reading
is cheap — one diff — and the alternative is a caveat that stays open because
looking was never scheduled.

## Evidence

Six mutations, all caught, recorded under `ledger/mutations/`:

- `ledger/mutations/20260930T155938-scripts-check-outside-premises-py.json` —
  an exemption need not name a path here; a tree recipe's count is not
  compared; a tree recipe may name a file that is not there. Caught by four
  named cases, three of them new.
- `ledger/mutations/20260930T160239-scripts-prebuilt-py.json` — a second binary
  variable is passed over; a known variable is reported as unknown too. Caught
  by the two new roster cases.
- `ledger/mutations/20260930T160138-scripts-test-check-toolchain-pins-py.json`
  — **the mutation the caveat named.** One case's expectation replaced by the
  fragment two reports carry. Before this change that survived; now
  `'never sets `GOTOOLCHAIN`' is about one report and not another` fails.
  `ledger/mutations/20260930T160118-scripts-test-check-toolchain-pins-py.json`
  is the same run against the anchor before `cargo`-style rewrapping moved it,
  which `scripts/mutate.py` refused rather than scoring.
- `check_caveat_citations.py`'s three `GONE` mutations are recorded with
  `ledger/2026-09-30-the-whole-deliberate-frame-read-in-one-pass.md`.

The scripted-edit reading, over `git diff 19fb33f..db5a316` outside `ledger/`
and `docs/caveat-status.json` — 1774 added lines across 34 files:

- The class the caveat names is a *replacement applied at several sites*, one
  of which has a context the others do not. In that range there is no such
  edit. The 92 deleted lines are one block move — the staleness refusal
  leaving `conftest.py` for `scripts/prebuilt.py` — and everything else is a
  whole-block insertion.
- Two edits placed text by anchor rather than by sorted position: the imports
  in `crates/slate-server/tests/batch.rs` and `tests/server.rs`. Those are the
  failure the caveat was written about, and they were found by CI and fixed in
  `db5a316`.
- Grepped the added lines for the specific shape that bit — a `&` in a format
  argument — and for doubled-token artefacts: two `format!`/`println!` sites,
  both hand-written, neither scripted.

`read_deliberate.py`'s later-entry signal, measured against the only two rows
this repository has ever labelled false-and-found: it names the entry that
closed the first (`2026-09-14-the-home-page-is-a-workbench.md`, its single hit)
and does not name the entry that closed the second. **One of two.** Measuring
it found a defect and fixed it: an audit entry quoting a caveat matched every
rare word by construction and crowded out the entry that did the closing, so a
body containing the claim's own first sixty characters — compared with
emphasis and backticks dropped from both sides — is now skipped.

`sh scripts/check.sh`: 92 passed, all of them.

## What this does not do

**The `tree` recipe counts a needle; it does not read the workflow.** Four
occurrences of `enablement: true` in `pages.yml` is three comments and one
step, and the count does not know which is which. A comment deleted and a step
added would keep it at four. It is a tripwire on the most falsifiable part of
the claim, which is what the recipe kind is for and is weaker than reading.

**An exemption can still name a path and argue something else.** The rule
holds the reason to naming a file that is here; whether the sentence around it
is true is the reading. One claim is enforced, not the argument.

**The toolchain-pin rule only sees reports some case produced.** An
expectation broadened to a fragment of a report *no case in the file
observes* — a branch nothing reaches — still passes, because there is nothing
to compare it against. That is the never-fires half one level in, and the
halves check beside it is what covers it.

**Reading a diff is a claim about attention.** The scripted-edit closure rests
on one reading of 1774 lines by the reader who wrote most of them, which is
the ceiling `ledger/2026-09-25-the-open-caveats-nobody-re-reads.md` prices at
right about six times in seven.

**One of two is not a validation rate.** The later-entry signal has two
labelled rows in the whole repository's history and now scores 1 on them.
Nothing here says what it would do on the next false verdict.
