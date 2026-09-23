# The SQL front end is its own crate, and the parser moved without changing

- **Date:** 2026-09-21
- **Author:** Claude Opus 5, F5a stage one
- **Touches:** new `crates/slate-sql`, `crates/slate-wasm`, root `Cargo.toml`
- **Kind:** refactor

## What changed

`crates/slate-sql`: the SQL parser (`sql.rs`, moved with `git mv`, unedited
but for nothing) and the query spec types it compiles to — `QuerySpec` and its
nine neighbours, cut out of `slate-wasm/src/lib.rs`. `slate-wasm` depends on
it and re-exports both, so every caller, test and site file keeps the names it
had. 454 lines left `slate-wasm`; 28 arrived, all of them the re-export and
its explanation.

No behaviour changed and none was meant to.

## Why

`docs/views.md` recorded a `[[views]]` block holding SQL as architecturally
blocked: it "would make the daemon's schema loader depend on the SQL front
end, which lives in `slate-wasm` — a dependency direction nothing has needed
yet". Yesterday's entry measured that claim and found it was about the crate's
name: 7,384 lines, 5 mentioning `wasm_bindgen`, and `sql.rs` mentioning none.

This is that measurement acted on. `slate-wasm` and `slate-serverd` can now be
peers over a shared crate rather than one reaching into the other, which is
what a view needs to be writable as the SQL it is.

## Alternatives rejected

**Move the lowering too, and finish the crate in one commit.** `build`,
`comparison`, `literal` and the rest — `QuerySpec` to `slate_kernel::Query` —
are still in `slate-wasm`, and `slate-serverd` will need them. They are also
interleaved with the header and label functions that exist to fill a grid, and
separating those is a judgement call per function rather than a cut. Doing it
here would have turned a mechanical move that the test suite can vouch for
into a mechanical move plus a dozen decisions, in one diff. Next commit.

**Re-point every caller at `slate_sql::` instead of re-exporting.** It would
be tidier and it would touch every test file and the site, turning a move into
a rename with a much larger diff and no way to tell the two apart in review.
The re-export says "this is where it lives now" in one place; the call sites
can follow later or never.

**A `sql` feature on `slate-wasm` rather than a crate.** Features do not fix
direction: `slate-serverd` would still depend on the browser binding, just on
less of it, and would pull `wasm-bindgen` into the daemon's tree.

## Evidence

The move is verified by not changing anything:

- `cargo test -p slate-wasm -p slate-sql`: 226 tests, every suite green, and
  **not one test edited**. That is the whole argument — the parser's tests all
  run through `Playground`, so they exercise the moved code across the new
  crate boundary exactly as before.
- `cargo clippy -p slate-sql -p slate-wasm --all-targets`: no warnings.
- `cargo fmt --all -- --check`: clean.
- `sh scripts/check.sh`: 34 passed, all of them — including the workspace
  layout guard, which the new member had to satisfy.
- `python3 site/check/workbench.py`: the workbench runs the kernel in a
  browser. Run because this moved the code the workbench runs; the wasm was
  rebuilt first (2,434 KiB raw, 840 KiB gzipped, against 2,418 before).
- `python3 site/check/docs.py`: the docs site holds together.

Two structs came along in the first cut and were sent back: `Answer` and
`PlanInfo`, which render results for the UI and sat interleaved with the spec
types. The compiler caught both as never-constructed, which is why the cut was
made by slicing and compiling rather than by reading.

## What this does not do

- **The lowering has not moved.** `slate-sql` parses SQL into a `QuerySpec`
  and cannot turn one into a `slate_kernel::Query`; that is still
  `slate-wasm`'s. A crate called `slate-sql` that stops there is half of what
  the name promises, and the next commit is the other half.
- **No view exists yet**, and no `[[views]]` block. This removes the
  obstruction; it builds nothing on the far side of it.
- **`slate-sql` has no tests of its own.** All 226 live in
  `crates/slate-wasm/tests/` and reach the parser through `Playground`. They
  are real tests of this code and they are not *its* tests: a change that
  broke the parser for a non-browser caller — a daemon, say — would be caught
  only if the browser happened to care. Worth its own suite once there is a
  second caller to write one against.
- **The re-export means nothing has been forced to move.** `slate_wasm::sql`
  still resolves, so a future caller can keep reaching through the binding
  without noticing there is a crate underneath. Nothing guards against that.
