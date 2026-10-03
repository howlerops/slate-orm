# `main` went red on a guard the sweep could not run, and three checks only Linux could pass

- **Date:** 2026-10-03
- **Author:** Claude Code (session: picking up from the handoff prompt, on macOS)
- **Touches:** `scripts/test_mutate_guard.py`, `scripts/check_generated_is_used.py`,
  `scripts/test_check_generated_is_used.py`, `scripts/check_handlers.py`,
  `scripts/test_check_handlers.py`, `scripts/run_examples.sh`
- **Kind:** fix

## What changed

Four defects, each making a check red for a reason unrelated to the tree.

1. **`test_mutate_guard.py`'s real-tree sweep leaves out guards that need a
   toolchain,** via a `TOOLCHAIN` roster with a reason per entry. Its one entry
   is `check_npm_package`. The roster is held to two things: every name in it
   is still a file in `scripts/`, and `ci.yml` still has a `run:` line invoking
   it — so an exemption cannot outlive its guard, nor the step that runs the
   guard somewhere it can answer. This is the one CI saw.
2. **`check_generated_is_used.py` no longer puts `\b` in a `git grep -E`
   pattern.** It reads `([^A-Za-z0-9_]|$)`, and its suite gains the two cases
   that catch both the defect and the obvious wrong fix.
3. **`check_handlers.py` resolves `root` once in `main`.** Its suite gains a
   case whose root is reached through a symlink the fixture makes, and its
   fixture runner turns a raise into a named failing case.
4. **`run_examples.sh` braces `${crate}` before a non-ASCII byte.**

## Why

**`main` was red at `bce9188`, and so were the two pushes before it** (CI runs
558, 559, 561; the job is "the Python that is not the client"). The cause is
the commit that added `check_npm_package.py`. That guard asks `npm pack` what
the tarball holds; `npm pack` runs `prepack`, which runs `tsc`. Its docstring
says it runs in CI's `typescript` job and not in `scripts/check.sh`, because it
needs `node_modules`. But `test_mutate_guard.py` sweeps **every**
`scripts/check_*.py` against the real tree to pin that each prints something
and exits 0 — and it runs in the `scripts` job, which installs Python and
nothing else, and in `check.sh`. So the new guard exited 2 there on every push,
and `check.sh` quietly acquired a dependency on npm that it promises never to
have. The mutations workflow had been given Node for exactly this guard; the
sweep, which asks the same question from a different file, had not.

The other three surfaced because this session ran on macOS, and `check.sh`
there was red on a clean tree in ways CI cannot see.

**`\b` is a GNU regex extension.** glibc honours it in an ERE; macOS's regex
library reads it as something no line contains. So on a Mac `importers()`
found no keyword import — `from .schema import` — and the guard refused two
modules that are imported (`examples/explorer/backends/python/adapter/schema.py`,
`examples/retention/schema.py`). The suite did not notice because both of its
importers quote a path, which matches the pattern's *other* arm; the keyword
arm, the only shape the real tree's Python uses, had no positive case at all.

**An unresolved root is not a prefix of its own resolved children.** The member
walks in `check_handlers.py` resolve each `crates/*/src` and then ask every file
found under it for `relative_to(root)`. Every temporary directory on macOS is
`/var/…`, which resolves to `/private/var/…`, so the suite raised `ValueError`
on its first rule-9 finding and stopped. A checkout reached through a symlink
would do the same to the real run on any system. Line 719 had the same latent
bug, reached only when a marker is found.

**macOS's `/bin/sh` is bash 3.2, which reads a high byte as part of a name.**
`"…in $crate…"` expanded `$crate\xe2` — the first byte of the ellipsis — which
is unbound under `set -u`, and the runner died on line 92 with a message that
was itself not valid UTF-8, so `test_run_examples.py` died decoding it.

## Alternatives rejected

- **Install Node in the `scripts` job.** Fixes CI in four lines. Leaves
  `check.sh` broken on every machine without `npm`, which is the promise the
  script is built on (`ELSEWHERE` in `scripts/test_check_sh.py` exists to keep
  it), and makes every future toolchain guard a reason to grow the job.
- **Have `check_npm_package` skip when `node_modules` is absent.** One line,
  and it is the thing `CLAUDE.md` names as a lie: *a skip is green*. In the
  `typescript` job a broken `npm ci` would then read as a passing guard.
- **Exclude by convention** — a module-level `NEEDS = "npm"` in each guard,
  read by the sweep. Spreads the decision over files and puts it in the
  guard's hands; a list in the one file that makes the exception is easier to
  audit and is where the two staleness checks naturally live.
- **A substring check that `ci.yml` mentions the guard.** The first draft did
  this. `ci.yml` also names `scripts/check_npm_package.py` in the comment above
  the step, so deleting the step left the check green. It matches a `run:`
  line now, and one mutation below is that deletion with the comment kept.
- **`git grep -P` to keep `\b`.** Needs a git built with PCRE, which is the same
  portability bet one layer down. `-w` does not fit: it applies to the whole
  match, which begins with `from`/`import` or a quote.
- **Resolve at each `relative_to`.** Two call sites today and a third the next
  time somebody adds a walk. Resolving the argument once makes every later use
  agree with the paths the walks produce.
- **A symlink case that relies on macOS's `/var`.** It would test nothing on
  Linux, which is where CI runs. The fixture makes its own link, and the
  evidence below shows that under Linux's conditions it is the *only* case
  that catches the regression.
