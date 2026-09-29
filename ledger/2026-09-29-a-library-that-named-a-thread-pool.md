# A library that named a thread pool, and the half no build could catch

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `scripts/{check_wasm_runtime.py,test_check_wasm_runtime.py,check.sh}`, `.github/workflows/ci.yml`
- **Kind:** guard

## What changed

`scripts/check_wasm_runtime.py`, two rules over the workspace's manifests:

1. Every crate `slate-wasm` links must ask tokio for features
   `wasm32-unknown-unknown` supports. The closure is **computed** from the
   manifests — five crates today — not listed.
2. Every library crate *outside* that closure taking a non-dev tokio must
   appear in `RUNTIME_ON_PURPOSE` with a reason. Three do:
   `slate-slatedb`, `slate-server`, `slate-headbench`.

Ten cases in `test_check_wasm_runtime.py`, both in `check.sh` and in CI.

## Why

`ledger/2026-09-14-the-kernel-on-wasm.md` left this:

> Nothing else in the workspace is checked for the same problem. The other
> library crates — `slate-tuple`, `slate-schema` — do not depend on tokio, and
> the rest are binaries or backends that want the full runtime, but "I looked
> and these are fine" is weaker than a check, and there is no check.

Half of it has since been answered by work nobody connected back to it: CI's
playground job builds `slate-wasm` for `wasm32-unknown-unknown` on every push,
so every crate in that closure is checked by a compiler, not by a reading. That
is the stronger check and this file does not replace it.

**The other half is what the caveat is actually about, and no wasm build can
reach it.** `slate-kernel` was outside every wasm build on the day it was
wrong: it declared `tokio.workspace = true` — `rt-multi-thread`, `macros`,
`sync`, chosen for the binaries — used none of it, and nothing anywhere
objected. What made it wrong was not the target. It was a library making a
runtime decision that belongs to whoever embeds it, and the browser was merely
the first environment rude enough to say so.

So rule 2 is the point and rule 1 is the cheap pre-check. Three library crates
inherit the workspace default today, and the honest answer for each is that its
whole job is to hold a listener, a lease or a load generator — which is a
sentence a person writes once, not a rule a script infers.

## Alternatives rejected

**Just point CI at more wasm builds.** `cargo build -p slate-orm --target
wasm32-unknown-unknown` and so on for every library. Measured before rejecting:
`slate-orm` fails, on `uuid` wanting a randomness source — the blocker the
original entry calls "genuinely wasm-specific and belongs in whatever crate
targets wasm". So the build would go red for a reason that is not the hazard,
and the fix would be to add a `js` feature to a crate nobody compiles for a
browser. A build answers "does this target wasm", and the question is "did a
library take a dependency it does not need".

**Assert the feature list of every crate directly.** A table of crate → allowed
features, checked exactly. It would have caught the kernel, and it makes every
legitimate feature change a two-file edit, which is how a roster becomes a
thing people route around. The rule here only fires on the shape that was
wrong.

**A list of "portable crates" maintained by hand.** Rejected for the reason the
transport-door guard's roster was: it would report clean on the day a crate
joined the wasm build carrying a runtime. Walking `slate-wasm`'s dependencies
is nine lines and cannot go stale.

**Deriving the wasm crate rather than naming it.** `crate-type = ["cdylib"]` is
the nearest signal and means several other things; there is exactly one such
crate and naming it costs one constant and a loud failure if it is renamed.

**Narrowing the workspace `tokio` to minimal features.** The original entry
rejected this and said "if a second library crate hits this, that is the moment
to change the default". No second crate has hit it — the three that inherit are
servers by construction — so the moment has not arrived, and this records the
condition rather than pre-empting it.

## Evidence

- `python3 scripts/check_wasm_runtime.py`: **ok, 5 crates in the closure, 3
  libraries outside it hold a runtime on purpose.**
- `python3 scripts/test_check_wasm_runtime.py`: **10 passed, 0 failed.**
- `cargo build -p slate-wasm --target wasm32-unknown-unknown`: **Finished**,
  24.5s, compiling `slate-tuple`, `slate-schema`, `slate-sql`, `slate-wasm` —
  the same five the closure computes. The static rule and the compiler agree
  about which crates are in scope.
- `cargo build -p slate-orm --target wasm32-unknown-unknown`: **fails**, on
  `uuid`'s `compile_error!` about randomness, not on tokio. Recorded because it
  is the measurement that rejected the "just build more crates" alternative.
- **Ten distinct mutations, twelve scored runs across three files.** Eight
  were caught on the first attempt; one could not score and one survived,
  and both are caught now. The first run
  (`ledger/mutations/20260929T040724-scripts-check-wasm-runtime-py.json`,
  outcome `interrupted`, 8 cases) broke off at the ninth because its anchor had
  moved under `ruff format` — the script refused to run the suite against
  unmutated code, which is the first of the six lies its own docstring lists.
  The re-runs
  (`ledger/mutations/20260929T040755-scripts-check-wasm-runtime-py.json`,
  3 cases, and
  `ledger/mutations/20260929T040857-scripts-check-wasm-runtime-py.json`,
  1 case) are both `clean`. The two mutations that appear twice are the two
  that did not score first time, re-run after the fixes below — which is why
  twelve runs cover ten mutations. Each caught mutation names the case it
  broke: removing the inherit check, the feature check, the
  outside-the-closure rule, the library/binary distinction, both
  exemption-staleness rules, the closure-of-one arm, the missing-crate arm,
  walking target tables, and counting dev-dependencies as dependencies.
- **A survivor that was a missing case, and a mutation that could not score at
  all.** Skipping the target-scoped dependency tables changed nothing, because
  the only such table in the fixture holds `uuid` — not a workspace crate and
  no tokio. `a_workspace_crate_behind_a_cfg` is that case now, and it has to be
  contrived: the crate asks for a refused feature *and* sits in the roster,
  because that is the only shape where the two rules give different answers.
- **And the harness could not report on a guard that raised.** Making the
  missing-wasm-crate arm unreachable left the closure walk to `KeyError`, which
  killed the test run before it printed a tally — `mutate.py` read that as
  NOTHING RAN rather than as a survivor. `run()` turns a raise into a failing
  case now. Same lie `test_conformance.py` was fixed for two days ago, met
  again in a file written from scratch this hour.
- `sh scripts/check.sh`: 76 passed. `python3 scripts/test_check_sh.py`: 115
  steps accounted for.

## What this does not do

**It reads manifests, not features as resolved.** Cargo unifies features across
a dependency graph: a crate can receive `rt-multi-thread` because something
*else* in the build asked for it, and no manifest says so. `cargo tree -e
features` would show that and is not run here. The wasm build in CI is what
catches it, which is the division of labour this file is built around — and it
means a green run here is necessary and not sufficient, the same way a green
local clippy is.

**`RUNTIME_ON_PURPOSE`'s reasons are unchecked prose.** The rule enforces that
a reason exists and that its crate is still real and still outside the closure.
Whether the reason is true is a reader's job, and all three are one sentence
each so that a reader can do it.

**It only knows about tokio.** `async-std`, a thread pool, a crate that spawns
its own — none is recognised. tokio is the one this repository uses and the one
the incident was about; a second runtime arriving would need this widened, and
nothing would say so.

**Nothing checks the wasm build's own feature list stays right.** `ON_WASM` is
copied from tokio's `compile_error!`. If tokio adds a wasm-supported feature
this refuses something that now compiles — loud, and wrong. If it *removes*
one, this passes something that no longer compiles and the CI wasm build
catches it. The failure directions are unequal and that is the safer way round,
but neither is verified against the tokio in `Cargo.lock`.
