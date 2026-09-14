# A library crate that had quietly acquired a binary's runtime

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `crates/slate-kernel/Cargo.toml`
- **Kind:** fix

## What changed

`slate-kernel` declares its `tokio` dependency directly instead of inheriting
the workspace one:

```toml
tokio = { version = "1", default-features = false, features = ["time"] }
```

It needs `time`, for a single `sleep` in the retry helper's backoff. It was
getting `rt-multi-thread` as well.

## Why

Feature declarations are additive. `tokio = { workspace = true, features =
["time"] }` does not mean "tokio, with time" — it means "the workspace's tokio,
which asks for `rt-multi-thread`, `macros` and `sync`, *plus* time". So every
consumer of the kernel linked a multi-threaded runtime to support one call to
`sleep`, and the comment sitting directly above the line — "this adds no new
runtime requirement" — had stopped being true without anyone touching it.

This is invisible on a normal build. It compiles, it works, and the extra
runtime is dead weight nobody trips over.

## How it surfaced

Compiling the kernel for `wasm32-unknown-unknown`:

```
error: Only features sync,macros,io-util,rt,time are supported on wasm.
```

Not a wasm problem. wasm is just the first target strict enough to notice that
a library had taken on a binary's dependencies, and it refuses rather than
quietly linking them. The fix is what the kernel should have said all along, so
this is a portability fix that would have been worth making with no browser
anywhere in the picture.

The other blocker on that build was `uuid` needing a randomness source on
wasm32, which is genuinely wasm-specific and belongs in whatever crate targets
wasm rather than here.

## Alternatives rejected

**Narrowing the workspace `tokio` to minimal features and adding
`rt-multi-thread` in each crate that needs it.** Arguably the more correct
shape — a workspace default that suits binaries is a trap for every library in
the workspace — and rejected for now because it touches `slate-slatedb`,
`slate-server`, `slate-serverd` and `slate-headbench`, all of which genuinely
want the multi-threaded runtime, in exchange for no behaviour change at all.
One line in the crate that was actually wrong is the proportionate fix. If a
second library crate hits this, that is the moment to change the default.

**`default-features = false` without saying why.** The flag looks like a
size optimisation and is not; without the comment the next person inherits the
workspace dependency back for consistency and reintroduces the problem. The
comment is longer than the line it explains, deliberately.

**Leaving it and adding a wasm-only patch.** A `[patch]` or a target-specific
override would have unblocked the browser build and left the kernel still
claiming a runtime it does not use. The bug is in the declaration.

## Evidence

`cargo test -p slate-kernel --no-fail-fast` passes natively with the change —
the dependency was unused, so nothing observable moves.

`cargo build --target wasm32-unknown-unknown` against a probe crate depending
on `slate-kernel`, `slate-schema` and `slate-tuple` (with `uuid`'s `js`
feature) exits 0. Before the change it failed on the tokio `compile_error!`
above; before that, on uuid's.

That probe is the first demonstration that the record layer's kernel and its
in-memory store run in a browser at all.

## What this does not do

Nothing else in the workspace is checked for the same problem. The other
library crates — `slate-tuple`, `slate-schema` — do not depend on tokio, and
the rest are binaries or backends that want the full runtime, but "I looked and
these are fine" is weaker than a check, and there is no check.

The wasm target is not in CI as of this commit, so nothing stops the kernel
reacquiring a non-wasm feature tomorrow. That arrives with the crate that
actually targets wasm; until then this fix is unguarded.
