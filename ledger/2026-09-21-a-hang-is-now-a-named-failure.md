# An example that never returns is killed and named rather than waited on, and the five that had no size knob have one.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #276 (F5v)
- **Touches:** `scripts/{run_examples.sh,test_run_examples.py}`, `crates/slate-kernel/examples/{correlation,flatten_cost,perf_report}.rs`, `docs/performance.md`
- **Kind:** guard, and three knobs

## What changed

`scripts/run_examples.sh` runs every example under `timeout`. The budget is
`EXAMPLE_SECONDS`, fifteen minutes by default and two under `--smoke`, and a
killed example is reported as `FAIL  <name>, still running after Ns` with a
line saying what to do if it never returns by design.

`slate-kernel`'s three measurable examples read `KERNELBENCH_ROWS`, which
`--smoke` sets to 500. `scripts/test_run_examples.py` goes from 9 cases to 15.

## Why

Two gaps named in #274's entry, and they are the same gap from two directions.

**The hang.** `check_examples_roster.py` requires a handshake line for any
example whose source matches `future::pending` or `signal::ctrl_c`. That is a
warning about the two spellings anybody has used so far, and #274's entry said
plainly that a third — a `recv()` nothing sends to, a joined thread that never
exits, a retry loop with no ceiling — would slip past it and hang the loop. In
CI a hang is the job's own six-hour limit, with no line saying which example
caused it. A budget turns that into a named failure in two minutes, and makes
the roster check an early warning rather than the only protection.

**The knobs.** `--smoke` exists so CI can prove every benchmark still runs
without paying for the measurement. Five of the eighteen read no knob, so they
ran at recorded size: `slate-kernel --smoke` took **31 seconds**, all of it
three examples doing real work. It is **2.7 seconds** now. That is not the
point by itself — thirty seconds is affordable — but an example whose smoke run
*is* its recorded run makes the recorded size a thing CI has an opinion about,
and the next benchmark added at a serious size would be the one that hurts.

## Alternatives rejected

**Teach `FOREVER` a third spelling.** Chasing a list of ways to block is the
losing half of this. The regex stays, as the thing that says *"add a handshake
line"* before the run rather than after it; the budget is what makes a miss
survivable.

**A wall-clock threshold per example.** `run_examples.sh` already refuses this
in as many words — a duration assertion on a shared runner is a flake waiting
for a slow morning — and the budget is deliberately not one: fifteen minutes
against a slowest recorded run of minutes, two against a smoke run of seconds.
An example that trips it is stuck, not slow.

**A knob for all five.** `concurrency_probe` and `multi_tenant` finish in under
a second at recorded size. A knob whose only effect is to make a fast thing
faster is a second thing to keep true, and the timeout covers them if that ever
stops being so.

**One knob name per crate is three names in the export list.** It is, and the
alternative was renaming `HEADBENCH_*` and `SCALE_ROWS` to one name and chasing
every doc that quotes them. Two new cases instead assert that a `--smoke` run
sets all three and a plain run sets none, so a fourth crate's knob declared and
not exported fails rather than running big and green.

## Evidence

`sh scripts/check.sh`: **40 passed, all of them.**
`cargo clippy --workspace --all-targets`: clean.
`python3 scripts/test_run_examples.py`: **15 passed, 0 failed** (9 before).

`sh scripts/run_examples.sh slate-kernel --smoke`, before and after the knobs:

| example | recorded size | `--smoke` |
|---|---:|---:|
| `concurrency_probe` | 0 s | 0 s |
| `correlation` | 9 s | 1 s |
| `flatten_cost` | 16 s | 0 s |
| `perf_report` | 6 s | 1 s |
| **total, wall clock** | **31 s** | **2.7 s** |

Both sizes print which they ran at, so a pasted line cannot be mistaken for the
other: *"Correlated-column estimates, 20000 rows"* against *"…, 500 rows"*, and
*"over 60000 rows: min 1368.6 ms"* against *"over 500 rows: min 8.7 ms"*.

**A label the knob made false, found by running it.** `perf_report`'s cases
were labelled `whole tenant (2500 rows)` and `indexed equality (~100 rows)` —
true while the size was a `const`, and a lie the moment it could change: a
smoke run printed `whole tenant (2500 rows)` beside a `rows` column reading
500. Both labels are computed now, and the recorded run still prints 2500 and
~100.

**Mutations via `scripts/mutate.py`, eight**, over `run_examples.sh`: the
timeout removed, a timeout reported as an ordinary exit code, the remedy line
dropped, a killed example not counted, the budget hard-coded past the
environment, the exit status read after the timeout test rather than before,
the kernel's knob left out of the export list, and the knob set to the recorded
size.

That sixth one is a real bug this change made and then caught. The first
version read `$?` in an `elif` test and again in the branch below it, so every
*non*-timeout failure reported `exit 1` instead of its own code. `code` is
captured once now, and a case pins a fixture exiting 7.

**A killed mutation run left the tree mutated, and #272's marker put it back.**
Editing the test file while a run was in flight was my mistake; stopping it left
`run_examples.sh` carrying `EXAMPLE_SECONDS=900`, and the next `mutate.py`
invocation said so by name — *"was left mutated by a killed run ('the budget is
hard-coded rather than read from the environment'); restored"* — rather than
letting three cases fail against code nobody meant to be there. That is the
first time that recovery has fired on something other than its own test.

`STUCK`, the fixture that never returns, sleeps 20 seconds rather than 600.
Both catch a removed budget; at 600 the wait is the suite's own 120-second
outer timeout, three cases over, and a mutation run took more than ten minutes.

## What this does not do

**It does not measure anything.** No benchmark's numbers change; three of them
can be asked for fewer rows. The figures in `docs/performance.md` come from a
run with the knob unset, and that page now says so and says a smoke number is
not comparable.

**The budget is two numbers nothing validates**, which is the shape #274 was
about. 900 and 120 are judgements against a slowest recorded run of minutes.
A benchmark added at an hour would trip the first one and the failure would
read as a hang; the remedy line does not mention raising it.

**It does not bound a server example.** Those take the handshake path, which
has always had its own budget. An example that binds, prints its line, and then
wedges is killed by the runner as usual and nothing notices it wedged.

**`--smoke` at 500 rows proves execution, not correctness.** Same caveat the
runner has always carried. `correlation`'s whole subject is how estimates
degrade with row count, and at 500 rows its table is not a finding about
anything.
