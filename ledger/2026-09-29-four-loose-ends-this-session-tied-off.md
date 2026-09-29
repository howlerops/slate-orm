# Four things this session's own entries said it had not done, done

- **Date:** 2026-09-29
- **Author:** Claude Code, task K6
- **Touches:** `scripts/{mutate,mutate_guard,test_mutate,test_check_sh,test_run_teardown}.py`,
  `examples/explorer/run.sh`
- **Kind:** fix

## What changed

Four caveats opened by four entries earlier today, closed:

1. **`mutate.py --help` says which dialect each adapter reports in.**
   `mutate_guard.py` declares `DIALECT = "python"`; `--help` reads it out of
   the file as text and prints it in a column; `test_mutate.py` checks the
   declared name is one `mutate.py`'s own table holds.
2. **`test_check_sh.py`'s `env:` comparison reads every workflow**, not
   `ci.yml` alone, and reports a name two workflows set differently.
3. **`CLAUDE.md`'s job count is checked against `ci.yml`.** Both occurrences,
   by the expression that file prescribes for itself.
4. **`run.sh --headless` prints its run directory**, so
   `test_run_teardown.py` reads the path instead of deriving it.

## Why

Each is a sentence one of today's entries wrote about itself, and the reason
for doing them together is that they share a failure mode rather than a
subject: **something a tool knows and does not say, so a reader reconstructs
it and the reconstruction can be wrong without anything noticing.**

- The adapter's dialect was in the file and not in `--help`, so a reader had
  to open `mutate_guard.py` — which is what
  `ledger/2026-09-29-the-tool-now-prints-its-own-adapters.md` was already
  about, one field short.
- The `env:` block is what makes a byte-identical command a *different check*
  in CI than locally — `RUSTFLAGS: -D warnings` is the worked example — and
  the comparison read one of five workflows. Only `ci.yml` has such a block
  today, so this changes no answer, which is exactly when widening is cheap.
- `CLAUDE.md` says how many jobs CI runs and says of itself that *"nothing
  checks it, which is why it said seventeen for as long as it did"*. It then
  drifted a second time, to twenty-one against twenty-two, corrected by hand
  in `ledger/2026-09-29-i-filtered-the-summary-line-out-of-my-own-check.md`.
  Twice is the count this repository uses to decide a habit needs a check.
- `run.sh` writes each service's log under `$TMPDIR/slate-explorer-<port>` and
  prints that path in the plain mode's banner and not in `--headless`, so the
  teardown test computed it. A second copy of an expression that would break
  **silently**: those logs are read only to explain a failure, so a wrong path
  means no explanation rather than an error.

## Alternatives rejected

**Importing each adapter to read its `DIALECT`.** One line instead of a
regex, and it runs the adapter's module-level code — `--help` should not be
able to start a subprocess. Reading the assignment as text costs a pattern and
cannot execute anything.

**Deriving the dialect from the adapter rather than declaring it.**
`mutate_guard.py` prints `N passed, M failed`, so the dialect is inferable from
its own output format. Inferring it means running it, and a declaration that is
checked against the table is stronger than an inference that is not.

**A number in `CLAUDE.md` rather than a word, so the check is a substring.**
The file writes prose and reads as prose; a digit in the middle of a sentence
about what CI covers would be the tail wagging the dog. `COUNTED` maps the nine
words a repository of this size could plausibly need, and a tenth job beyond
that range fails loudly — which is the right failure, because the sentence
would need rewriting anyway.

**Checking every number in `CLAUDE.md`.** The obvious generalisation and the
wrong one: that file's other numbers are measurements with their own
provenance (4.7 ns a frame, 131 MB at `--release`, 1.4 GB in debug), and a rule
that demanded a source for each would either be a roster of nine special cases
or a lint that fires on prose. One number, derived from one file in this
repository, by the expression that file already prints.

**Making `test_run_teardown.py` fall back to deriving the path when the runner
does not print one.** Belt and braces, and it would hide the brace breaking:
the derived path is the thing being removed, and keeping it as a fallback means
a change to the convention still produces no explanation and no complaint. The
test now says *"the runner never printed a 'logs ' line"* instead.

## Evidence

- `python3 scripts/mutate.py --help` ends with
  `mutate_guard.py        python   Run repository guards and report in a shape
  scripts/mutate.py can read.`; `python3 scripts/test_mutate.py` — 75 passed,
  0 failed, with the case renamed to say it checks the dialect too.
- `python3 scripts/test_check_sh.py` — **7 passed** (was 6), 0 failed, 131
  steps, 12 blocks and 2 env vars. The new check is `CLAUDE.md's job count is
  ci.yml's`, and the `env:` one is renamed from *"check.sh exports ci.yml's
  workflow-level env"* to *"every workflow's top-level env"* because the old
  name is now false.
- **`run.sh --headless` prints the path, and it resolves.** From a real run's
  captured output: `logs /tmp/slate-explorer-7421`, which parses to a
  directory holding `go.log`, `head.log`, `node.log`, `python.log`.
- **The teardown test still passes** against a clean machine — `ok the runner
  reaps everything it started, on SIGTERM (7 processes)` — and its port
  pre-flight still refuses a dirty one, exercised by holding the four ports
  with a live `run.sh` and watching it report `FAIL the runner starts its four
  services` naming all four.
- `sh scripts/check.sh` exits 0, at the second attempt: the first found
  `invalid-return-type` in `mutate.py`, because widening `adapters()` to
  return a three-tuple left its annotation saying two. `ty` caught it and
  nothing else would have — the function's one caller unpacks three names, so
  Python runs it fine. That is the check's whole job and it is worth recording
  as a catch rather than as a step that passed.
- **No mutation run.** Three of the four changes are covered by a case in a
  suite a mutation run can already score, and the fourth — `run.sh` printing a
  line — is exercised by the observation above rather than by a test. Where
  each stands is in *What this does not do*, rather than counted as coverage
  here.

## What this does not do

**The log-tail path was not re-exercised end to end after the change.** It runs
only when the runner starts *and then* fails to reach readiness, and the port
pre-flight now catches the failure that used to produce it. What was verified
is the two halves separately: `run.sh` prints the line, and the line parses to
the real directory with its four logs in it. The join between them is read, not
run.

**The job-count check knows nine number words.** A twenty-sixth job fails it
with "not a number COUNTED knows", which is a true and slightly unhelpful
message pointing at the right file. A word-to-number library for one sentence
is not worth a dependency.

**Two workflows setting one name differently is reported as a mismatch against
`check.sh`, not as a conflict between them.** The complaint carries both
values, so a reader sees the disagreement, but the check's subject is still
"does `check.sh` export what CI sets" and there is nothing today to test the
new branch against — no second workflow has a top-level `env:`.

**`--help` prints a declared string, not a verified behaviour.**
`test_mutate.py` checks `mutate_guard.py`'s `DIALECT` names a dialect that
exists; nothing checks that its output is *readable* by that dialect. The two
would come apart if the adapter's summary line changed, and what would catch
that is a mutation run against a guard, which is not this check.
