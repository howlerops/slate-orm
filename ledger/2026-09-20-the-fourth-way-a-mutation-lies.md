# A mutation scored against the previous mutation's bytecode

- **Date:** 2026-09-20
- **Author:** Claude, having pointed the mutation harness at its own new guard
- **Touches:** `scripts/mutate.py`, `scripts/test_mutate.py`
- **Kind:** fix

## What changed

`scripts/mutate.py` now gives every command run a fresh bytecode cache, and
learns a second output dialect so it can drive this repository's Python guards
and not only `cargo`. The docstring's list of ways a mutation run lies goes
from three to four.

## Why

The harness shipped this morning listing three lies, all met by hand in one
session. Pointing it at `scripts/check_cited_tests.py` — six mutations, the
first non-Rust subject it had ever had — found a fourth, in the harness itself.

CPython invalidates a `.pyc` on the source's **mtime and size**. A mutation
worth making is usually the same size as what it replaces; all six of mine
were:

```
'{4,}'     -> '{3,}'      same length
'not any(' -> 'not all('  same length
'return 1' -> 'return 0'  same length
```

Six runs inside one second, all same-size, so after the first recompile the
cache stopped invalidating. Two consequences, and the second is worse:

- **A mutation was scored against the previous one's bytecode.** The `not any`
  → `not all` case was reported as caught by *the four-word threshold test*,
  which is the test the `{4,}` → `{3,}` case before it breaks. A plausible
  result attributed to the wrong cause.
- **The restore was invisible.** `finally: path.write_text(original)` wrote the
  right bytes, the final verification ran the mutated bytecode anyway, and the
  harness announced "the tree did not come back clean" about a tree that was.

The first is the dangerous one. It does not look like a failure — it looks like
a pass, with a test name beside it, which is the whole class of thing this
script exists to stop. The only reason it was caught is that the attributed
test was one I could tell was wrong by reading it. A mutation set where the
names were less distinctive would have read as six clean results.

`cargo` hashes contents and was never exposed. So the bug could only exist once
the script was pointed at something other than what it was written for — which
is also the reason the dialect work and this fix are one change: the second
dialect is what made the defect reachable.

## Alternatives rejected

**`PYTHONDONTWRITEBYTECODE=1`.** Stops the *writing* and not the *reading*, so
a `.pyc` already on disk — and there is one, from the baseline run — is still
picked up. It would have fixed nothing here.

**Delete `__pycache__` before each run.** Works, and reaches outside the thing
being run: a mutation harness that walks the tree deleting files is one
`ROOT`-resolution bug away from deleting the wrong ones. `PYTHONPYCACHEPREFIX`
into a temporary directory redirects rather than removes, and the directory
goes away with its context manager.

**Set the prefix only for the `python` dialect.** The narrower change, and
rejected because the reasoning that makes `cargo` safe — it hashes contents —
is a fact about today's `cargo` that the next dialect's tool need not share.
The cost is one recompile per run on a script that already spawns a process.

**`touch` the file to a distinct mtime after restoring.** Fixes the invisible
restore and not the cross-mutation contamination, since the mutations
themselves are what land in the same second. Half a fix for the less serious
half.

**Leave it, and note it as a caveat.** What the previous entry's "what this
does not do" section would have grown. Against it: the entry shipped this
morning argues at length that writing up a recurring failure instead of
guarding it *was the defect*. Doing that again, in the same tool, on the same
day, would be difficult to defend.

## Evidence

The defect, as first observed — note the attribution on line 2 and the last
line:

```
  ok   the threshold drops to four words  ->  a four-word name is below the threshold and is ignored
  ok   the paragraph rule demands every name resolve, not any  ->  a four-word name is below the threshold and is ignored
  ...
  !! the tree did not come back clean: ['a doc naming a test that does not exist fails', ...]
```

`git` showed the subject file byte-identical to the original, and
`rm -rf scripts/__pycache__` made the suite pass again — which is the whole
diagnosis in one command.

After the fix, the same six mutations:

```
  ok   the threshold drops to four words  ->  a four-word name is below the threshold and is ignored
  ok   the paragraph rule demands every name resolve, not any  ->  a dead name beside a live one is history, and passes
  ...
restored: 1 suites reported, none failing
```

Each now attributed to the test that actually covers it.

**The new case fails without the fix.** `two same-size mutations are not scored
against each other's bytecode` builds a module, mutates `VALUE = 1` to `2` and
then to `3`, and requires both `saw_two` and `saw_three` in the output;
without a fresh cache the second reports `saw_two`. It also asserts every run
was handed a *distinct* cache directory, which is the deterministic half —
whether two writes land in the same mtime second is a race, and a test that
depends on winning it would pass for the wrong reason on a slow machine.

Confirmed by mutating `mutate.py` through `mutate.py`, 4 mutations, all caught:

```
ok  the per-run bytecode cache is removed  -> two same-size mutations are not scored against each other's bytecode
ok  the exact-once rule accepts any count  -> an anchor that matches nothing ..., an anchor that matches twice ...
ok  a zero-suite run is no longer a hard error -> a mutation that does not build is not mistaken for a survivor
ok  the restore is skipped                 -> a mutation the suite catches is reported with the test's name, ...
```

`scripts/test_mutate.py` 7 → 8 cases, all passing. `scripts/check.sh` 23/23.

**A dead end worth recording**, because it is the same class of bug one level
up: the first version of the new test case was a syntax error and never ran.
The runner is a Python program held in a `'''...'''` literal, and I wrote
`"\n"` inside it — which Python resolves when *`test_mutate.py`* is parsed,
turning the runner's source into an unterminated string. The file looked
correct; the value was broken. `CLAUDE.md` already says to re-read a scripted
edit's output, and this was mine.

## What this does not do

**It does not make the harness dialect-agnostic.** Two dialects, `rust` and
`python`, both hand-written regexes. `pytest`, `go test` and `npm test` all
report differently and none of them are here; each would report zero suites and
be refused, which is a loud failure rather than a wrong one, but it is not
support.

**The `python` dialect reads this repository's own house style**, `ok    name`
/ `FAIL  name` / `N passed, M failed` — not any general Python convention. A
script that prints something else is not drivable, and nothing enforces that
the guards keep printing it; if one drifts, its mutation runs report zero
suites.

**Nothing detects the general case of a stale cache.** The fix is
`PYTHONPYCACHEPREFIX`, which is specific to CPython. A command that caches
compiled output somewhere else keyed on mtime — a bundler, a typechecker's
incremental store — has the same hazard and is not handled. I did not look for
one.

**The contamination window was not measured.** I know six same-size mutations
inside one second contaminated each other, and I did not establish how far
apart two runs must be to be safe, because the fix makes the distance
irrelevant. Anyone reasoning about a mutation run made *before* this commit
should treat same-size mutations in one batch as unattributed.
