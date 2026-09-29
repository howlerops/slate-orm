# A teardown test that has not been seen to pass

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `scripts/test_run_teardown.py` (new, unwired)
- **Kind:** test

## What changed

`scripts/test_run_teardown.py`: start the explorer's stack, wait for all four
services, record every descendant, `SIGTERM` `run.sh` itself, assert nothing
recorded survives. It is the case
`ledger/2026-09-14-the-demo-runner-left-processes-behind.md` asked for.

**It has never been observed passing, it is wired into nothing, and the caveat
it was written for stays open.** That is the entry.

## Why

> Nothing tests the teardown. There is no case that starts the stack, kills it
> and asserts nothing survives, so a future edit could reintroduce this and
> only a CI cleanup line would notice. That check is cheap and is not written.

Cheap to write, and the writing is not the cost. The container ran out of disk
while `run.sh` was still building — `df` reported 308 MB free with the Go and
Node adapters mid-compile — so the start never reached `adapters are up` inside
600 seconds, and the run was killed before it reached a single assertion.

The design decision worth keeping is in the file: **the descendants are
recorded before the kill.** Asking afterwards which processes are gone proves
nothing, because a survivor is reparented to init and is no longer a descendant
of anything the test can walk. That is the whole reason the original leak was
invisible except in CI's own cleanup line.

## Alternatives rejected

**Commit it wired into CI anyway.** It would probably work there — CI has disk
and already pays for two `run.sh` invocations. "Probably" is the objection.
`CLAUDE.md`: *a check that never fires is a check nobody has debugged*, and a
check nobody has ever seen fire **or fail** is worse than that. Turning a
twenty-one-job workflow red at 3am on an unrun script is how a check gets
switched off rather than fixed.

**Delete it and note the attempt.** The container is ephemeral, so that is
deletion. The file is a real artifact with a real design argument in it; the
next session should re-run it, not re-derive it.

**Shrink it to something that fits here.** A fake service tree that spawns a
grandchild, and a copy of `run.sh`'s `set -m` / `kill -- -$pid` shape to kill
it with. That tests the copy. The leak was `go run` compiling and then
`exec`ing, and `npm start` spawning node — a stand-in that spawns `sleep`
reproduces the shape and not the thing.

## Evidence

- `ruff check`, `ruff format --check`, `ty check` against the CI virtualenv:
  clean. That is all that is verified about this file.
- The run: `SLATE_SERVERD=… python3 scripts/test_run_teardown.py` reached the
  stack-start phase and was still there at 600 s, with `slate-serverd`, `go run`
  and `npm start` alive and the Python adapter not yet up. `df -h /` at that
  moment: **308 MB free, 100% used.** `python3 scripts/reclaim.py` then freed
  4.24 GB.
- **A hypothesis, labelled as one.** A manual `SIGTERM` to `run.sh` on that
  wedged container was followed by `run.sh exited` and all three services still
  alive. That is what the leak looks like. It is *not* reported as a finding:
  a `pkill -f "examples/explorer/run.sh"` had already delivered a `TERM`
  moments earlier, so the trap may have run once and been past by the time the
  second signal arrived; the disk was full; and the adapters may have been
  mid-compile, which is the one state `run.sh`'s comment says survives a `TERM`.
  Three confounders and one observation. Reproducing it on a healthy machine is
  the next step.

## What this does not do

**It closes nothing.** The caveat stays open, with the same words it had. What
changed is that the case exists and the next attempt starts from a file rather
than a blank page.

**No mutation test.** There is nothing to mutate against: a mutation is scored
by a named test failing, and this test has not been seen to pass, so a failure
would say nothing about the mutation.

**The `SIGTERM`-to-`run.sh` observation is unresolved and may be a real
regression.** If it reproduces cleanly, the 2026-09-14 fix has come undone and
that is more urgent than the test. If it does not, the confounders explain it.
I do not know which, and this entry is careful not to imply either.

**It is not in `check.sh` and not in `ci.yml`.** Deliberate, per the
alternatives above, and it means nothing runs it — which is the state the
caveat describes, one file further along.
