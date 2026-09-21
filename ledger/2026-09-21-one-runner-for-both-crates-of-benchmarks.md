# The storage examples run now too, and the runner that runs them has tests of its own.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #271 (F5q)
- **Touches:** `scripts/{run_examples.sh,test_run_examples.py,check.sh,test_check_sh.py}`, `crates/slate-headbench/run.sh`, `crates/slate-slatedb/run.sh`, `crates/slate-slatedb/examples/{ascending_walk,cost_calibration,scan_tuning}.rs`, `.github/workflows/ci.yml`, `docs/performance.md`
- **Kind:** guard

## What changed

`scripts/run_examples.sh` runs every example in a named crate and fails if any
stops working. Both `slate-headbench` and `slate-slatedb` delegate to it from a
four-line `run.sh`, and both have a CI job. `slate-slatedb`'s eight run at
smoke size in **21 seconds**.

`cost_calibration` and `scan_tuning` take `SCALE_ROWS`, which `cost_at_scale`
already did; `ascending_walk` moves from `HEADBENCH_ROWS` to the same name, one
per crate.

`scripts/test_run_examples.py` is new: nine cases over directories it writes,
because against the real crates — where everything passes — a mutation to the
floor, the counting or the handshake survives.

## Why

#265 gave `slate-headbench` a runner after four of its five benchmarks turned
out broken. `slate-slatedb`'s eight were in exactly that state, and they matter
more: `SCAN_ROW_COST` and `POINT_READ_COST` are calibrated *from* them, so a
benchmark nobody runs is a planner constant nobody re-measures. #269 found
`POINT_READ_COST` three times its measured value; #269 and #270 each found a
defect in an example while running one by hand.

**One script rather than a second copy.** The alternative differed in a
directory and five environment variables, which is the list-nobody-keeps-in-sync
these runners exist to argue against.

## Alternatives rejected

**Two runners, one per crate.** A hundred lines duplicated to vary six. The
thin `run.sh` wrappers keep the paths `docs/performance.md` already quotes.

**Skip `s3_server`.** It is a server: it prints `LISTENING <addr>` and serves
until killed, so the first smoke run hung on it. Skipping is what `CLAUDE.md`
names — **a skip is green** — and it is not necessary: the handshake exists
precisely so `examples/deployed/run.sh` can wait for it, so there is something
to assert. The runner waits for the line and kills the process, which proves as
much about the binary as running a benchmark to completion does.

**Rename `SCALE_ROWS` to `HEADBENCH_ROWS` everywhere.** One name would be
tidier and would mean chasing every doc that quotes the other. One per crate,
documented where its examples are, and the smoke block exports both.

**Test the runner only against the real crates.** What #265's runner did, and
`scripts/mutate.py` showed the cost: dropping the example floor, uncounting a
failure and disabling the handshake check **all survived**, because a green run
does not distinguish a runner that counts failures from one that ignores them.
`EXAMPLES_DIR`, `BINARIES_DIR` and `SKIP_BUILD` exist for that, and for nothing
else.

## Evidence

`crates/slate-slatedb/run.sh --smoke`, from a cold build:

```
ok    ascending_walk, 1s      ok    replicas, 0s
ok    bucket_layout, 9s       ok    row_footprint, 4s
ok    cost_at_scale, 1s       ok    s3_server, said LISTENING in 1s
ok    cost_calibration, 1s    ok    scan_tuning, 3s
8 passed, 0 failed
```

21 seconds. `crates/slate-headbench/run.sh --smoke` through the same script:
5 passed, unchanged from #265.

**The first smoke run found a defect in an example written the same day.**
`ascending_walk` marks its spread arms with `id % stride() == 7`; at 200,000
rows the stride is 500 and that is 400 rows, and at a smoke size the stride is
5, `id % 5 == 7` is true of nothing, and the arm returned zero rows while
asserting 400. It is `7 % stride()` now — identical at the recorded size, and
defined at every other. A constant that is only valid above some fixture size
is a fixture size nobody stated.

**Nine mutations of the runner, all caught**, once it had fixtures: the
handshake check made unconditional, the roster no longer naming the server, the
example floor dropped, a failure uncounted, a failure stopping the run, the
exit code ignoring the count, and the wait budget shrunk to nothing and grown
to unbounded.

**Two of those needed the fixtures fixed before they could be caught**, and
both are worth the lines:

- *The wait shrunk to nothing* survived, because every fixture server printed
  its handshake before the runner's first look. A server that takes a moment to
  bind — which is what a real one does — is the only thing that makes waiting
  observable.
- *The wait grown to unbounded* survived, because the mute fixture exited after
  60 seconds and the liveness check caught it regardless of the budget. It
  outlives the harness's own timeout now, so the budget is the only thing that
  ends the wait.

**Two defects in the fixtures themselves, both found by running them.** The
first servers looped `while true; do sleep 1; done`, so killing the shell left
an orphaned `sleep` behind every case; they `exec` now, so the process killed is
the one that sleeps. And `subprocess.run` had no timeout, so a runner that
stopped recognising the server left the suite — and any mutation run over it —
waiting forever rather than failing. A hang is a failure now.

**A mutation run that timed out left the tree mutated.** `scripts/mutate.py`
restores in a `finally`, which does not run when the process is killed from
outside, and the next test run failed confusingly against a roster entry
reading `s3_server-renamed`. Restored by hand; noted here because the script's
own docstring promises a restore and that promise has one hole in it.

`sh scripts/check.sh`: **38**, from 37. `scripts/test_check_sh.py` refused the
new CI steps until they were accounted for.

## What this does not do

**`--smoke` numbers are meaningless**, at 2,000 rows rather than 200,000. The
exit code is the whole check, as it is for `slate-headbench`.

**Nothing runs either crate at its recorded size.** The figures in
`docs/performance.md` are still taken by hand and still go stale between the
day they were measured and the day somebody checks — which is the whole subject
of #269 and #270 and is not closed by a smoke run.

**`mutate.py`'s restore has a hole.** A `finally` does not run through a
`SIGKILL`, so a mutation run killed from outside leaves the file changed. Not
fixed here, because the fix is a guard on the *next* run — a marker file, or a
`git status` check — and it belongs to that script's own task.

**The handshake roster is one entry and cannot be checked for staleness.**
`check_demo_surface.py` reports a roster entry naming something that no longer
exists; this one would need to know that `slate-slatedb/s3_server` is still an
example, which the floor already half-covers and nothing states directly.

**The floors are two numbers in a `case`.** Add a third crate and the script
refuses it by name, which is deliberate, and nothing tells you the number for
an existing crate is still the right one beyond the directory disagreeing.
