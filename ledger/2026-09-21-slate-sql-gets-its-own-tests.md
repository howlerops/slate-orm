# `slate-sql` gets tests that do not go through a browser

- **Date:** 2026-09-21
- **Author:** Claude Opus 5, F5a stage three
- **Touches:** new `crates/slate-sql/tests/front_end.rs`
- **Kind:** fix

## What changed

Seven tests against `slate-sql`'s own surface: `sql::parse` text to a
`QuerySpec`, `lower::build` that spec to a `slate_kernel::Query`, and the two
composed. They build a `TableDef` by hand and mention no browser.

## Why

The two previous entries both recorded this and neither fixed it: every test
of this code — 226 — lives in `crates/slate-wasm/tests/` and reaches it
through `Playground`. They are real tests of the parser and they are not
*its* tests. A change that broke a non-browser caller would be caught only if
the browser happened to care about the same thing, and the second entry noted
the gap had got one commit wider, because the lowering is now two crates from
the suite exercising it.

The earlier entries said this was worth doing "once there is a second caller".
That was the wrong trigger and this corrects it: the right time is *before*
the second caller arrives, because the point of a seam's own tests is to say
what the seam promises before something starts depending on it. `slate-serverd`
is that caller, and the two functions this file pins are exactly the two a
view resolves through.

The file also pins something by compiling at all. It builds its own table and
never names `slate_wasm`, `Playground` or a fixture — so if the crate ever
grew a dependency back on the binding, this would stop building. The
extraction claimed the front end is portable; a test that could only run
inside the binding would not be evidence of that.

## Alternatives rejected

**Move some of the 226 down from `slate-wasm`.** They test SQL *through the
playground*, including headers, timings and the compiled-spec panel, none of
which exist here. Splitting each one into the part that belongs in each crate
is a large edit to a suite whose value is that it has not changed across two
moves — and the previous two entries leaned on exactly that.

**A property test over generated SQL.** `slate-wasm`'s `sql.rs` suite already
has one (`sql_and_the_spec_return_the_same_rows_and_the_same_plan`) and it is
better placed there, where there is an executor to compare against. What was
missing here is not more coverage of the parser; it is any statement at all of
what the crate's *public* entry points do.

**Wait until the daemon exists and test through it.** That is the trigger this
entry argues against, and it has the failure this repository keeps meeting: a
test that reaches the code through its newest caller tests the caller.

## Evidence

Seven tests, and four mutations of `lower.rs`, all caught:

- `build` keeping only the first of several conditions → `two_conditions_lower_to_a_conjunction`
- `comparison` resolving every filter to ordinal 0 → two tests
- `literal` ignoring the column's type → `a_spec_lowers_to_a_kernel_query_with_that_filter`
- `ge` lowering to `CmpOp::Gt` → the same

`cargo clippy -p slate-sql --all-targets` clean, `cargo fmt --all -- --check`
clean, `sh scripts/check.sh` 34 passed.

**A fifth mutation survived and should have.** Breaking `conditions()` — which
also folds a `Vec<FilterSpec>` into an `Expr` — changed nothing, because
`build` does not call it: it assembles its own filter from `spec.filter` and
`spec.filters`. Two functions doing the same fold by different routes is worth
knowing about and is not this file's to fix; `conditions` serves the predicate
write path, which has its own tests elsewhere.

## What this does not do

- **It covers the read path and nothing else.** `Statement::Join`,
  `Statement::Chain`, the write statements, aggregates, windows and computes
  all go untested here. A view is a single-table read (`views.md`), so this
  covers what the next caller needs and stops; the browser's 226 cover the
  rest, through the browser.
- **`Schema<'_>` is the only way in, and it takes a slice of `TableDef`.** How
  a daemon gets that slice — from its TOML catalog — is not tested, because
  there is no daemon-side code yet.
- **Nothing checks the crate builds without `slate-wasm` in the workspace.**
  The independence claim rests on this file not naming it, which a reader can
  verify and a compiler cannot. A real check would build the crate alone in a
  scratch workspace, which is more machinery than the claim is worth today.
