# Working in this repository

## The ledger is not optional

**Every change outside `ledger/` needs a ledger entry, in the same commit.**
A `pre-commit` hook enforces it and will refuse the commit otherwise.

```sh
cp ledger/TEMPLATE.md ledger/$(date +%F)-a-short-slug.md
```

Read [`ledger/README.md`](ledger/README.md) before writing your first one. The
short version: one file per change, never a shared log — several agents work
here at once and a shared file conflicts on every commit.

The hook checks that the five sections exist and have something under them, and
refuses an entry still carrying the template's own prose. It cannot check
whether what you wrote is true or useful. That part is yours, and the section
worth the most is **Alternatives rejected** — what else would have worked and
what it would have cost. It is also the one most often left thin.

An entry is not a commit message. The message says what this commit does; the
entry says why the change exists at all, what it was weighed against, what was
measured, and where it stops. If the two say the same thing, one of them is
wrong.

`--no-verify` bypasses the hook. It exists so that a check with no escape hatch
does not get switched off permanently the first time it blocks a recovery. A
bypassed commit is a commit whose reasoning is nowhere, so write the entry
afterwards.

## How work is judged here

These are the standards the existing code and docs are held to. They are not
aspirational — `docs/correctness.md` and `docs/performance.md` are full of
worked examples, including several where a claim was withdrawn.

**Comment the why, never the what.** Every non-obvious decision carries the
reasoning, the alternatives rejected, and what they would have cost. A comment
restating the code is noise; a comment explaining why the obvious approach was
wrong is the most valuable line in the file.

**A finding must be demonstrated, not argued.** A failing test, a measurement,
a mutation that got caught. "This looks wrong" is a hypothesis and should be
labelled one.

**Mutation-test what you write.** Break each thing you added, confirm a
*named* test fails, restore, re-verify. A mutation that causes no failure is a
missing test — write it rather than hide it. Several of this repository's worst
bugs were found exactly this way, and several tests exist only because a
mutation survived.

**Prefer an oracle to a hand-written case.** A test that agrees with an
independent implementation catches the cases nobody thought of, which is the
whole point. Hand-written differentials test the cases somebody thought of. Both
have their place; know which one you are writing.

**Measure, do not assert.** Report spread, not a single number. A difference
inside run-to-run noise is not a finding — say so. Never report a number you
did not observe. If a result contradicts a hypothesis you already wrote up,
withdraw the hypothesis and say that you did.

**Report honestly.** What you did not test, what you guessed at, dead ends you
hit. A null result stated plainly is worth more than a manufactured finding. If
you left part of the task undone, say which part and why.

**Stale documentation is worse than none**, because it is read as current. If a
change makes a doc comment, a README bullet or a design note wrong, fixing it is
part of the change.

## What runs, and where

`main` is the trunk. `.github/workflows/ci.yml` runs on **every push, to every
branch** — seventeen jobs covering formatting, the Rust workspace, the Go,
Python and TypeScript clients, the demo frontend, the pre-commit hook's own
tests, a workspace-layout guard, the repository's *other* Python (every
harness, script and site check outside `clients/python`), the landing page's
quickstarts, the three-SDK conformance runner, a browser e2e, MinIO, the whole
stack deployed against object storage, and the release build for both shipping
targets.

That sentence was false until recently in a way worth knowing about: the
workflow existed, was marked active, and had run **zero times**, because it
triggered only on `main` and pull requests while every branch was a feature
branch with no pull request. Turning it on found eleven real defects in one
morning. **A check that never fires is a check nobody has debugged** — which
applies to anything you add here too.

Locally, the useful subset:

```sh
cargo fmt -p <the crates you touched>                   # CI checks --all; see below
cargo test -p <the crates you touched> --no-fail-fast   # see the disk note below
cargo clippy --workspace --all-targets                  # RUSTFLAGS=-D warnings in CI
sh .githooks/test-pre-commit.sh                         # the hook's own suite
ruff check . && ty check                                # the Python outside clients/; see below
python3 scripts/check_workspace.py                      # every crate is a member
python3 site/check/docs.py                              # the docs site holds together
python3 site/check/quickstarts.py                       # the docs' code, run
python3 site/check/workbench.py                         # the kernel, in a browser
cd examples/explorer && ./run.sh --conformance          # the three SDKs agree
cd examples/explorer && ./run.sh --e2e                  # the demo, in a browser
cd examples/deployed && ./run.sh                        # the whole stack, for real
```

