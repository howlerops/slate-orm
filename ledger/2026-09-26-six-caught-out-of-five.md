# An entry said six mutations were caught and cited a run of five. The guard that checks such claims could not see the arithmetic; now it can, and the one wrong citation in the ledger is corrected.

- **Date:** 2026-09-26
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_mutation_claims.py`, `scripts/test_check_mutation_claims.py`, `ledger/2026-09-23-two-compile-fail-doctests-were-passing-for-the-wrong-reason.md`, `docs/caveat-status.json`
- **Kind:** fix

## What changed

`check_mutation_claims.py` compares the numbers an entry states with the runs
it cites: no count may exceed the total cases in the cited records. One-sided,
and scoped to lines that say what they are counting.

It found one entry.
`ledger/2026-09-23-two-compile-fail-doctests-were-passing-for-the-wrong-reason.md`
said *"Six mutations were caught"* over a block of six names, and cited a run of
five cases — four caught, one an expected survivor. The six names are from a
different run, made five minutes earlier, which the entry never cited. That
citation is added.

## Why

The caveat, from the entry that added the guard:

> **It still cannot tell whether the numbers in the table are the record's
> numbers.** The guard checks that a cited run exists and, if the entry claims a
> clean sweep, that the run had no survivors. An entry claiming "six caught"
> over a record with four cases passes.

It was not a hypothetical. The same entry's next caveat said why — *"The
thirteen citations were reconstructed, not recorded at the time [...] nothing
proves the run I chose is the one whose numbers were transcribed"* — and one of
the thirteen was the wrong run. An entry's evidence section is the only place a
reader can check a mutation claim without re-running it, and a citation to the
wrong run makes that check produce the wrong answer confidently.

## Alternatives rejected

**Require equality rather than an upper bound.** The strongest rule and the
wrong one: a session may run twelve mutations and write up the three that
mattered, and often should — this ledger is full of entries that ran a batch,
found a survivor, fixed it, re-ran, and wrote up the finding rather than the
fourteen cases around it. Equality would make honest summarising a failure.
Catching more than you ran is impossible; running more than you write up is
good practice.

**Re-run the committed specs and compare.** The other, larger half of the same
caveat family, and still open: it needs the suites, `cargo test --workspace`
does not fit on this container, and in CI it is a job of its own. Comparing
*stated* numbers with *recorded* numbers needs neither, and catches the class
of error that actually occurred — a transcription from the wrong run.

**Count the evidence table's rows instead of the prose.** More precise where it
applies, and it does not apply: rows group. Several entries here write "each of
the three never-fires halves, deleted" as one row for three cases, which is
better prose and would read as a discrepancy.

## Evidence

**The finding.** `ledger/mutations/20260923T135240-crates-slate-derive-src-lib-rs.json`
holds eight cases, six caught and two survived, and its six caught names are
exactly the six the entry lists. The cited
`…T140240-crates-slate-derive-src-lib-rs.json` holds five, and its Evidence
section describes those five correctly. Two runs, one cited, and the uncited
one is where the body's numbers came from.

**The false-positive measurement, which shaped the rule.** Applied to the whole
entry text, the `N cases` shape reported **nine** entries; **five** were test
suite counts, not mutation counts:

| entry | the number | what it counts |
| --- | --- | --- |
| `…every-client-sends-a-disjunction` | 131 | conformance cases |
| `…four-holes-in-the-trackers-own-data-model` | 35 | `test_caveats.py` cases |
| `…the-citation-nobody-could-follow` | 21 | the guard's own suite |
| `…the-sweep-for-none-first-that-never-happened` | 17 | suite cases |
| `…the-stylesheet-had-nothing-dead-in-it` | 9 | suite cases |

"Case" means a mutation case and a test case in the same ledger, and only the
sentence tells them apart. Scoping the patterns to lines that mention
`mutation` or a record path leaves **one** report, the real one — every genuine
claim here is written on a line that names what it is counting, because the
evidence section says "Mutations" before it counts them.

**Mutations.** Two runs, eight cases:
`ledger/mutations/20260926T235434-scripts-check-mutation-claims-py.json` (seven
cases, six caught, one survivor) and
`ledger/mutations/20260926T235500-scripts-check-mutation-claims-py.json` (the
survivor re-run after its test was written: caught).

| mutation | outcome |
| --- | --- |
| the arithmetic rule never reports | caught |
| the comparison reversed | caught, 4 named cases |
| only the first cited run is counted | caught |
| the count is scoped to the whole entry again | caught, by the test-suite case |
| a number after the noun is not read | caught |
| a spelled-out number reads as nothing | caught |
| an unreadable record counts as 99 cases | **survived**, then caught |

The survivor is the instructive one. With a malformed record as the *only*
citation, the total is zero either way and the rule does not run at all, so
returning 99 changes nothing observable. It takes a second, valid record for
the difference to appear — which is the shape of every mutation that looks
equivalent and is not.

**Four existing fixtures had to change**, and they are the rule working. Each
said "Four mutations" over a one-case record; that is the defect this rule is
for, and a fixture should not be an instance of a rule it is not about. `CLEAN`
now has four cases and the two survivor fixtures state their own.

**Suites.** `scripts/test_check_mutation_claims.py` 22 passed 0 failed (was 16).
`sh scripts/check.sh` 67 of 67.

## What this does not do

**Still nothing re-runs the committed specs.** The larger half of this caveat
family, unchanged and recorded as its own caveat already. This checks that the
numbers in an entry could have come from the runs it names; it does not check
that the runs still produce them.

**It compares totals, not the breakdown.** An entry saying "eight cases, six
caught, two survived" against a record of eight cases where *seven* were caught
passes, because eight is not more than eight. Catching the breakdown means
parsing which noun each number belongs to, and the evidence tables here are
prose in a fixed shape rather than a format — the shape that could be parsed is
the one that would go stale. The clean-sweep rule already covers the direction
that matters most: an entry claiming everything was caught beside a run where
something survived.

**A number written some other way is unread.** "A dozen", "half of them", a
figure in a table cell rather than a sentence. `NUMBER` is one through twelve
and digits, and the guard's own caveat about phrasings — *"a guard that only
recognises one way of saying a thing is a guard on a phrasing, not on a
practice"* — applies here exactly as it did to the claim patterns. It is
narrower than that risk deserves and the alternative is a number parser, which
would read every measurement in every entry.
