# The lowering follows the parser, and the line between them is one sentence

- **Date:** 2026-09-21
- **Author:** Claude Opus 5, F5a stage two
- **Touches:** new `crates/slate-sql/src/lower.rs`, `crates/slate-wasm/src/lib.rs`
- **Kind:** refactor

## What changed

Sixteen functions — 718 lines — from `slate-wasm/src/lib.rs` into
`slate_sql::lower`: `build`, `comparison`, `literal`, `aggregates`, `windows`,
`compute_scalar`, `computes`, `joined_computes`, `chained_computes`,
`joined_ordinal`, `chained_ordinal`, `base_of`, `group_value_type`, `having`,
`aggregate_of`, `conditions`. `slate-wasm` imports the eleven it still calls.

`slate-sql` now does what its name says: SQL in, `slate_kernel::Query` out.
`slate-wasm` is 2,388 lines, down from 3,493 before this pair of commits and
7,384 across the crate.

## Why

The previous commit stopped at the spec on purpose and said so: a crate called
`slate-sql` that cannot produce a `Query` is half of what the name promises,
and `slate-serverd` needs the other half to resolve a view into something the
planner can take.

## The line, because it is the whole judgement

**A function moved if it produces something `slate_kernel` understands** — a
`Query`, an `Expr`, a `Scalar`, an `Aggregate`, a `Window`, a `Value`. **It
stayed if it produces strings for a grid**: `column_header`, `window_header`,
`labels`, `joined_labels`, `joined_names`, `joined_headers`,
`grouped_headers`, `zone_suffix`, `text`, `render`, the two `*_scales`, and
the four keyspace-viewer helpers.

Worth stating because the two kinds sat interleaved and read alike:
`windows` builds a `KernelWindow`, `window_header` builds the text above the
column that window produces, and they were forty lines apart. A daemon
resolving a view needs the first and has no grid to fill.

## Alternatives rejected

**Move the header and label functions too, so all spec-derived code lives
together.** Tempting for tidiness and wrong on the rule: they exist to fill
the workbench's results pane, they return `String`, and nothing outside a UI
wants them. Moving them would make `slate-serverd` depend on a crate that
knows what a column header looks like.

**Split `lower` further — one module per output type.** Premature. 718 lines
in one module with a stated rule is readable; six modules would need six
rules, and nothing is asking for the seams yet.

**Leave the lowering in `slate-wasm` and have `slate-serverd` depend on that.**
The original objection, now with the parser one crate lower — the daemon would
still pull `wasm-bindgen` into its tree to reach `build`.

## Evidence

Again the argument is that nothing changed:

- `cargo test -p slate-wasm -p slate-sql`: **226 passed, 0 failed**, the same
  226 as before the move and before the previous commit, with no test edited.
- `cargo clippy -p slate-sql -p slate-wasm --all-targets`: no warnings.
- `cargo fmt --all -- --check`: clean.
- `sh scripts/check.sh`: 34 passed, all of them.
- `python3 site/check/workbench.py`: green, after rebuilding the wasm (2,441
  KiB raw, 840 KiB gzipped).

The compiler found the one thing reading would have missed: `SortKey` was used
by a moved function and imported by neither crate's new header. It also
flagged four imports `slate-wasm` no longer needs and eight kernel types it no
longer names, all removed.

## What this does not do

- **No view exists yet.** Both obstructions are gone — the crate boundary and
  the missing lowering — and nothing has been built on the far side. That is
  the rest of F5a: `[[views]]` in TOML, substitution before planning, writes
  refused, and the "not a privilege boundary" warning where operators see it.
- **`slate-sql` still has no tests of its own.** All 226 reach it through
  `Playground`, so the lowering is now two crates away from the suite that
  exercises it. The previous entry called this worth fixing once there is a
  second caller; there still is not, and the gap is one commit wider.
- **The re-exports still hide the move.** `slate_wasm::sql` and
  `slate_wasm::QuerySpec` resolve as before; `slate_wasm::build` does not,
  because it was never public. Nothing stops a new caller reaching through the
  binding for the parser.
- **`compute_scalar`, `computes`, `base_of`, `comparison` and `windows` are
  public in `slate-sql` and unused by `slate-wasm`.** They are called from
  within `lower` itself. Public because the module is the seam a daemon will
  use and guessing which five it will not want is how a seam gets reopened —
  but they are unexercised as public API until something outside calls them.