The client suites and the demo build `slate-serverd` with `cargo` by default.
`SLATE_SERVERD=/path/to/slate-serverd` (and `SLATE_TESTSERVER` for the Python
suite) points them at a prebuilt binary instead, which is how CI builds it once
for every job and how you run a client suite with no Rust toolchain. A path
that is set and missing is a hard error, never a silent fall back to building.

## Practical notes

- `cargo fmt --all` touches other agents' in-flight files. Use
  `cargo fmt -p <your-crates>`, and actually run it: CI checks `--all`, and a
  session that formatted nothing turned that job red on nothing but line
  breaks. It is its own job now, so it no longer hides clippy and the tests
  behind it, but red is still red.
- Disk is tight and several builds run at once. A linker `Bus error`, an
  `rustc-LLVM ERROR: IO failure`, or a sudden burst of `E0463: can't find
  crate` is almost always ENOSPC or a damaged build cache, not your code.
  Reclaim with the dedup snippet in `ledger/README.md`, and never delete
  `target/debug/build` — that breaks build-script outputs and produces
  hundreds of convincing, fictional compile errors.
- `cargo test` stops at the first failing binary. Use `--no-fail-fast` before
  concluding how much is broken.
- **A skip is green.** The Python harness used to *skip* its whole suite when
  `cargo` was absent, which in CI reads as a passing suite that started no
  server and exercised nothing. Prefer a hard error to a skip whenever the
  thing being skipped is the point.
- **A workflow filter that does not match is silent.** `paths:` beside `tags:`
  in a `push` trigger adds no branch pushes *and* would stop a tagged release
  publishing; a `paths:` filter also does not match when a branch is created,
  which would have meant the site never deployed at all. Both are written up in
  the workflow files. Prefer running something cheap unconditionally.
- **CI's clippy is newer than yours, and `-D warnings` makes that fatal.**
  `dtolnay/rust-toolchain@stable` tracks the current release; this container
  has whatever it was built with. That gap is not theoretical: three commits
  in a row went red on `chunks_exact_to_as_chunks` and `unnecessary_sort_by`,
  and later a fourth on `useless_borrows_in_formatting` — three lints that **do
  not exist** in the local toolchain (`cargo clippy -- -W
  clippy::useless_borrows_in_formatting` here answers `unknown lint`), so
  `cargo clippy --workspace --all-targets` was clean here and failed there.
  Run the workspace command rather than `-p your-crate` before pushing — it
  catches the crates you forgot — and treat a green local clippy as necessary
  rather than sufficient. Installing a second toolchain to check is usually
  not possible here; the disk note above is why.

  All three were in **test** code, which is only linted under `--all-targets`,
  and the third arrived by a mechanical edit: a script rewriting `row.computed()`
  to `&row.computed` put the `&` into a format argument as well as into the
  `matches!`. Re-read a scripted edit's output the way you would re-read one you
  typed; a replacement that is right in most positions is not right in all of
  them, and that is a different failure from the toolchain gap even though it
  surfaced through it.
- **`ty` resolves imports against whatever `site-packages` you happen to have,
  and CI has almost none.** The `scripts` job installs `clients/python[dev]`
  and nothing else, so a script importing a module this container happens to
  carry passes here and fails there — which is how `site/data/make-trips.py`
  and its `pyarrow` went red on a file nobody had touched. To run the check the
  way CI does:

  ```sh
  python3 -m venv /tmp/ci-env && /tmp/ci-env/bin/pip install -e './clients/python[dev]'
  /tmp/ci-env/bin/ty check --python /tmp/ci-env
  ```

  A bare `ty check` is necessary and not sufficient, the same way a green local
  clippy is.
- **Pin anything that generates committed code.** `grpcio-tools` was declared
  `>=`, so the test that regenerates the Python protobuf stubs and compares
  them byte for byte was pinned to upstream's release calendar. It went red
  with nothing changed in the repository.
