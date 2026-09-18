# Five thousand lines of Python that ran twelve CI jobs and had no checker

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5)
- **Touches:** a new root `pyproject.toml`, `.github/workflows/ci.yml`,
  `CLAUDE.md`, and every Python file outside `clients/python`
- **Kind:** fix

## What changed

A root `pyproject.toml` configures ruff and ty for the repository's *other*
Python: two code generators, four harnesses under `examples/`, three site
checks, the three-SDK conformance runner, the explorer's Python adapter and the
workspace guard. A `scripts` CI job runs both. Twenty-eight lint findings and
seventeen type diagnostics are fixed, and three `examples/deployed` helpers —
`as_int`, `as_float`, `as_str` — narrow a read-back column by asserting rather
than casting.

## Why

`clients/python` has had ruff and a type checker for months, and CI has run
them since this morning. The code that *runs twelve of CI's other jobs* had
neither. So a harness could carry a name error in a branch nobody takes, or
call a method the client does not have, and the first anyone would know is a
red job with a traceback in it — which is a worse signal than the one-line
message a checker gives, and arrives after a five-minute build.

It found four real defects, listed under Evidence. None of them is dramatic;
all four are the kind that sit for months.

## Alternatives rejected

**Extending `clients/python`'s configuration to the whole repository** rather
than a second one at the root. Both tools resolve the *nearest* configuration,
so one at the root would be shadowed for everything under `clients/python`
anyway — and the two want different things. The client excludes a generated
proto tree and carries its own `# ty: ignore` spellings; the scripts want
`BLE001`, which the client does not.

**A `[project]` table in the root `pyproject.toml`**, which is what most
repositories have. Left out deliberately: this is not a package, nothing
installs it, and adding one would make `pip install -e .` at the repository
root do something — which is a thing somebody would then do by accident and
get a half-built distribution out of.

**Enabling `ruff format` while here.** It was noted as unused earlier today and
it stays unused. Formatting five thousand lines in the same commit as four
behavioural fixes would bury them, and a formatter is a decision about a
repository rather than a fix to one. Left for its own change.

**`cast` instead of the three narrowing helpers.** A `cast` satisfies a checker
and checks nothing, which is worse than no annotation because it reads as one.
The helpers assert, so a column that comes back as the wrong type — the decoder
bug a *deployed* harness exists to catch — fails at the read with the value in
the message rather than as a confusing comparison a hundred lines later. The
same three exist in `clients/python/tests/conftest.py` and are deliberately not
shared: this is an example directory that runs against an installed client, and
importing three assertions out of another project's test suite would be a worse
dependency than restating them.

**Suppressing the `log_message` override rather than fixing it.** Two
characters either way, and the fix is the honest one: `*_args` alone dropped
the base class's positional `format`, so `BaseHTTPRequestHandler` calling
`log_message(fmt, a, b)` would have bound `fmt` to a parameter that did not
exist. It never fired because the body is `pass`.

## Evidence

**Four real defects, none found by any test:**

1. `examples/explorer/backends/python/adapter/__main__.py` wrote
   `result = b and session.batch(b)` and then read `result.outcomes`. On the
   branch where the `Batch` is falsy, `result` is the *batch* and `.outcomes`
   is an `AttributeError` raised inside an HTTP handler. Three operations are
   always queued above it, so the branch has never been taken.
2. `examples/batchbench/bench.py` typed its session parameter `object` and
   silenced the two resulting errors with `# type: ignore[attr-defined]` — a
   mypy spelling that ty does not read. So the annotation was wrong, the
   suppressions were inert, and neither checker had ever seen the two calls the
   benchmark exists to time.
3. `site/check/workbench.py` passed `… and batch and float(batch.group(1)) > 0`
   to a `check(ok: bool, …)`, where `batch` is a `re.Match | None`. On no match
   the expression is `None`, and the check read as a failure by luck rather
   than by saying so.
4. `examples/deployed/check.py` passed `seen and seen <= replica_names` the
   same way, where `seen` is a set. An empty `seen` — no read named the view
   that served it, which is the thing being checked — produced an empty set
   rather than `False`.

Plus three `# noqa: BLE001` directives with reasons written beside them,
against a configuration that never enabled the rule. The rule is enabled now,
which makes those three deliberate and finds any fourth; one of the three
turned out to be inert for a different reason (the handler re-raises, which
`BLE001` already allows) and its reasoning is now a comment rather than a
directive.

**Ten mutations, all killed by ruff or ty:**

| mutation | caught by |
| --- | --- |
| a name error in a branch nobody takes | ruff `F821`, ty `unresolved-reference` |
| a harness calls a method the client does not have | ty `unresolved-attribute` |
| a blind `except` swallows a harness failure | ruff `BLE001`, `SIM105` |
| an import that is never used | ruff `F401`, `I001` |
| a typed dict built from unnarrowed columns | ty `invalid-assignment` |
| a string key interpolated without narrowing | ty `invalid-assignment` |
| the empty-set case reads as a falsy set again | ty `invalid-argument-type` |
| the adapter's `b and` comes back | ty `unresolved-attribute` |
| `re.search`'s `None` reaches a `bool` parameter | ty `invalid-argument-type` |
| `int(...)` instead of `as_int(...)` on a count | **SURVIVED**, then killed |

The last is the one worth recording, twice over. It survived first as an
**equivalent mutation**: `total = next((group.aggregates[0] …), 0)` compared
with `==` against an `int` typechecks whatever its type is, and at runtime the
value is already an `i64`, so removing the helper changes nothing a checker or
an interpreter can see. That is an honest null result about that call site: the
helper there documents intent and is not enforced.

At the *second* site it survived for a fixable reason. `got_borough` was an
unannotated dict comprehension, so a checker infers whatever the comprehension
produces and the helpers become optional. `got_borough: dict[str, int]` makes
them load-bearing, and the mutation now fails. An annotation that turns a
decorative helper into an enforced one is worth more than the helper was.

Green afterwards: `ruff check .` and `ty check` at the root, the docs check,
the workbench browser check (75 checks, after
rebuilding the wasm its staleness guard demanded), `site/check/quickstarts.py`, `scripts/check_workspace.py`, and
the three-SDK conformance runner at 86 cases — the last three of those are
themselves among the files this change edits.

## What this does not do

- **Nothing here is *run* by the new job.** It lints and type-checks; the
  harnesses are executed by the twelve jobs that already run them. A script
  that passes both tools and does the wrong thing is still only caught by the
  job that runs it.
- **`ruff format` is still unused**, on this tree and on `clients/python`. Both
  are hand-formatted and consistent enough; nobody has decided.
- **`examples/deployed` was not run end to end here.** It wants MinIO and a
  full stack, and its Python was exercised by import and by the two checkers
  rather than by a run. That is stated rather than implied: the narrowing
  helpers are new code on a path this session did not execute.
- **No docstring, naming or complexity rules.** `select` is the same families
  `clients/python` uses plus `BLE`, so a contributor moving between the two
  meets one set of rules. `D`, `N` and `C901` would each be a decision about
  five thousand lines written before them.
