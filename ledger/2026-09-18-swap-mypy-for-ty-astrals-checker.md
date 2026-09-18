# The Python client type-checks with `ty`, not mypy

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5), at the maintainer's request
- **Touches:** `clients/python/` (`pyproject.toml`, `src/slate/`, `tests/`,
  `README.md`), `.github/workflows/ci.yml`
- **Kind:** process

## What changed

`mypy` is gone. `ty` — Astral's type checker, the same house as `ruff`, which
this package already used — checks `src/slate` and `tests` instead, configured
under `[tool.ty]`. CI installs with `uv` and runs `ty check`, `ruff check .`
and then the suite.

`mypy-protobuf` stays despite the name: it is a *protoc plugin* that emits the
`.pyi` stubs, and those stubs are what any checker reads.

## Why

Asked for: one vendor's toolchain for the Python client rather than two.

## Alternatives rejected

**Keeping mypy alongside ty.** Two checkers means two suppression vocabularies
on the same lines, and `ty` does not understand `# type: ignore[a-mypy-code]`
— it reads the bare `# type: ignore` and its own `# ty: ignore[its-code]`. A
line satisfying both would carry two comments and a reader would not know which
was load-bearing.

**Waiting for ty 1.0.** It is 0.0.82 and says so. The counter-argument is that
it found real things the moment it ran, and that the pin makes the churn a
deliberate act rather than a surprise. Recorded as the main risk below.

**`astral-sh/setup-uv` in CI.** The idiomatic route and it pins two things
whose current versions cannot be checked from this container. A fabricated
action tag is a red build that teaches nothing, so CI does `pip install
uv==0.8.17` — a version verified to exist because it is the one installed
here — and uses `uv pip install --system` from there. Worth revisiting.

## Evidence

**ty found 57 diagnostics where mypy reported clean.** They were not all
findings, and the split is the interesting part:

| | |
| --- | --- |
| 37 in `src/slate/_proto` | generated code; mypy excluded it, ty now does too — configuration, not a finding |
| 11 at the `grpc` boundary | **real**, see below |
| 3 `Sequence.__getitem__` overrides | **real**, and previously suppressed |
| 6 deliberate bad values in tests | suppressions whose spelling changed |

**The `grpc` boundary is the one that matters.** gRPC raises an object that is
both a `grpc.RpcError` and a `grpc.Call`; Python cannot say "both", so the
declared type has neither `code()` nor `details()`. Every client here reads
them anyway. mypy never said so because `grpc.*` was under
`ignore_missing_imports` — the whole package was `Any`, so nothing on that
boundary had been checked *at all*. That is a checker configured not to look,
and it had been that way since the client was written.

The fix is a `@runtime_checkable` Protocol, `slate.errors.RpcCall`. It is a
drop-in for the two `hasattr` calls it replaces because `isinstance` against
such a Protocol tests attribute *presence* and nothing else — which keeps the
duck-typing `tests/test_details.py` depends on, where the fake is not a
`grpc.Call`. `isinstance(error, grpc.Call)` was the tempting fix and is a
different one: a bare `RpcError` is not a `Call`, and neither is that fake.
Checked, not assumed.

One deliberate behaviour change falls out: the two `hasattr` calls were
independent, the Protocol takes `code` and `details` together. An object with
one and not the other now reads as neither. gRPC does not produce such an
object; the choice is pinned by `test_a_half_shaped_failure_is_taken_as_neither`
rather than left implicit.

**Three suppressions became a fix.** `Row`, `JoinedRow` and `Group` declare
`Sequence` and declared `__getitem__(int)`, with `# type: ignore[override]`
recording the mismatch. `Sequence.__getitem__` has to accept a slice, and
slicing always worked because the backing store is a tuple — so the annotation
was a promise narrower than the class. Overloads now, no suppression, and a
test for the capability that had none.

**Three mutations, all killed**: emptying the Protocol kills two tests in
`test_details`; never narrowing kills two in `test_crud`; slicing returning only
the first element kills the new slicing test.

**The checks fire**: a bad annotation exits `ty` 1, an unused import exits
`ruff` 1, both exit 0 clean.

**A defect I introduced, caught by the suite.** The new slicing test took id
`9_100` in the shared `DOCS` table, which `test_deadlines` already owns. It
failed on a full run and passed alone — which is what a "flake" usually turns
out to be. Moved to 9_200 with a note about how ids keep tests apart here.

248 tests pass, `ty check` and `ruff check` clean.

## What this does not do

- **It does not check more than mypy did in aggregate.** `ty` resolves `grpc`,
  which mypy was told to ignore — so that boundary is newly covered. Whether ty
  is stricter *elsewhere* than `strict = true` mypy was is not something this
  change measured, and the honest position is that it is a different checker
  rather than a better one.
- **`ty` is 0.0.82.** Pre-1.0, pinned hard, and the main risk here: its rule
  names are what this package's `# ty: ignore[...]` comments spell, so a rename
  turns a suppression into an error *and* a checked line into an unchecked one.
  Bumping it is a deliberate act.
- **It does not adopt `uv` for anything but CI's install step.** No lockfile,
  no `uv.lock`, no `uv run`. Contributors can still use pip; the README says
  uv because that is what CI does.
- **It does not touch the other Python in this repository.** `scripts/`,
  `site/check/`, `examples/deployed/` and the conformance runner remain
  unchecked and unlinted, as the previous entry recorded.
