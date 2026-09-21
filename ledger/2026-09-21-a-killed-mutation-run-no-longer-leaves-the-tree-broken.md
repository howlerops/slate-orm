# A mutation run killed mid-flight left the tree mutated, and that is a fifth way it can lie.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #272 (F5r)
- **Touches:** `scripts/{mutate.py,test_mutate.py}`, `.gitignore`
- **Kind:** fix

## What changed

`mutate.py` writes `.mutate-in-flight.json` before it touches a file and
removes it after the restore, recording the path and both versions. The next
run reads it and does one of three things: restores the file when it still
holds exactly what was written, drops the marker when the file is already back
or has vanished, and refuses with instructions when it holds something else.

The docstring's "four ways a mutation run lies" is five. `test_mutate.py` is
22 cases, from 18.

## Why

The restore lives in a `finally`, which does not run through a `SIGKILL` — and
a mutation run over a suite that takes minutes is exactly what a timeout kills.
One did, this session: `scripts/run_examples.sh` was left carrying a mutated
roster entry, the next test run failed against it, and the failure read as a
broken test rather than a dirty tree. Ten minutes went into the wrong file.

That is the script's own first lie with a longer fuse — a suite running against
code nobody meant to be there — and it is the one failure mode the tool causes
rather than catches. #271's entry named it; this closes it.

## Alternatives rejected

**Restore whatever the marker says, always.** Simpler, and it would overwrite
an edit somebody made after the kill — trading one silent wrong state for
another. The three-way answer costs fifteen lines and never guesses.

**A marker beside each subject rather than one for the repository.** No
self-reference problem and a worse guard: the run that trips over a leftover
mutation is usually a run of something *else*, which is precisely how this went
unnoticed. One marker at the root is seen by the next run of anything.

**Trap `SIGTERM` and restore there.** It covers the polite kill and not
`SIGKILL`, which is what a timeout sends when the first one is ignored, and it
does nothing for a power cut or an OOM. A marker on disk covers all of them and
needs no signal handling.

**`git checkout` the file on recovery.** Wrong for a subject with uncommitted
work, which is the normal state while writing the test a mutation asked for.

## Evidence

Three states, each a case, each mutation-tested:

| the marker says, and the file holds | what happens |
| --- | --- |
| mutated text, exactly as written | restored, and said so |
| the original already | marker dropped |
| something else | refused, naming the file and the case |
| a path that no longer exists | marker dropped |

Six mutations via `scripts/mutate.py --dialect python`, all caught: recovery
never attempted, the restore writing the *mutated* text back, an edited file
overwritten rather than refused, the refusal turned into a shrug, the marker
left behind after a restore, and a vanished file treated as a leftover.

**One survivor, recorded rather than fixed.** Removing the `MARKER.unlink` on
the vanished-file path survives, because the run that follows writes the marker
again and removes it on the way out. It is redundant in every path a test can
reach and covers the one it cannot — a spec naming a file that does not exist,
which raises in `apply_once` before anything is written. `expect_survivor`
carries the reason, which is what that field is for.

**The self-reference had to be fixed before two of those could be caught.**
`test_mutate.py` drives `mutate.py` as a subprocess, and a mutation run over
`mutate.py` therefore has a parent and a child sharing one marker at the
repository root — each recovering the other's state. Two mutations of the
restore path read as survivors while running the suite by hand caught them
twice. `MUTATE_MARKER` gives each run its own, and only the tests set it.

`python3 scripts/test_mutate.py`: **22 passed**. `sh scripts/check.sh`: 38, and
`ty` at the root refused a `tuple[bool, bool, bool]` annotation on a function
returning `tuple[bool, ...]` — the third time this session the root Python
checks have caught something in a file nobody had type-checked before.

## What this does not do

**It does not recover a run killed between the write and the marker.** There is
no window: the marker is written first. What there is no answer for is a crash
*inside* `path.write_text` leaving a half-written file, which no marker
describes and which `git diff` shows anyway.

**It only notices on the next `mutate.py` run.** A leftover mutation sits there
until somebody runs the tool again — `git status` shows both the dirty file and
the marker in the meantime, which is more than before and less than a hook.

**Nothing cleans up a marker for a file in a directory that still exists but is
not yours.** The "refused" path is a dead end until a person reads it, which is
deliberate: the alternative is guessing.

**The three states are checked by writing the marker by hand**, not by killing
a real run. That is the input `recover` actually takes, and it keeps the suite
in the static-check run where it belongs; it does not prove that a real
`SIGKILL` leaves exactly this marker. The bug it fixes was observed, so the
shape is known rather than assumed.