- **Make the runner portable past the `${crate}` fix.** It also calls GNU
  `timeout`, which macOS does not ship; see the last section. That is a missing
  tool rather than a wrong line, the same class as `ruff` being absent, and
  fixing it is a design choice (fall back to `gtimeout`, reimplement in shell,
  or refuse up front) that deserves its own change.
- **Leave the macOS failures and call macOS unsupported.** This session is on
  macOS, and each one costs the next session on a Mac an hour working out that
  a red check is lying. Three of the four fixes are a line each.

## Evidence

Reproduced before fixing, on this machine with no `node_modules`:
`python3 scripts/test_mutate_guard.py` printed `FAIL check_npm_package: FAIL
\`npm pack --dry-run\` exited 2` — the line in CI run 561's log. `git grep -c
-E` against the adapter's `__main__.py`: `schema\b` found 0 lines, `schema` and
`schema([^A-Za-z0-9_]|$)` found 1. `test_check_handlers.py` and
`test_run_examples.py` both failed identically on an unmodified worktree of
`bce9188`; the runner's raw stderr was
`run_examples.sh: line 92: crate\xe2: unbound variable`.

Mutations, all via `scripts/mutate.py`:

| record | mutation | caught by |
| --- | --- | --- |
| `ledger/mutations/20261003T184020-scripts-test-mutate-guard-py.json` | the `TOOLCHAIN` filter removed | every real guard passes and says so |
| same record | the roster entry renamed to a guard that does not exist | `TOOLCHAIN names check_npm_packagex, which is not in scripts/` |
| `ledger/mutations/20261003T184111-github-workflows-ci-yml.json` | the step's `run:` replaced, its comment kept | `TOOLCHAIN leaves check_npm_package out of the sweep, and ci.yml no longer runs it` |
| `ledger/mutations/20261003T184234-scripts-check-generated-is-used-py.json` | back to `\b` | a keyword import with nothing quoted counts |
| same record | no boundary after the stem | a module whose name only starts with the stem is not an import |
| same record | the boundary admits `_` | a module whose name only starts with the stem is not an import |
| `ledger/mutations/20261003T185017-scripts-check-handlers-py.json` | `root = root.resolve()` removed | the rule-9 cases, by name, on macOS |
| `ledger/mutations/20261003T185032-scripts-check-handlers-py.json` | the same, with `TMPDIR` set to a resolved path so no temp dir is behind a link — Linux's condition | a root reached through a symlink is read, not raised on — and nothing else |

Three earlier records from this work are kept because they are part of how it
was found, not because they score the final code:
`ledger/mutations/20261003T184153-scripts-check-generated-is-used-py.json` is
the `\b` and no-boundary mutations **surviving** the suite before the two new
cases existed — that, not an argument, is what established the keyword arm was
untested. `ledger/mutations/20261003T184927-scripts-check-handlers-py.json` is
the resolve mutation scored `NOTHING RAN`, because the suite crashed rather
than failing; `mutate.py` refused to call a crash a catch, which is why the
fixture runner now converts a raise. And
`ledger/mutations/20261003T185019-scripts-test-check-handlers-py.json` is a
survivor that was my mistake rather than a finding: it mutated the *test's*
link with the guard fix still in place, so the case passed because the guard
was right. The `TMPDIR` run is the question that one meant to ask.

`run_examples.sh`'s fix has no mutation record. Its suite cannot reach a
baseline on this machine (below), and `mutate.py` will not score against a red
one. The evidence is the before-and-after of the same command: before, the
`crate\xe2` line and a `UnicodeDecodeError`; after, the runner reaches line 246
and each example reports exit 127 from `timeout`.

After: `test_mutate_guard.py` 13 passed, `test_check_generated_is_used.py` 13
passed, `test_check_handlers.py` 63 passed; `check_generated_is_used.py` exits
0 on the real tree.

## What this does not do

- **The `\b` regression is caught only on macOS.** On Linux `\b` works, so the
  mutation back to it is equivalent there and CI cannot catch a reintroduction.
  The case is still right; it is the platform that has no difference to see.
- **`run_examples.sh` still needs GNU `timeout`, which macOS does not ship.**
  With no `timeout` on `PATH`, every example reports `exit 127`, which reads as
  nineteen broken examples rather than one missing tool. `brew install
  coreutils` provides it as `gtimeout`, which the runner does not look for.
- **`check.sh` on this machine is not green.** Before these fixes, with
  nothing installed: 87 passed, 10 failed. After them, with `ruff`, `ty` and
  `clients/python[dev]` in a scratch virtualenv on `PATH`: 92 passed, 5 failed
  — `example-runner-guard` (the `timeout` above) and four TypeScript checks
  that need a `node_modules` this checkout does not have. Neither is in a file
  this touches. The `npm ci` that would settle the four was not run.
- **Nothing finds the next GNU-only construct.** The other two `git grep`
  callers use `-F`. A guard refusing `\b` in an `-E` pattern would cover one
  spelling of a class whose members are not enumerable; the same is true of
  the next unbraced `$name` before a multi-byte character, of which a scan of
  every tracked shell script found only the one fixed here.
- **The sweep's exemption is a list maintained by hand.** A new guard needing a
  toolchain will turn the `scripts` job red, as this one did, and be added
  here; that is loud rather than silent, which is the acceptable direction.
- **CI was not watched to green before this entry was written.** The run's
  conclusion is read after the push, not here.
