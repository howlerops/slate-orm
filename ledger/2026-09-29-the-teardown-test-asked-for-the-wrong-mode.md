# The teardown test could not have passed: it waited for a line only one of the runner's four modes prints

- **Date:** 2026-09-29
- **Author:** Claude Code, task K2
- **Touches:** `scripts/test_run_teardown.py`, `.github/workflows/ci.yml`,
  `scripts/test_check_sh.py`
- **Kind:** test

## What changed

`scripts/test_run_teardown.py` **passes**, for the first time since it was
written, and is wired into CI's `demo` job. Six defects, all in the test and
none in `run.sh`:

1. it started `run.sh` in the plain mode and waited for `adapters are up`,
   which **only `--headless` prints**;
2. `START_LIMIT` bounded nothing, because `readline()` blocks and the deadline
   was only consulted between lines;
3. **its own cleanup leaked the thing it tests** — `killpg` on the runner's
   group, while `set -m` puts each service in *its own* group;
4. a failed start printed `it exited 1` and nothing else, never the
   per-service logs where the cause lives;
5. no `N passed, M failed` line, so `scripts/mutate.py` could not tell a
   failure from a suite that never ran;
6. no `FAIL  <name>` line, so a mutation it *did* catch scored `UNREADABLE`.

## Why

`ledger/2026-09-29-a-teardown-test-that-has-not-been-seen-to-pass.md` is
unusually honest about its own state — *"It has never been observed passing, it
is wired into nothing, and the caveat it was written for stays open. That is
the entry."* — and attributes it to the container running out of disk. That was
true of the run it describes and was not the whole story.

**Defect 1 is the wall behind the disk.** `adapters are up` appears once in
`run.sh`, inside `if [ "$mode" = --headless ]`. Plain `run.sh` falls through to
running the frontend in the foreground and never says the words. The test
invoked `["bash", RUNNER]` and waited for the headless line, so it could not
have passed on any machine with any amount of disk.

**Defect 3 is the one worth the entry.** Five of the first six starts died on
`address already in use`, each poisoned by the one before, and I spent an hour
hunting phantom processes with `pkill` patterns that matched nothing. The
source was this file: on any failure the `finally` did
`killpg(getpgid(runner), SIGKILL)`, and `run.sh` sets `-m` precisely so that
**each service is its own process group** — so the kill reaped the runner and
left four listeners. The test written to catch a runner leaking its children
was leaking them, by the same mechanism, in its own cleanup. It now sends
`SIGTERM` to the runner first, which runs the trap that is the thing under
test, and keeps the group kill as a backstop.

**Defect 2 and the fix that caused defect 5-and-a-half.** The wait loop was
`while time.monotonic() < deadline: readline()`, and `readline()` blocks, so a
quiet runner was waited on for ever — observed at **fifteen minutes past a six
hundred second limit**. Replacing it with `selectors` introduced a fresh bug I
then had to find: `select` watches the file descriptor while `TextIOWrapper`
buffers above it, so once a chunk was slurped the descriptor read as empty and
the loop slept **with the awaited line already in Python's buffer**. The banner
and `adapters are up` arrive in one burst, so a start that had succeeded
reported as a timeout, twice, before I read my own change properly. The pipe is
now bytes at `bufsize=0`, split into lines by hand.

Defects 4, 5 and 6 are the same shape at three levels: a failure that does not
say what failed. The test now prints the runner's last twenty lines *and* the
tail of every log in the run directory, a `N passed, M failed` summary, and a
`FAIL  <name>` line — and, because a held port is the failure it kept meeting,
it refuses to start when one of the four addresses already has a listener.

## Alternatives rejected

**Adding `adapters are up` to the plain mode.** One line in `run.sh`, fixing
the symptom in the wrong file. The plain mode ends by running the vite dev
server in the foreground; what this test wants is the mode that starts four
services and waits, which exists. Changing a runner to suit a test that was
asking for the wrong thing is how a runner accumulates modes nobody uses.

**Polling the four ports instead of reading stdout.** Sidesteps defect 1
entirely and answers a different question: a port accepting connections says a
service is up, and this test must know that **the runner believes** every
service is up, because that is the moment its process tree is complete.
Recording a partial tree and then asserting it was reaped is what would make
this test vacuous.

**A reader thread and a queue**, rather than `selectors` over a raw pipe.
Equivalent and heavier — a thread outliving a killed subprocess needs its own
shutdown — and it would have hidden defect 5-and-a-half rather than teaching
it.

