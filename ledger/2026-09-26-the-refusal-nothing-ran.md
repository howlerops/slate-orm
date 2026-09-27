# Two refusals this repository had written and never exercised: a mistyped benchmark section, and a mutation dialect that reads too much.

- **Date:** 2026-09-26
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/run_examples.sh`, `scripts/check_examples_roster.py`, `scripts/test_check_examples_roster.py`, `scripts/test_run_examples.py`, `scripts/mutate.py`, `scripts/test_mutate.py`, `docs/caveat-status.json`
- **Kind:** process

## What changed

`run_examples.sh` grew a `refuses()` table beside its `handshake()` one: for
each example that reads section names off its command line, an argument none of
them has. After the example's ordinary run, the runner hands it that argument
and requires **exit 2** — folded into the example's own result line, so the
closing `N passed, M failed` still counts examples. `check_examples_roster.py`
gained a fourth rule holding that table equal to the examples whose source calls
`sections::from_args`, in both directions, plus a check that the argument is not
a string in that example's own source. Separately, `test_mutate.py` gained
`CORPUS` — real clean and failing output from all five runners `mutate.py`
speaks — and three cases over it, the load-bearing one being that **no
dialect's failure pattern matches anything in any dialect's clean run**.

## Why

Both are the same defect, five days apart: a refusal that was written, was
correct, and was never run.

`head_report --typo` used to print its header, run nothing, and exit 0.
`slate_headbench::sections` fixed that in
`ledger/2026-09-26-a-mistyped-section-is-a-refusal-now.md`, and that entry
recorded its own hole in the same breath — *"no automated run passes `--typo` to
a built binary and asserts exit 2"*. `Selection::new` has six unit tests;
`from_args`'s `std::process::exit(2)` beneath it, which is the part a reader at
a terminal actually meets, was reached by nothing. A refusal nothing exercises
can be deleted by an edit nobody notices, and the run stays green, because a
benchmark that has stopped refusing still benchmarks.

`mutate.py`'s hole is the same shape one level up. All six of its recorded lies
are a pattern matching too *little*, and each is loud in the end: a missed
failure scores as a survivor, and a survivor exits non-zero and demands an
explanation. A failure pattern that matches a line which is **not** a failure is
the mirror image and is silent. Every mutation then looks caught, the run exits
0, and the session writes up a test that defends nothing. Nothing in `mutate.py`
reads its failure patterns at all — the exact-once rule is about the anchor and
the reported-suites count is about the *report* pattern — so the tool cannot
catch this about itself. Recorded as a caveat in
`ledger/2026-09-22-four-dependencies-and-a-tool-that-was-lying.md`.

## Alternatives rejected

**For the benchmark refusal.**

*A Rust integration test spawning the example binary.* The natural place, and it
cannot be written cheaply: `env!("CARGO_BIN_EXE_…")` exists for `[[bin]]`
targets and not for examples, so the test would have to find or build the
binary itself — reimplementing the part `run_examples.sh` already does, in a
crate whose test binaries are among the largest here. The runner already has the
binaries in hand; the check is four lines there.

*A shared bad-section constant instead of one argument per example.* Shorter,
and it gives up the check that matters most. The guard compares each argument
against that example's own source, because an argument that is accidentally a
real section makes the example run, exit 0, and the runner report `wanted 2` —
a failure that reads as "the refusal is gone" and sends the next reader to look
for something that never moved. One shared constant cannot be checked per
example, and the table is three lines.

*Counting the refusal as a check of its own, with its own report line.* Rejected
because the closing `N passed, M failed` is read against the number of
examples, and a run printing more results than there are examples adds a third
number to keep true. `scripts/test_run_examples.py` already has four cases about
that arithmetic.

**For the dialect corpus.**

*A script that runs all five real runners on tiny generated projects, in CI.*
The strongest oracle, and the one this started as. It needs `rustc`, `go`,
`node` and `pytest` present, which is exactly what `test_mutate.py`'s docstring
says it deliberately does not need — *"no toolchain, no build and no seconds —
which is what lets it sit in the static check run beside the linters."* Rebuilt
as a captured corpus instead: the samples still come from the real runners, once
each, with the commands that produced them written down; they simply do not have
to be re-run to be checked. The cost is real and stated under **What this does
not do**.

*Checking each dialect against its own clean sample only.* Half the value.
A dialect is chosen by the caller and applied to whatever that command prints,
so `python` pointed at `go test` is an ordinary mistake — and the `go` dialect's
own *report* pattern had to be narrowed for `go test`'s bare trailing `FAIL`
already. The cross product is twenty-five comparisons and costs nothing.

*Leaving the caveat open on the grounds that the patterns look right.* That is
the hypothesis this repository asks to be demonstrated. Two realistic
over-matches were written and both were caught; see below.

## Evidence

**The real binary refuses.** Built at `--release` (debug does not fit on this
container — `CLAUDE.md`'s note), and run:

```
$ ./target/release/examples/head_report --not-a-section
no section named --not-a-section. This example has: rpc, stream, commit, routing, lease, views.
Run it with no arguments for all of them.
exit=2
```

**Mutations, all through `scripts/mutate.py`.** Seven runs, recorded:

- `ledger/mutations/20260926T230113-scripts-run-examples-sh.json` (4 cases)
- `ledger/mutations/20260926T230248-scripts-run-examples-sh.json` (2)
- `ledger/mutations/20260926T230356-scripts-run-examples-sh.json` (1)
- `ledger/mutations/20260926T230452-scripts-check-examples-roster-py.json` (8)
- `ledger/mutations/20260926T230604-scripts-check-examples-roster-py.json` (2)
- `ledger/mutations/20260926T231134-scripts-mutate-py.json` — **scored nothing**:
  the anchor occurred zero times because the spec was hand-written into a
  heredoc and the shell ate a level of backslashes off the regex. Protection 1
  doing its job; the spec was generated with `json.dumps` after that.
- `ledger/mutations/20260926T231149-scripts-mutate-py.json` (4)

Twenty-one scored cases, four survivors. Three were findings and are below; the
fourth was my own command being too narrow — deleting `head_report`'s refusal
line survived against `test_run_examples.py` alone, because that file reads the
roster *off the script* rather than restating it, and the guard that catches a
deleted line lives in the other suite. Re-run against both: caught.

Against `scripts/run_examples.sh`, scored by `test_run_examples.py` and
`test_check_examples_roster.py` together:

| mutation | outcome |
| --- | --- |
| `bad=$(refuses …)` → `bad=` | caught, 3 named cases |
| `[ "$code" -eq 2 ]` → `-ne 77` | caught, 2 named cases |
| the `failed` increment deleted | caught, 2 named cases |
| `head_report`'s refusal line deleted | caught by the roster guard |
| `stream_step`'s argument → `nagle`, a real section of it | caught by the roster guard |

That fifth one is worth the row: `stream_step`'s argument → `rpc` **survived**,
and correctly — `rpc` is a section of `head_report` and not of `stream_step`, so
the example still exits 2 and nothing is wrong. An equivalent mutation arriving
looking exactly like a discovery, which `CLAUDE.md` warns about; the real
section name catches it.

Against `scripts/check_examples_roster.py`, eight mutations, six caught at once:
the forward arm, the reverse arm, the argument-in-source arm, both never-fires
halves, `TAKES_SECTIONS` matching nothing, and `body()` slicing the wrong
function. **Two survived:**

- `body()`'s `^\}` → `\}`. A real change with no observable effect: no arm in
  either table contains a brace, so the slice is identical. Fixed by a case with
  a `${…}` in an arm, which truncates the table under the mutation and drops the
  spare crate's line. Re-run: caught.
- `body()` returning `""` rather than `None` for a missing function. Genuinely
  redundant — both callers tested it with `if not …`, so the distinction the
  docstring claimed was never acted on. Deleted the `None` and the claim rather
  than writing a test for a branch nothing reads.

Against `scripts/mutate.py`'s own `DIALECTS`, scored by `test_mutate.py`:

| mutation | outcome |
| --- | --- |
| rust `… \.\.\. FAILED$` → `… \.\.\. \w+$` | caught — found `['a_unit_test', 'b_unit_test']` in a **clean** run |
| node `^not ok \d+ -` → `^(?:not )?ok \d+ -` | caught — found `['a named case']` in a clean run |
| pytest `FAILED\|ERROR` → `FAILEDX\|ERRORX` | caught, 3 named cases |
| go report `^(?:ok\|FAIL)` → `^(?:FAIL)` | caught, 4 named cases |

The first two are the caveat's exact wording — a pattern matching lines that are
not failures — and before this they matched nothing here. Every mutation run is
recorded under `ledger/mutations/`.

**And a citation I invented, caught by the guard written for it.** The
`CORPUS` docstring first cited
`ledger/2026-09-21-mutate-py-reads-three-more-runners.md`, which has never
existed; the caveat is in
`ledger/2026-09-22-four-dependencies-and-a-tool-that-was-lying.md`.
`scripts/check_cited_docs.py` failed the `check.sh` run on it. That is the
**sixth** invented citation in three days and the fifth a guard caught, which
is the only reason any of them is a footnote rather than a defect somebody
follows.

**Suites.** `scripts/test_run_examples.py` 19 passed 0 failed (was 15),
`scripts/test_check_examples_roster.py` 24 passed 0 failed (was 16),
`scripts/test_mutate.py` 70 passed 0 failed (was 41).

## What this does not do

**The corpus is captured, not re-captured.** Five samples from five runners on
one day on one container. A runner whose output format moves is caught only when
its pattern stops matching its own sample — which the second case does check —
but a format that moves *toward* something another dialect's pattern matches
would not be noticed until somebody re-captures. The alternative needed four
toolchains in the static check run; the trade is stated above and this is its
cost.

**Nothing has run the negative case end to end.** The real binary was observed
exiting 2 by hand, and the runner's handling of exit 2 is tested against
fixtures. The two have not met: `slate-headbench`'s five examples do not all
exist on this container at `--release`, and `run_examples.sh` refuses a
directory short of its floor. CI's `headbench` job is the first run that will
join them, which is the same "a check that never fires is a check nobody has
debugged" risk `CLAUDE.md` names, taken knowingly.

**`node --test`'s per-file wrapper line is absent from the node samples.** The
sample was taken from a single-file run and node did not emit one; the
`(?!.*\.ts$)` exclusion in that dialect is therefore still defended only by the
hand-written fake beside it, not by real output.

**The refusal check runs one argument per example.** It proves a name with no
section is refused. It does not probe a *near miss* (`strem` for `stream`), a
name that is a section of a different example, or two bad names at once —
all three are unit-tested in `sections.rs` against `Selection::new`, and none of
them is exercised against a built binary.

**Neither guard can see a fourth kind of silent pass.** `check_examples_roster.py`
knows an example takes sections because its source says `sections::from_args`.
An example that parsed `std::env::args()` by hand — which is what all three did
before that module existed — is invisible to it, and would go back to printing a
header and exiting 0 with nothing to say so.
