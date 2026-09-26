# `mutate.py`'s node dialect skipped the one line a test file that will not load ever says — and the skip never fired, because it was written for a filename this repository does not produce.

- **Date:** 2026-09-26
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/mutate.py`, `scripts/test_mutate.py`, `docs/caveat-status.json`
- **Kind:** fix

## What changed

The `node` dialect's failure pattern dropped its `(?!.*\.ts$)` exclusion. The
comment above it, which explained a behaviour node does not have, was replaced
by the run that was actually observed. `NODE_FAKE` grew a third branch for a
test file that throws while loading, and the case asserting the wrapper line is
**skipped** was replaced by one asserting it is **counted**.

## Why

Found while writing the caveat that closed it. The previous entry
(`ledger/2026-09-26-the-refusal-nothing-ran.md`) recorded, under **What this
does not do**, that the node corpus sample had no per-file wrapper line in it,
so the exclusion was defended only by a hand-written fake. Capturing one was
supposed to be a five-minute footnote. It turned into two findings.

**The premise was wrong.** The comment said node emits the wrapper *alongside
the real case*. On v22.22.2, with two files — one passing case, one failing —
it does not:

```
ok 1 - a named case
not ok 2 - a failing case
```

Add a third file that throws at load and the line appears, and it is the only
thing that file says:

```
not ok 3 - c.test.js
```

**So the exclusion was backwards.** A mutation that makes a test file fail to
load leaves `# fail 1` — which the *report* pattern matches, so the run is
scored rather than reported as "nothing ran" — and, with the only failure line
excluded, no failure name at all. `mutate.py` would call that a **survivor**:
the mutation is reported as untested code, and whoever ran it writes a test for
something that is already broken. That is the pytest `ERROR` hole
(`ledger/2026-09-20-the-fourth-way-a-mutation-lies.md`) in a different dialect,
put there on purpose by a comment nobody could check.

**And it never fired.** `clients/typescript/package.json` runs `node --test
dist-test/test/*.test.js`. The wrapper, when it appears, ends in `.js`. The
exclusion tested for `.ts`. A guard that was both wrong and dead, which is why
neither the guard nor its fake ever disagreed with anything.

## Alternatives rejected

**Keep the exclusion and correct it to `.js`.** This is what a careful reading
of the old comment suggests, and it would have made the bug *live*: every
load-failure would then be skipped and scored as a survivor, in the runner this
repository actually uses. The exclusion is not mis-spelled, it is the wrong
idea.

**Exclude a wrapper only when the run has other failures too.** Would preserve
the old comment's intent — don't attribute a catch to a filename — while
keeping a load failure visible. Rejected as a rule with no case behind it:
node does not emit a wrapper beside a real case, so the situation it guards
against was never observed, and a conditional exclusion is a second thing to
keep true against a runner whose output nobody here re-reads often. A filename
reported as the failing "test" is a *good* name for this failure: it is the
file that did not run.

**Leave it and note the discrepancy.** The caveat would have stayed open and
the hole would have stayed one `.ts` → `.js` away from being live. A dead guard
is not a safe guard; it is an untested one.

## Evidence

**Observed, on this container, node v22.22.2.** Three files under `node --test`
— `a.test.js` (one passing case), `b.test.js` (one failing case), `c.test.js`
(`throw new Error('boom at load')`):

```
ok 1 - a named case
not ok 2 - a failing case
not ok 3 - c.test.js
ok 4 - a named case
ok 5 - another case
# pass 3
# fail 2
```

The two-file run, without `c.test.js`, produced no line ending in a filename at
all. Both runs are in the session log; only the shape is recorded here.

**Mutation.** `ledger/mutations/20260926T232202-scripts-mutate-py.json`: the
exclusion put back, spelled `.js` so that it would actually fire —

| mutation | outcome |
| --- | --- |
| `^not ok \d+ - (.+?)$` → `^not ok \d+ - (?!.*\.js$)(.+?)$` | caught, by `a test file that will not load is counted, not skipped` |

One case, one catch. The old test asserted the reverse and would have passed
against the mutation, which is what makes this a replacement rather than an
addition.

**The other two dialects, re-examined.** The question each time is the same:
is there an input where the *report* pattern matches — so the run is scored —
while the *failure* pattern finds nothing, although nothing really ran?

- `pytest`, collection error: prints `ERROR test_bad.py` in the short summary
  and `1 error in 0.10s`. The failure pattern matches `ERROR`, so it is a catch.
  Safe. On an empty directory it prints `no tests ran in 0.00s`, which the
  report pattern (`^\d+ (?:passed|failed|error)`) does not match, so the run is
  refused as NOTHING RAN. Safe. Both observed.
- `python`, the house style: the summary is the last line a runner prints, so a
  runner that dies part-way through prints none. Observed with a two-line script
  that prints one `ok` line and then raises: output is `ok    a thing`, exit 1,
  no `N passed, M failed`. NOTHING RAN. Safe.
- `rust` was already covered by the existing `WILL_NOT_BUILD` case, and for a
  structural reason: `cargo` builds every target before running any test, so a
  crate that does not compile means no `test result:` line at all.

`scripts/test_mutate.py`: 70 passed, 0 failed. `sh scripts/check.sh`: 65 of 65.

**And the seventh invented citation in three days.** That record's timestamp was
first written as `…T232711`, from nothing — the run is `…T232202`. Caught by
reading the directory before `scripts/check_mutation_claims.py` had to. Six of
the seven were caught by a guard; this one was caught by the habit the guards
taught. The generating mistake is always the same shape: a filename or a
timestamp that *looks* like the real one, typed rather than read.

## What this does not do

**The TypeScript suite has not been re-run.** The dialect changed, not the
client, and nothing in `clients/typescript` reads `mutate.py` — but the claim
that the wrapper ends in `.js` is read off `package.json` rather than from a
run of that suite, because it needs a built `slate-serverd` and this container's
disk. The alternative was a release build for one string; the string is quoted
above and is one line of a file in this repository.

**No corpus sample carries the wrapper.** `CORPUS` in `scripts/test_mutate.py`
holds a clean and a failing node run, and the load-failure shape lives in
`NODE_FAKE` instead. Deliberately: `CORPUS`'s contract is one clean and one
failing run per dialect, checked as a cross product, and a third shape per
dialect would make that table's rules per-dialect. The fake is where
node-specific mechanics already live.

**The audit of the other two dialects is by observation, not by a test.**
Three of five dialects have now had this bug — `go`'s `[build failed]`,
pytest's `ERROR`, and this — so the remaining two were re-examined, and both
are safe for the reason recorded in **Evidence** rather than because something
in the suite would notice if they stopped being. Writing that check means a
"broken input" fixture per dialect, which is a sixth table beside `CORPUS`,
`DIALECTS` and three fakes; the observations are cheap to redo and the risk
they cover is a runner changing what it prints when it cannot run, which is
rarer than a runner changing what it prints when it can.
