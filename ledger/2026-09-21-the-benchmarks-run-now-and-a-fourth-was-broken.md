# Running the benchmarks found a fourth one broken, by a different security fix than the first three.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #265 (F5k)
- **Touches:** `crates/slate-headbench/{run.sh,src/fixture.rs,tests/examples_reach_the_server.rs,examples/{cache_probe,s3_nodelay}.rs}`, `.github/workflows/ci.yml`, `scripts/test_check_sh.py`, `docs/performance.md`
- **Kind:** fix + guard

## What changed

`crates/slate-headbench/run.sh` runs every benchmark in the crate and fails if
any of them stops working. `--smoke` asks each for the smallest fixture it will
accept; a new `headbench` CI job runs that on every push. The list of
benchmarks is read off disk, so a sixth is covered by existing.

The first run found `head_report` panicking on `PERMISSION_DENIED` at its
explain call. `fixture::security()` granted `Action::ALL`, which is the four
*data* actions and deliberately excludes `Explain` — so the role could not run
the thing the benchmark times. It now grants `Action::EVERYTHING`.

`cache_probe` and `s3_nodelay` hard-coded their 20,000-row fixture while the
other three took `HEADBENCH_ROWS`. Both now take it, default unchanged.

## Why

This crate's five benchmarks were built by `cargo clippy --all-targets` and run
by nothing. Three had been broken since `leadership` gained authentication;
they were fixed earlier this session, and that entry's own caveat said the two
tests added alongside could not see "an example that authenticates *wrongly* —
a bad principal, a role with no grant". A fourth was broken exactly that way,
by `EXPLAIN` becoming privileged, and both tests passed over it.

So the answer is not a better source check. Two different security fixes to
`slate-server`, months apart, each broke benchmarks nobody would think to run
afterwards. That is the case for a machine running them rather than a person
remembering to, and it is the same argument `ci.yml` already makes beside the
`batchbench` job in the words "a benchmark nobody runs stops building, and then
its numbers are quietly from whatever the code looked like last time somebody
tried".

Nothing asserts a duration. A wall-clock threshold on a shared runner is a
flake waiting for a slow morning; the exit code is the whole check.

## Alternatives rejected

**Widen the source check instead.** It is what the earlier fix did and it is
why this one was needed: a regex over the examples sees the shape it was
written for. `EXPLAIN` becoming privileged changed no line in any benchmark —
the break is in the *interaction* between a grant in one crate and a call in
another, and there is no text to match on. Running it is the only thing that
sees that class, and the class has now happened twice.

**Assert the numbers, so CI catches a regression too.** Rejected for the reason
`batchbench` records: the recorded figures come from a quiet machine and say
so, and a threshold on a GitHub runner would be red on a busy morning and
ignored by the third time. A benchmark that cries wolf gets its job deleted,
and then the exit-code check goes with it.

**List the five benchmarks in the script.** A roster is right where the entries
need *reasons* — that is what `check_handlers.py` does. Here there is nothing
to justify: every benchmark should run. A list would be one more thing to
forget, and forgetting is the failure this script exists because of. `ls` plus
a floor on the count is the same guarantee with nothing to maintain.