**Leaving the port pre-flight out now that the log tails explain the failure.**
The tails do explain it. The pre-flight is worth its five lines anyway because
it changes what the failure *means*: "the runner never started" invites you to
debug the runner, and "port 7431 already has a listener" tells you it is your
own last run. Six starts is the measurement that made that worth writing.

**Keeping it out of CI until it had been seen to fail.** That was the previous
entry's position and it was right at the time. It is discharged: the mutation
below is the test failing, on purpose, for the reason it exists.

## Evidence

- **It passes, repeatedly.** `SLATE_SERVERD=…/target/release/slate-serverd
  python3 scripts/test_run_teardown.py`:

  ```
        7 processes under the runner while it serves
  ok    the runner reaps everything it started, on SIGTERM  (7 processes)

  1 passed, 0 failed
  ```

  Seven, not four: the head node, three adapters, and the shells `npm start`
  and `python3 -m adapter` are wrapped in. The grandchildren are the point.
- **Defect 1 is structural.** `grep -n "adapters are up"
  examples/explorer/run.sh` returns one line, guarded by
  `if [ "$mode" = --headless ]`.
- **Defect 2, observed:** a run started 13:53 was still in `readline()` at
  14:09, against `START_LIMIT = 600.0`, with all four services listening and
  `run.sh` in its own wait loop for a Go adapter that had failed to bind.
- **Defect 3, observed twice:** after a failed run, `held()` reported
  `['7431 (the Go adapter)', '7432 (the Node adapter)']`, and
  `lsof -tnP -iTCP:7431 -sTCP:LISTEN` named a `go` process whose parent had
  been reaped. Four rounds of `pkill -f` had matched none of them.
- **Three mutations against the real `run.sh`**, and the first two are
  findings rather than ceremony:
  - `ledger/mutations/20260929T152152-examples-explorer-run-sh.json` —
    replacing the group `TERM` with a `TERM` to the recorded pid **survived**.
    Not a missing test: `cleanup` sends a group `TERM` *and then* a group
    `KILL`, so breaking one leaves the other reaping the tree. That is real
    redundancy, and a single-point mutation cannot express the leak.
  - `ledger/mutations/20260929T152241-examples-explorer-run-sh.json` —
    `set -m` → `set +m`, the point both kills depend on, scored
    **`UNREADABLE`**: the test failed correctly and named no failing test, so
    the run could say nothing. That is defect 6, found by the tooling rather
    than by reading.
  - `ledger/mutations/20260929T152404-examples-explorer-run-sh.json` — the
    same mutation after the fix: **caught**, by `the runner reaps everything it
    started, on SIGTERM`. It is a change: without job control `$!` is not a
    group leader, so `kill -- -$pid` has no group to reach.
- `python3 scripts/test_check_sh.py`: 6 passed, 0 failed, **131** steps (was
  130) — the new CI step is accounted for, with a reason.

## What this does not do

**The two group kills in `run.sh` are not independently tested.** The surviving
mutation above says so precisely: either one alone reaps the tree on this
machine, so nothing here fails if one is deleted. Testing them apart means
mutating both at once, which `mutate.py` expresses as one anchor and these two
lines are forty apart. Recorded rather than worked around.

**A mutation that breaks the teardown cannot verify its own restore.** The
`set +m` run left listeners by construction, so `mutate.py`'s restore check ran
into the port pre-flight and reported `the tree did not come back clean`. The
*tree* was clean — `git diff` on `run.sh` was empty — and the machine was not.
That is the pre-flight working and the restore check reading it as a source
change; nothing distinguishes them today.

**The port pre-flight names four addresses and `run.sh` binds five.** The web
port is left out because `--headless` does not start the frontend, so this test
cannot collide on 7440. A reader comparing the list to `run.sh` will notice the
difference before the code does.

**`RUN_DIR` duplicates an expression from `run.sh`.** The test derives
`$TMPDIR/slate-explorer-<head port>` itself, so a change to that convention
makes the log tails silently vanish — which is defect 4 again, one layer down.
The better fix is `run.sh` printing its run directory in `--headless` too; it
prints `  logs       $run` in the plain mode's banner and not there.

**It has never run in CI.** Everything above is this container. CI has a
different kernel, a different Go, and process groups that are expected to
behave identically — "expected" being the word, until the `demo` job runs it.
