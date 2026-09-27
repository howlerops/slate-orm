# A script running a pinned `go install` beside a pinned `go-version` must say what `GOTOOLCHAIN` is. Inheriting it is how a workflow nobody edited went red.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_toolchain_pins.py`, `scripts/test_check_toolchain_pins.py`, `scripts/check.sh`, `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** correctness

## What changed

A new guard: if any workflow pins `go-version`, every file in this repository
that runs `go install <pkg>@<version>` must mention `GOTOOLCHAIN`. Both
never-fires halves, nine fixture cases and the real tree.

Registered in `scripts/check.sh` and `.github/workflows/ci.yml`; 106 CI steps,
all accounted for.

## Why

`ledger/2026-09-27-setup-go-v6-pins-gotoolchain-and-the-generator-needed-1-25.md`
recorded the defect and then recorded what it could not close:

> **Nothing checks that a pinned generator's Go floor is met.** The pin and
> the `go-version` in `.github/workflows/ci.yml` are two numbers in two files
> with a relation between them that only a failing run states. A guard reading
> the plugin's `go.mod` off the proxy would be a network call in a static
> check.

That framing was the obstacle. Comparing the two *numbers* needs the module
proxy and is out of reach; the thing that actually broke needs neither. The
generator never named `GOTOOLCHAIN` at all, so its behaviour was whatever
`actions/setup-go` currently defaults to — and the default changed under a
file nobody had edited. What is checkable locally is the **inheritance**, not
the arithmetic.

## Alternatives rejected

**Require `GOTOOLCHAIN=auto` specifically.** Narrower and wrong. A script that
sets `local` has decided its pins must fit the workflow's Go, which is a
coherent position and the one a project wanting reproducible builds would take.
What is refused is saying nothing.

**Read the plugin's `go.mod` from the proxy and compare floors.** The direct
check, and it needs a network call `scripts/check.sh` does not make — the
script's whole value is that it needs no binary, no browser, no container and
no network. That property is also the one CI checks for free, loudly, on the
first run after a pin moves. What CI cannot catch is the silent case: a job
working today because of a default.

**Put the rule in `check_handlers.py` or another existing guard.** Every guard
in `scripts/` is one subject, and `check_guard_scope.py` now checks that each
docstring's account of what it reads matches what it reads. A rule about
workflows inside a guard about gRPC handlers would be a scope claim nobody
could make true.

**Leave it, since the entry before this fixed the one occurrence.** The
occurrence is fixed. The class is "a behaviour inherited from an action's
default", and the reason it waited weeks is that nothing named it. This is a
guard against the second one.

## Evidence

**It reports the defect as it was.** The case
`a pinned workflow and an installer that inherits is reported` writes exactly
the pre-fix shape — a pinned `go-version:` in a workflow and
`subprocess.run(["go", "install", "example.com/cmd/x@v1.2.3"])` with no
`GOTOOLCHAIN` — and the guard names the file, the package and the workflow.

**Three distinctions it gets right, each with a case.** Setting `GOTOOLCHAIN`
to `local` passes, because the rule is about inheriting rather than about the
value. A shell `go install` is read as well as a Python argument list, since
the pattern matches the `@version` in either. `go install ./...` is not a
pinned install: it builds this module and cannot want a Go the module does not
declare.

**Today's tree**: `5 pinned go-version across 1 workflow(s), 1 pinned go
install target(s) in 1 file(s), every one of them deciding GOTOOLCHAIN rather
than inheriting it`.

**Mutations.** Two runs, six cases, no survivors:
`ledger/mutations/20260927T012857-scripts-check-toolchain-pins-py.json` (six
cases sent, three scored and three refused for a bad anchor) and
`ledger/mutations/20260927T012910-scripts-check-toolchain-pins-py.json` (those
three again, with anchors read out of the file).

| mutation | outcome |
| --- | --- |
| an installer that inherits is not reported | caught, 2 cases |
| the pinned-workflow never-fires half never fires | caught, 2 cases |
| the no-installer never-fires half never fires | caught |
| `go-version-file` counts as a pin | caught |
| an unpinned `go install` counts as pinned | caught, 2 cases |
| vendored trees are scanned after all | caught |

The fourth is worth the row: `go-version-file: clients/go/go.mod` is *not* a
pin — `setup-go` reads the version from the module and does not force
`GOTOOLCHAIN=local` — so a pattern loose enough to match it would make the
rule fire on a tree where its premise is false.

**The first attempt at three of these could not be scored.** `mutate.py`
refused them with "the text to replace occurs 0 times": the anchors were
written from memory with the wrong backslash escaping, which is the first of
the six lies its docstring lists. Re-derived by reading the lines out of the
file, they all applied. Worth recording because writing an anchor by hand and
writing it by `pathlib.read_text()` look identical in a session transcript and
only one of them is checked.

**Suites.** `scripts/test_check_toolchain_pins.py` 10 passed 0 failed.
`scripts/test_check_sh.py` 4 passed, 106 CI steps accounted for.
`sh scripts/check.sh` 71 of 71.

## What this does not do

**It does not compare the two version numbers.** A pinned tool whose floor is
higher than the workflow's Go still fails in CI, and that is the intended
division: CI catches the arithmetic, this catches the silence. If the module
proxy ever becomes something `check.sh` may call, the arithmetic is one
`go list -m -f '{{.GoVersion}}'` away.

**It is Go only.** `cargo install wasm-bindgen-cli --version 0.2.128 --locked`
is the same shape — a pinned tool beside a pinned toolchain — and `rustup`
has no `GOTOOLCHAIN` equivalent to inherit, so there is nothing analogous to
check. Python's pins are in `pyproject.toml` and are resolved by pip against
`requires-python`, which is a different mechanism again. Stated so that the
guard's name is not read as a promise about the other two.

**A file could mention `GOTOOLCHAIN` in a comment and satisfy it.** The rule
is "this file has decided", and a decision written as prose next to code that
does not set it would pass. Matching an assignment instead would mean parsing
four languages' ways of setting an environment variable; the looser rule is
the one whose failure mode is a file that thought about the problem rather
than one that did not.

**Nothing checks the other direction — an action bump.** If `setup-go@v7`
stops setting `GOTOOLCHAIN=local`, every `GOTOOLCHAIN` this guard demanded
becomes unnecessary and nothing says so. That is the general "an action's
defaults are part of your toolchain" problem `CLAUDE.md` now names, and this
guard is one instance of it rather than an answer to it.
