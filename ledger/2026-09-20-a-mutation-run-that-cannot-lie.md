# A mutation harness, because doing it by hand failed three ways in one day

- **Date:** 2026-09-20
- **Author:** Claude, after reporting the same process failure three times without fixing it
- **Touches:** `scripts/mutate.py` (new), `scripts/test_mutate.py` (new), `scripts/check.sh`, `.github/workflows/ci.yml`, `CLAUDE.md`
- **Kind:** process

## What changed

`scripts/mutate.py` applies a mutation, runs a command, and refuses to be
ambiguous about what happened. Seven tests for it that need no toolchain, so it
joins the static check run — 20 checks became 21.

## Why

`CLAUDE.md` asks for a mutation test on everything written here, and it is the
rule that has earned its keep: this session alone, seven mutations survived a
first attempt and every one was in code or a test I would otherwise have
reported as covered. But the *running* of them has been a `sed`, a `cargo test`
and a grep for `FAILED`, and that pipeline lies in three ways. All three
happened today.

**The anchor moves.** Three times a patch silently matched nothing, because
`cargo fmt` had rewrapped the call it anchored on. The suite then ran against
unmutated code and passed — which reads as *"the mutation survived"*, and a
survivor is supposed to mean "write a test". The lie does not just waste a run,
it argues for a test that should never exist. Each time, the only thing that
caught it was an `assert count == 1` I had happened to put in the throwaway
script.

**Nothing runs.** An ENOSPC mid-run killed the build, the grep found no
`FAILED`, and the output was indistinguishable from a clean pass. I reported
three mutation results from that run before noticing, and had to redo them.

**The shell eats it.** One replacement containing a bare `retired` arrived as
`retired: command not found`.

Each of those is a guard this repository would write for any other recurring
mistake — `EXPECTED_REFUSALS`, `MUST_DIFFER`, `test_check_sh.py`'s `ELSEWHERE`
are all the same instinct. Writing up the failure three times and not building
the guard was the actual defect.

**A survivor exits non-zero**, which is the design decision worth arguing for.
A survivor is a finding — a missing test, or redundant code — and it needs a
response. Left as a line of output among others it scrolls past; as a non-zero
exit it stops whoever ran it. `expect_survivor` is the escape hatch, and it
takes a *reason*, in the idiom this repository already uses: a list you are
forced to edit is a list that stays true. It also inverts — a recorded survivor
that starts being caught fails too, because the reason has expired.

## Alternatives rejected

**`cargo-mutants`.** The real tool, and it generates its own mutations, which
is the wrong shape for the discipline here. `CLAUDE.md` asks you to break *the
thing you just wrote* and name the test that catches it; that is an argument
about your own change, not a survey. It is also another toolchain install on a
container that cannot fit `cargo test --workspace`.

**A shell function in `check.sh`.** It is where the other guards live, and it
would carry the quoting bug that caused one of the three failures. Taking the
spec as JSON on stdin is precisely what makes a replacement containing a
backtick, a `$` or a bare word safe.

**Just remember to write `assert count == 1`.** What I did, three times, and it
worked three times — which is the argument *against* it: it worked because I
remembered, and the two failures that got through were the two where I did not
check a second thing (suites reported) or a third (the shell).

**Parse `--format json` from libtest instead of the human output.** More robust
and it is nightly-only for the fields that matter. The two regexes here are
matching `test X ... FAILED` and `test result:`, both of which have been stable
for a decade; if they ever change, `test_mutate.py` fails rather than the
harness going quietly blind — which is the property that matters.

**Let a survivor exit zero and print a warning.** Rejected on the evidence: the
three failures above were all *visible in the output* and I still acted on the
wrong reading. Exit codes are what stop a script; warnings are what a reader
skims.

## Evidence

**Seven tests, run against a fake `cargo`** — a few lines of Python that print
what a real run prints, chosen by what it finds in the file under mutation. That
makes the script's parsing the thing under test, which is where all three
failure modes live, and it means no build and no toolchain:

```
ok    a mutation the suite catches is reported with the test's name
ok    an anchor that matches nothing is refused before anything runs
ok    an anchor that matches twice is refused too
ok    a mutation that does not build is not mistaken for a survivor
ok    a surviving mutation fails loudly rather than scrolling past
ok    a survivor with a recorded reason is accepted
ok    a recorded survivor that is now caught fails, because the reason has expired
```

Every case also asserts the subject file came back byte-identical, because a
harness that leaves a mutated tree makes every later run a lie.

**And against the real thing.** Driven at `slate-kernel`'s `soft_delete` suite,
it distinguishes the three outcomes that a grep cannot:

```
baseline: 1 suites reported, none failing
  ok   a real mutation that is caught  ->  without_read_deleted_the_bulk_writes_stay_out_of_reach_too
  !!   the loop's two filters swapped: SURVIVED (1 suites ran).
  !! a mutation that does not compile: NOTHING RAN — ["error[E0599]: no method named `no_such_method_exists` ...]
restored: 1 suites reported, none failing
```

The middle line is today's real survivor — the redundant branch documented in
`ledger/2026-09-20-the-cost-the-measurement-pointed-at.md` — and with its reason
recorded it reads `survived, as recorded` and exits zero.

**One of my own test cases was wrong**, which is worth recording given the
subject: the "matches twice" case first used `"O"` against `ORIGINAL`, where it
occurs once. The harness correctly reported a survivor and the *test* failed,
which is the harness catching my error about the harness.

`scripts/check.sh` 21/21, `ruff` clean over both files,
`scripts/test_check_sh.py` accounts for the new CI step.

## What this does not do

**It does not find mutations for you.** It runs the ones you name. That is
deliberate — see the `cargo-mutants` rejection — and it means the coverage
question "what *could* be mutated here" is as open as it was.

**No mutation spec is committed anywhere.** Each entry's evidence table is still
hand-written from a run, and nothing re-runs today's mutations tomorrow. A
committed spec per module that CI drives is the obvious next step and it is not
here; it would need the suites, which is a different CI job from the one this
sits in.

**It assumes the command is a Rust test run.** The two regexes are libtest's.
Pointing it at `pytest` or `go test` would report zero suites and refuse, which
is at least a loud failure rather than a wrong one — but it is not supported.

**It does not enforce its own use.** Nothing checks that a ledger entry's
mutation table came from this script rather than a hand-run `sed`, and the
three failures it exists to prevent are all still available to anybody who does
not reach for it. `CLAUDE.md` now says to; that is persuasion, not a guard.
