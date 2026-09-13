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

## Practical notes

- `cargo fmt --all` touches other agents' in-flight files. Use
  `cargo fmt -p <your-crates>`.
- Disk is tight and several builds run at once. A linker `Bus error`, an
  `rustc-LLVM ERROR: IO failure`, or a sudden burst of `E0463: can't find
  crate` is almost always ENOSPC or a damaged build cache, not your code.
  Reclaim with the dedup snippet in `ledger/README.md`, and never delete
  `target/debug/build` — that breaks build-script outputs and produces
  hundreds of convincing, fictional compile errors.
- `cargo test` stops at the first failing binary. Use `--no-fail-fast` before
  concluding how much is broken.
