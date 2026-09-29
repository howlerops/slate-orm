# CI went red on a lint my own `check.sh` run had already reported, because I grepped its output

- **Date:** 2026-09-29
- **Author:** Claude Code
- **Touches:** `scripts/test_check_sh.py`, `CLAUDE.md`
- **Kind:** fix

## What changed

One line. `workflow_steps(TWO_FILES + [[...]])` becomes
`workflow_steps([*TWO_FILES, [...]])`, which is what `ruff`'s `RUF005` asks
for. Run 487 of CI failed on it, one job of twenty-two, and nothing else.

And two words in `CLAUDE.md`: **twenty-one jobs → twenty-two**, in both places
that say it. The run that failed reports `total_jobs: 22` and the count
`CLAUDE.md` prescribes for itself gives 22.

## Why

The interesting part is not the lint. It is that **`sh scripts/check.sh` had
already reported it, in the run immediately before the commit, and I did not
see it** — because I ran

```sh
sh scripts/check.sh 2>&1 | grep -iE "^FAIL|passed, all|[0-9]+ failed$"
```

to keep the output short, and the script's own summary line on failure is

```
84 passed, FAILED: python-rest-ruff
```

which matches none of those three alternatives. `passed, all` matches only the
*green* summary; `[0-9]+ failed$` matches a per-suite line and not this. So the
filter was, precisely, a filter that shows success and hides failure — the
shape `scripts/mutate.py`'s docstring calls out and the shape CLAUDE.md's "a
skip is green" note is about, applied by hand to a check that was working.

`check.sh` is documented to report at the end rather than stopping at the first
failure, and its whole value is that one line. Piping it through a grep written
from memory of what success looks like throws away the thing it exists to
produce.

## Alternatives rejected

**Changing `check.sh`'s failure summary to contain the word `failed` in a
greppable position.** Tempting and wrong twice over: it fixes this grep and not
the next one, and the script already communicates by exit code, which is what a
caller should read. The defect is in the caller.

**A `--quiet` flag on `check.sh` printing only the summary.** It has one: the
exit code, and `$?` is shorter than any flag. What went wrong here was a reader
discarding output, not a script producing too much.

**Blaming the toolchain gap.** CLAUDE.md records that CI's `ruff` is newer than
this container's and that a green local run is necessary and not sufficient —
`RUF036` went red exactly that way. It would be an easy and false story here:
the local `ruff 0.15.8` reported `RUF005` too, in the same words, at the same
line. This one was seen locally and discarded.

## Evidence

- CI run **487**, job *the Python that is not the client*, conclusion
  `failure`, one failed job of 22. Log:
  `RUF005 Consider [*TWO_FILES, [...]] instead of concatenation`,
  `scripts/test_check_sh.py:330:30`, `Found 1 error.`
- `ruff check .` in this container, after the fix: `All checks passed!`. Before
  it: the identical `RUF005` message, which is what makes the toolchain-gap
  explanation false rather than merely unlikely.
- `python3 scripts/test_check_sh.py`: 6 passed, 0 failed; 130 steps, 12 blocks
  and 2 env vars, all accounted for. The change is a spelling of the same list
  literal, so the fixture it feeds is unchanged and the run recorded as
  `ledger/mutations/20260929T134606-scripts-test-check-sh-py.json` still
  describes this file.
- **Nothing was mutated for this entry, deliberately.** `[*a, b]` and `a + [b]`
  evaluate to the same list, so a patch between them is the "not a change" case
  `scripts/mutate.py`'s survivor message names first, and running it would
  produce a survivor that means nothing.

## What this does not do

**It does not stop the next filtered summary.** The remedy is to read
`check.sh`'s last line or its exit code, which is a habit, and this entry is
the second place today to conclude that the honest fix for a habit is writing
it down. Nothing enforces it.

**Nothing checks the job count, and this is the second time it has drifted.**
`CLAUDE.md` says so itself — "a count of `jobs:` keys in `ci.yml` and nothing
checks it, which is why it said *seventeen* for as long as it did" — and it has
now said *twenty-one* through one more job landing. The count is a one-line
`re.findall` the file already prints, so a guard is cheap; what is not cheap is
deciding whether every number in `CLAUDE.md` should be checked, and this entry
fixes one sentence rather than building the rule.
