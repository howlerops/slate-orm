# A teardown that killed the parent and left the grandchild

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `examples/explorer/run.sh`
- **Kind:** fix

## What changed

`run.sh` kills each service's **process group** rather than the one pid it
recorded, escalates from `TERM` to `KILL` after a grace period, and turns job
control off before reaping so the shell does not announce each death on stderr.

## Why

The CI runner reported, after the demo job:

```
Terminate orphan process: pid (3524) (go)
Terminate orphan process: pid (3571) (node)
Terminate orphan process: pid (3806) (go)
```

That is GitHub cleaning up after a script that was supposed to clean up after
itself. A runner reclaims its own machine, so nothing broke — but the same
script run on a developer's laptop leaves a head node and three adapters
holding ports, and the next run fails with something unrelated-looking.

## The mechanism, which is not the one the code suggests

Each service starts as `( cd ... && command ) &` and `$!` is recorded. The
obvious reading is that `$!` is the *subshell*, and killing a subshell does not
kill its child — but bash exec-optimises the last command of a subshell, so
`$!` really is `go run` or `npm`.

The problem is one level further down. `go run` compiles and then runs the
built binary as a **child**; `npm start` spawns node as a **child**. Killing
the recorded pid kills the launcher and leaves what it launched.

Demonstrated rather than reasoned about, on a parent that spawns a child and
waits:

```
--- kill only the recorded pid (what run.sh did)
  orphaned 'sleep 300' still running: 1
--- with a process group
  orphaned 'sleep 300' still running: 0
```

`set -m` gives every background job its own process group whose id is the
leader's pid, so `kill -- -$pid` reaches the whole tree.

## Alternatives rejected

**`pkill -f` on the service command lines.** Would work and would also kill a
*second* demo stack somebody else is running, or an unrelated `go run` in
another checkout. A teardown that reaches outside its own children is worse
than one that misses some of them.

**Recording the grandchildren's pids.** Means asking `go run` and `npm` what
they spawned, which is racy (the child may not exist yet when the parent is
recorded) and different for each of the four services.

**`exec`ing the real binaries instead of launchers.** `go run` could be
`go build` then exec the binary, and `npm start` could be `node dist/main.js`.
It removes the nesting for two services and not for vite, and it means the demo
runs something different from what its README tells a reader to run.

**`timeout --kill-after` per service.** Bounds the run, not the teardown, and
the failure is at exit rather than during.

## Evidence

`./run.sh --conformance`: 34 cases, the three SDKs agree, **0 surviving
processes**. `./run.sh --e2e`: 17 passed, **0 surviving processes**. Counted by
listing every process and matching the four service command lines, rather than
by looking at a port.

The isolated probe above is what establishes the mechanism; the demo runs are
what establish the fix works in place.

`set +m` inside `cleanup` removes three lines of `[2] Terminated ( cd ... )`
from stderr on every successful run — noise that reads like a failure. The
groups still exist by then, so the kills are unaffected: verified by the
survivor count staying at 0 with the suppression in place.

## What this does not do

**I could not reproduce the leak locally.** Runs before the fix left 0
survivors here too; the orphans were only ever seen in CI, where the job runs
`--conformance` and then `--e2e` back to back on a busier machine. So the fix
is justified by the mechanism and the isolated probe, not by a local
before-and-after — and a run that leaked here would have been better evidence
than the one I have.

Nothing tests the teardown. There is no case that starts the stack, kills it
and asserts nothing survives, so a future edit could reintroduce this and only
a CI cleanup line would notice. That check is cheap and is not written.

The grace period is ten 0.2s polls, which is a guess at "long enough for four
services to close listeners". Nothing measured how long they actually take.
