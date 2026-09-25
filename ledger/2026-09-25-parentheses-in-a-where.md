# `WHERE (a = 1 OR b = 2) AND c = 3` works. The spec grows a fourth field rather than replacing two, and adding it exposed a subquery inside a bracket that would have matched nothing in silence.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `crates/slate-sql/src/{lib,sql,lower}.rs`, `crates/slate-sql/tests/front_end.rs`, `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/{sql,examples}.rs`, `site/workbench.js`, `site/docs/limits.html`
- **Kind:** feature

## What changed

The single-table `WHERE` parses as a tree: `primary → conjunction →
disjunction`, with `'(' disjunction ')'` as a primary. `QuerySpec` grows
`predicate: Option<PredicateSpec>`, a three-variant recursive enum, and
`lower::predicate` turns it into the kernel's `Expr`, which has been a tree all
along.

The parser **flattens before it stores**: an all-`AND` clause still lands in
`filters` and an all-`OR` one in `any_of`, brackets or not, so
`WHERE (year >= 1970)` produces the spec `WHERE year >= 1970` does, byte for
byte. `predicate` is `Some` only for a shape neither flat list can hold.

An unbracketed mixture is **still refused**. The message names the fix.

## Why

`ledger/2026-09-25-the-disjunction-the-kernel-always-had.md` recorded `No
parentheses, so no nesting`, and `QuerySpec::any_of`'s own doc comment argued
against a tree: it "would make the Spec tab, the panel and every consumer of
this shape handle a recursive form for a query nobody has yet asked to write."

That argument is good and it had a hole. The recursion does not have to
*replace* the flat lists; it can be a field that is `None` for every query
anyone writes, which costs the flat consumers exactly nothing. `IN (…)` already
covers the common bracketed case — one column, several values — so what was
genuinely unreachable is a disjunction whose arms differ in *which* column they
name: `(author_id = 1 AND year >= 1990) OR (author_id = 2 AND year < 1950)`.
The kernel has evaluated that since expressions existed.

## Alternatives rejected

**Re-triage it as `deliberate` and cite the field comment.** The comment is
twelve hours old and argued by me, which is the weakest possible authority for
leaving something undone. Reading it again found the hole in it.

**Make the tree the only representation.** Tidier, and it rewrites every spec
JSON on disk, every test that compares one, the Spec tab, the panel, the
round-trip property and `slate-serverd`'s view resolution — to express
`WHERE year >= 1970`, the query everybody writes, as `{"of": {...}}`. The cost
of the fourth field is that two fields can describe one thing, which this
repository is usually right to distrust; what makes it safe is that the parser
never populates both and
`a_flat_where_never_lands_in_the_nested_field` asserts it over seven spellings
including `((year >= 1970))`.

**Allow an unbracketed mixture now that a precedence exists.** SQL's precedence
is unambiguous and the reader's is not: `a AND b OR c` reads as
`a AND (b OR c)` to about half of everyone. The refusal is *more* defensible
now than when it was written, because the fix is one keystroke and the message
says so.

**A `Group` node in the spec, so parenthesisation survives into the JSON.** It
would make `((a))` a different spec from `a`, which is a difference no consumer
wants and every consumer would have to strip. Parenthesisation is a parse-time
fact and stays one.

**Brackets in a `HAVING` too.** Its group-condition lists have no nested form
to lower onto, so it would mean a second `PredicateSpec` field on three specs
and a second lowering. Nobody has written the query. Refused for now and
recorded below.

## Evidence

**A defect found by building it.** `Playground::resolve_subqueries` walked
`spec.filters` and nothing else. A subquery inside a bracket lands in
`predicate`, that loop cannot see it, and `build` turns an unresolved subquery
into `IN` over an *empty list* — no rows, no error, and no way to tell it from
a query that legitimately matched none. It walks the tree now, and `any_of`
too, which the parser cannot populate with a subquery today and which was
exactly the asymmetry that made this a bug once.

**A test that passed for the wrong reason, caught by `mutate.py`.** The first
version of `a_subquery_inside_a_bracket_is_still_resolved` used
`(subquery OR year >= 1990)`; the second arm admits rows on its own, so the
test passed with the fix reverted. It uses `OR author_id = 9999` now — an arm
that matches nothing — and asserts the answer equals the flat form's. A
disjunction whose other arm can carry the answer cannot test the arm you mean,
and the mutation run is the only reason that is not still shipped.

**Redundant code found the same way.** `primary` returned "was this
bracketed?" alongside its tree, which reads like the natural way to know.
Flipping it to `false` changed nothing: the flag was never read, because the
fact that a bracketed part was parsed by a *different* recursive call already
carries it. Deleted, with the reasoning in the function's comment.

**A gap in this crate's own coverage.** Mutating the flattener's `Any` branch
to return an empty list survived: every nesting case in `slate-sql`'s suite
nested under an `AND`, and the `OR`-of-`AND`s case existed only in
`slate-wasm`'s round trip. "Covered somewhere" is not covered here;
`two_bracketed_conjunctions_ored_together` closes it.

**Mutations**, eleven run in total, eleven caught after the two findings above
were fixed:

| mutation | caught by |
|---|---|
| a bracketed group is not treated as grouped | *survived* — redundant code, deleted |
| a conjunction that combined nothing is marked bare | four cases, including the flat-spec invariant |
| the mixing refusal never fires | three refusal cases |
| the closing `)` is optional | `an_unclosed_bracket_is_refused_at_the_bracket_and_not_swallowed` |
| the flattener drops a nested `All` | four cases |
| the flattener drops a nested `Any` | *survived* — missing case, written |
| `PredicateSpec::All` lowers to `Expr::Or` | three cases |
| `PredicateSpec::Any` lowers to `Expr::And` | three cases |
| the nested predicate is not ANDed onto the query | three cases |
| `resolve_subqueries` skips the tree | *survived once* — the test was wrong; caught after |

**Suites.** `cargo test -p slate-sql`: 4 + 33 passed (4 + 24 before).
`cargo test -p slate-wasm`: every binary green, 19 `test result: ok` lines, no
failures — including `examples.rs`, whose kitchen-sink assertion was rewritten
because the workbench's third statement is now a bracketed mixture rather than
a flat `OR`. `cargo clippy --workspace --all-targets`: zero warnings.
`sh scripts/check.sh`: 53 passed, all of them.

## What this does not do

**No brackets in a `HAVING`**, on any shape. Rejected above; the group
conditions stay two flat lists and a mixture is refused bracketed or not.

**No brackets in a join's `WHERE`**, which still takes `AND` only. Its
conditions are split by side so each scan is narrowed before the hash join
runs, and neither a disjunction nor a tree spanning both sides can be split
that way. That is the same reason `OR` does not reach it, unchanged.

**The round-trip property does not generate a nested predicate.** Generating
one means generating the brackets, and a generator that emits `(a AND b) OR c`
is a second implementation of the precedence this front end refuses to have.
Three hand-written shapes round-trip instead, and the renderer brackets every
branch rather than deciding where brackets are needed — for the same reason.

**No client can send a nested predicate as SQL**, because no client sends SQL:
this is the browser front end and `slate-serverd`'s view resolution. The
clients build `Expr` directly and have had `Or` and `And` all along, so there
is nothing missing there — a thing I have now written down four times today
and would rather be checkable than repeated.

**The planner still does nothing special with it.** A nested predicate is a
residual filter evaluated per row, exactly as a flat disjunction is. Whether a
bracketed `OR` over an indexed column could become a union of ranges is the
open question `The planner does nothing with a disjunction` already records,
and this widens its surface without answering it.
