# CI runs mypy and ruff over the Python client, which it had been installing and not running

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5), working the second six of `docs/orm-comparison.md`
- **Touches:** `.github/workflows/ci.yml`, `clients/python/pyproject.toml`,
  `clients/python/src/slate/` (`__init__.py`, `client.py`),
  `clients/python/tests/` (conftest and eleven test modules)
- **Kind:** process

## What changed

The `Python client` CI job runs `python -m mypy` and `python -m ruff check .`
before the suite. Getting there meant closing 53 mypy errors and 23 ruff
findings, all of which had accumulated unseen; `mypy` and `ruff` are now pinned
exactly, as `grpcio-tools` already was. Two shared helpers, `as_int` and
`as_str`, went into `tests/conftest.py` for the narrowing every test that reads
a column back was doing implicitly.

## Why

`pyproject.toml` has said since this package was written that "a type error in
this package is a test failure, not a lint", and the same about ruff. The CI
job installed `.[dev]` — which pulls both in — and then ran only `pytest`. So
the tools were downloaded on every build and never invoked, which is the most
expensive way to not have a check, and the claim in the configuration file was
false for months.

The item as written was "put mypy in CI". Ruff turned out to have the identical
gap, in the same file, with the same false claim beside it. Fixing one and
leaving the other is the shape of bug this repository keeps finding, and which
the deployed-harness change earlier today was entirely about — so both.

## Alternatives rejected

**Narrowing mypy to `src/slate` and leaving `tests/` out.** It would have been
green immediately: the package itself has zero errors, all 53 were in tests.
And it would have meant editing `files = ["src/slate", "tests"]` down to make a
check pass, which is weakening the standard rather than meeting it. The tests
are where a wrong assumption about what a column returns actually shows up.

**Leaving the tools floating.** `mypy>=1.10` and `ruff>=0.6` were what was
there, and both are now exact. Under `strict`, `warn_unused_ignores` is on, so
a `# type: ignore` is an error when it is missing *and* an error once it is no
longer needed — four in `tests/` were required when written and are "unused" on
mypy 2.3, with nothing here changed. Ruff is the same in the other direction:
`select` names rule *families*, so an upstream release adds rules to this
package's configuration without anybody editing it. This repository has already
paid this once, with `grpcio-tools`, and wrote down why. The cost is that a
bump is now a deliberate act, which is the point.

**Adding `ruff format --check` while in the neighbourhood.** 26 files would be
reformatted, and this package has never claimed to use the formatter — only
`ruff check`, which `[tool.ruff.lint]` configures. Adding it would be imposing
a new standard under cover of enforcing an old one, and a large mechanical diff
across files other people are working in. Deliberately not done, and the
formatter is still not run anywhere.

**`cast()` for the narrowing helpers.** Shorter, and it checks nothing.
`as_int` and `as_str` assert, so a column that comes back as the wrong type
fails there with the value in the message instead of surfacing as a comparison
that never matches. `as_str` earns its place twice over: `f"{value}"` on a
`bytes` column produces `b'...'` rather than the text, which is a decoding bug
that would have read as a computation bug.

**One `&&`-joined CI step.** Two steps, so the log names which tool failed
without anybody opening it, and both before the suite because they take seconds
where it takes minutes.

## Evidence

**Two real defects in the package's public surface, found by the checkers
rather than by a test.**

- `CalendarUnit` was imported into `slate/__init__.py` and left out of
  `__all__`, unlike `TimeUnit`, `Metric` and `Scalar` beside it. Found by ruff
  as `F401 imported but unused`.
- Chasing that one turned up a second: `calendar_part` and `calendar_trunc`
  are exported, and `CalendarPart` — the enum they take as their first
  argument — was not imported at all. A caller could reach the functions from
  `slate` and could not call them without a second import from `slate.scalar`.
  Both enums are exported now.

Also a `zip()` over two plan input lists with no `strict=`, in a test comparing
a grouped plan against a plain one: mismatched lengths would have truncated
silently and the assertion below would have passed on the shorter list. Now
`strict=True`.

**The checks fail when they should**, which is the part `CLAUDE.md` says to
verify rather than assume — a check that never fires is a check nobody has
debugged. Adding `def _mutation(x: int) -> str: return x` to a test module
exits mypy 1; adding an unused `import os` exits ruff 1; both exit 0 with those
removed.

**And everything still passes**: 238 tests, mypy clean over 40 source files at
both `--python-version 3.11` (local) and `3.12` (CI's interpreter), ruff clean.

The 53 mypy errors were in ten test modules and almost all one shape: `Row.get`
returns the whole `PyValue` union, and a test that reads an `id` back wants an
`int` to sort, compare or key a dict on. `as_int` closes all of those. The rest
were four ignores that had gone stale, three helpers typed `object` in a file
written earlier this session (`test_paged_join.py` — mine, and sloppier than it
should have been: `session: object` and then `.page_join` on it behind an
`attr-defined` ignore), and `BatchOutcome.written`, which is `None` on a failed
operation with `ok` as a separate property, so no checker can see that
asserting one narrows the other. That is now a `_written` helper that asserts
and names the error.

## What this does not do

- **It does not run the formatter.** See above. 26 files are unformatted by
  `ruff format`'s reckoning and stay that way.
- **It does not check the other Python in this repository.** `scripts/`,
  `site/check/`, `examples/deployed/` and the conformance runner are all
  Python and none is type-checked or linted; only `clients/python` has the
  configuration for it. Worth its own item — `examples/deployed/check.py` in
  particular is several hundred lines that a checker would have opinions about.
- **It does not enable `SLF001`.** Three test modules reach into
  `client._conn.stub` to count requests and carried `# noqa: SLF001` for a rule
  that is not in `select`. The noqas are gone rather than the rule added: the
  `select` list is the standard, and a comment suppressing a rule nobody turned
  on is noise that reads as a real suppression.
- **It does not raise the floor on `python_version`.** mypy analyses at 3.11
  because `requires-python` says `>=3.11`, whatever interpreter runs it.