**Run them from `cargo test` instead of a script.** `cargo test` does not run
examples, so it would mean a test that shells out to five binaries — the same
script, wearing a test's clothes, and inside a harness that captures output so
a benchmark's report goes nowhere. It would also land in `cargo test
--workspace`, which `CLAUDE.md` says does not fit on this disk.

**Leave the two hard-coded row counts alone.** They are what made a smoke pass
slow: `s3_nodelay` took 122 s and `cache_probe` 11 s at 20,000 rows, against
5 s and 1 s at 500. A CI job that takes four minutes longer than it needs to is
a job someone eventually makes conditional.

## Evidence

**The defect, and that the runner catches it.** `scripts/mutate.py --dialect
python` over `fixture.rs`, reverting the grant to `Action::ALL`:

```
ok   the benchmark role loses the explain grant again  ->  head_report, exit 101
```

That is the live bug reproduced on demand. Before the fix, `head_report` ran
23 of its 207 output lines and aborted; after it, all five benchmarks complete:

```
ok    cache_probe, 1s          ok    s3_nodelay, 5s
ok    head_concurrency, 71s    ok    stream_step, 17s
ok    head_report, 59s
5 passed, 0 failed
```

153 s of running plus about 78 s of building, measured here across three full
passes with the spread in the seconds. The row knob is most of why: 122 s → 5 s
for `s3_nodelay`, 11 s → 1 s for `cache_probe`.

**The loop reports every failure, not the first.** Mutating the invocation to
`if false`:

```
ok   a benchmark that fails is reported by name
     ->  cache_probe, exit 1, head_concurrency, exit 1, head_report, exit 1, s3_nodelay, exit 1
```

Four names because `mutate.py` prints only the first four; all five failed.

**`mutate.py` cannot score the never-fires guard, and the reason is worth
writing down.** A fired guard means *no benchmark ran*, and "no suite reported"
is precisely what the script treats as a hard error rather than a result — the
second of the four lies its docstring lists. So that half was demonstrated
directly, on the real script, with the glob narrowed to three of the five:

```
=== three of five, guard present:
found 3 benchmarks under .../crates/slate-headbench/examples; expected at least 5.
exit=1
=== three of five, guard deleted:
ok    cache_probe, 1s
ok    head_concurrency, 70s
ok    head_report, 59s
3 passed, 0 failed
exit=0
```

A green check over a tree with two benchmarks missing from it, which is what
the guard is for.

**My first comment on that guard was wrong and running it said so.** It claimed
the guard catches a directory that exists and holds no `.rs`. It does not:
`ls` fails on the empty glob and `set -e` aborts, with or without the guard —
verified both ways. What the guard actually catches is the case `set -e` misses,
a directory with *some* benchmarks, which is the demonstration above. The
comment now says that.

`scripts/test_check_sh.py` refused the new CI step until it was accounted for,
which is that guard doing its job on the commit that adds a job:

```
FAIL  every ci.yml step is in check.sh or in ELSEWHERE
      not in check.sh and not in ELSEWHERE: (in .) crates/slate-headbench/run.sh --smoke
```

`sh scripts/check.sh`: 34 passed. `cargo clippy --workspace --all-targets`:
clean. `cargo test -p slate-headbench`: 2 passed.

## What this does not do

**Smoke numbers are meaningless and the script says so, but nothing enforces
it.** At 500 rows `cache_probe` cannot show a cache effect and `s3_nodelay`
cannot tell two sockets apart. Nothing stops somebody reading a `--smoke`
figure into `docs/performance.md`; the only defence is that the header each
benchmark prints carries the row count it actually used.

**The exit code is the whole assertion.** A benchmark that runs to completion
while measuring the wrong thing — a section silently skipped, a fixture that
loaded no rows — passes. The `assert_eq!(counted, rows())` lines inside two of
them are the only correctness check in the set, and they were already there.

**A nonsense section argument runs nothing and exits 0.** `head_report --
typo` matches no section, prints its header and stops. `--smoke` passes no
section so it is not reachable from CI, but it is a hole in the same family as
everything above and it is not closed here.

**Release mode is untested by this.** The job builds debug, because that is what
is quick; `docs/performance.md`'s numbers are all `--release`. A benchmark that
compiles and runs in debug and fails under release optimisation would pass.

**Four of five is what was broken, and the fifth is unproven history.**
`stream_step` and `head_concurrency` were repaired earlier by inspection and
not run until now; `cache_probe` and `s3_nodelay` were never broken as far as
anyone knows, but nothing ran them before today either, so "never broken" is a
statement about the absence of evidence.

**Nothing checks the CI job actually runs.** It is a new job in `ci.yml` and
this repository's own history includes a workflow that was marked active and
had run zero times. The first push will say; until it does, the job is exactly
the kind of thing this entry is about.
