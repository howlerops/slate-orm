# The question blocking views was answered by counting five lines

- **Date:** 2026-09-21
- **Author:** Claude Opus 5
- **Touches:** `docs/views.md`
- **Kind:** docs

## What changed

`views.md`'s *Open, and deliberately not decided here* named one question as
blocking: where a view gets declared. It said a `[[views]]` block holding SQL
"would make the daemon's schema loader depend on the SQL front end, which
lives in `slate-wasm` — a dependency direction nothing has needed yet". That
paragraph is replaced with the measurement that answers it.

## Why

Views are the largest remaining row in `orm-comparison.md`'s gap table, they
are designed down to the RLS composition, and the note said one packaging
question stopped the build. Before doing anything else I checked whether the
objection was about the code or about the crate it sits in.

It was about the crate:

| | lines |
| --- | --- |
| `crates/slate-wasm/src/` | 7,384 |
| mentioning `wasm_bindgen` | **5** |

One `use`, one `inline_js` shim for `performance.now()`, and two attributes on
`Playground` and its constructor. `sql.rs` is 3,402 of those lines and
mentions none — it imports `QuerySpec` and its neighbours from the crate root,
`TableDef` from `slate-schema`, `ValueType` from `slate-tuple`, and nothing
else. The one genuinely wasm-specific thing in the manifest is a
target-scoped `uuid` feature, already commented as such.

So the SQL front end is not browser code a daemon would reach into. It is
portable Rust housed in the binding crate because the binding needed it first.
Extract `slate-sql` and `slate-serverd` depends on the parser exactly as the
browser does — peers over a shared crate, no new direction.

The objection was real as written and wrong in what it implied, and the thing
that made it look architectural was the crate's *name*.

## Alternatives rejected

**A `QuerySpec` in TOML.** The note's own alternative, and its judgement that
it is unreadable still holds: a view is a query and the readable way to write
a query down is SQL. Nothing about the measurement changes that half.

**Build views against the Rust builder only, and leave TOML out.** It would
sidestep the packaging question entirely — and ship a feature no
`slate-serverd` operator can use, which is every operator the daemon has. The
TOML surface is the point.

**Leave the note as it stood and build anyway.** Then the note says building
is blocked while the build proceeds, which is worse than either. The note is
where the next person looks.

**Do the extraction now, in this entry.** It is a seven-thousand-line move
across a crate boundary with a wasm build, a browser check and CI's
workspace-layout guard downstream of it. It is ordinary work and it is not
small; conflating the decision with the refactor would make both harder to
review. Filed as its own task instead.

## Evidence

The counts above, from `wc -l crates/slate-wasm/src/*.rs` and
`grep -n wasm_bindgen crates/slate-wasm/src/lib.rs`, both quoted in the note.
`python3 site/check/docs.py`: the docs site holds together.

No code moved and nothing was built, so there is nothing else to measure. The
claim this entry makes is about where lines are, and it is checkable by
re-running those two commands.

## What this does not do

- **It builds no view.** The security design was already settled; what was
  missing was a declaration surface, and what is fixed is the belief that
  there could not be one.
- **It does not prove the extraction is clean.** Five `wasm_bindgen` lines say
  the *binding* is thin; they do not say `Playground` separates cleanly from
  the parser, and `lib.rs` mixes the spec types with kernel execution. The
  refactor may find a seam that is not where it looks. That is a claim for the
  task that does it, with the compiler as the evidence.
- **It leaves the other two open questions open**, untouched: what `EXPLAIN`
  over an expanded view reveals, and whether a view may reference another one.
