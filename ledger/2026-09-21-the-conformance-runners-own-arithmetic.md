# The runner that decides whether three SDKs agree was counting its own results wrong, and had no test to say so.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #268 (F5n)
- **Touches:** `examples/explorer/conformance/{conformance.py,test_conformance.py}`, `scripts/check.sh`, `scripts/test_check_sh.py`, `.github/workflows/ci.yml`
- **Kind:** fix + test

## What changed

`conformance.py` keeps findings as a `Finding(case, lines)` rather than a flat
list of display strings, and `tally` computes the summary from them. A finding
about no single case — a `MUST_DIFFER` pair — no longer subtracts from the
number of cases that passed, and two findings about one case no longer subtract
twice.

The `MUST_DIFFER` check moves out of `main` into `must_differ_findings`, which
takes the agreed answers and returns findings and touches no adapter.

`test_conformance.py` is new: 52 checks, in `check.sh` and in CI.

## Why

The old summary was `broken = sum(1 for line in failures if
line.startswith("FAIL"))` and `passed = len(CASES) - broken`. That is right
only while findings are one-to-one with cases, and they are not:

- A `MUST_DIFFER` pair is a relationship *between* two cases. Both can agree
  across all three SDKs — both pass — and the pair still fails, because what
  fails is that they agree with each *other*. All 130 cases passing with one
  such finding printed `129 passed, 1 failed`. There is no 129th case.
- Two findings about one case subtracted two from the case count.

Both numbers were wrong, in a runner whose entire job is to report a count
somebody reads instead of the 130 cases.

The deeper reason is the one the file's own comment already half-admits: *a
runner nobody can mutation-test is a runner whose own correctness is taken on
trust*. Everything it does is I/O against three adapters, so the one piece
that is not — the arithmetic — was unreachable without standing the whole
thing up, and went unexamined for exactly that reason.

## Alternatives rejected

**Fix the arithmetic and leave the structure.** A one-line change — count
distinct case names in the `FAIL` lines — and it would have worked by parsing
the runner's own output, which is the shape of fix that breaks the next time
somebody edits a message. The information was thrown away at the point the
finding was built; putting it back there is the fix.

**A `kind` enum on `Finding` rather than `case: str | None`.** More
expressive, and nothing needs the extra expressiveness: the only question the
tally asks is "which case, if any". `None` answers it, and answers it in the
type.

**Test the runner end to end instead.** It needs three adapters and a server,
so the test would live in `run.sh --conformance` and cost minutes, and it still
could not construct the case the defect lives in — 130 agreeing cases with one
failing pair — without a fixture engineered to disagree. Extracting the two
pure pieces makes the defect reachable in milliseconds.

**Leave `MUST_DIFFER` inline.** It is the first thing I tried, and the mutation
run rejected it: a mutation attributing a pair's finding to one of its cases
**survived**, because the suite tested `tally` and not what fed it. A test of a
pure function is worth what its inputs are worth.

## Evidence

`python3 examples/explorer/conformance/test_conformance.py`: **52 passed**.
`sh scripts/check.sh`: 35, up from 34. `scripts/test_check_sh.py` accounted for
the new CI step, as it is there to.

The defect, before and after, on the case that motivated it:

| findings | old | new |
| --- | --- | --- |
| none | 130 passed, 0 failed | 130 passed, 0 failed |
| one failing case | 129, 1 | 129, 1 |
| two findings, one case | **128, 2** | 129, 2 |
| one `MUST_DIFFER` pair | **129, 1** | 130, 1 |

Nine mutations via `scripts/mutate.py --dialect python`, all caught by named
tests:

| mutation | caught by |
| --- | --- |
| a `MUST_DIFFER` finding counted against a case again | `a must-differ finding costs no case at all` |
| two findings about one case cost two passes | `two findings about one case still cost one pass` |
| the failed count reports cases rather than findings | three cases, including the two above |
| a pair's finding attributed to one of its cases | `and is blamed on no case, so both of its cases still pass` |
| a not-comparable finding attributed to a case | `and that one is blamed on no case either` |
| a pair that agreed with itself is not reported | `a pair that agreed with itself is reported` |
| the not-comparable branch stops saying so | `a pair with one case missing is reported as not comparable` |
| `EXPECTED_REFUSALS` gains an entry naming no case | the roster check, by name |

**The fourth of those is why the extraction happened**, and it is worth the
line: it survived the first version of this suite. The arithmetic was tested,
was correct, and was being handed the wrong thing.

**One mutation could not be scored, and `mutate.py` said so rather than
guessing.** Removing the guard that skips a pair with a missing case
(`if quiet not in agreed_by_name or loud not in agreed_by_name`) produces code
that raises `KeyError` on the next line — the guard is load-bearing for
correctness and not only for reporting, so no mutation of it yields a running
program. The script reported `NOTHING RAN` rather than a survivor, which is the
second of the four lies its docstring lists, caught. The branch is covered
instead by mutating its *message*, which the suite catches.

**A real type error, caught by the check I ran afterwards.** Two assertions
were written `found and all(...)`, which is `list[Finding] | bool` rather than
`bool`, and `ty` refused the file. That is the root `ty` run doing its job on a
file nobody had type-checked before, and the reason `check.sh` runs all four
Python checks rather than the client's pair.

## What this does not do

**The runner's I/O is still untested.** `call`, `normalise`, the adapter
comparison and the refusal roster's *behaviour* all need three SDKs and a
server, and they still do. What is now testable is the part that turns results
into a verdict, which is the part that was wrong.

**Nothing checks the finding *messages*.** A finding could name the wrong case
in its text while carrying the right `case`, and every test here would pass.
The messages are read by people, not parsed, so this is a real gap and a small
one.

**`EXPECTED_REFUSALS` is checked for staleness and not for truth.** The suite
confirms each entry names a case that exists; whether that case *should* be
refused by all three SDKs is a judgement the runner makes at run time and
nobody re-examines.

**`tally` cannot see a case that never ran.** If the loop skipped a case
entirely — a `continue` in the wrong place — the count would report it as
passing, because passing is defined as "not in the findings". A case that
produces neither an agreement nor a finding is invisible, and nothing here
closes that.
