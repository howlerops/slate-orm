# Nested eager loading, and a depth limit with nothing to limit

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `crates/slate-orm/src/relation.rs`, `crates/slate-orm/tests/many_to_many.rs`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`load_nested` returns every parent's children, each paired with its own
children, in two reads. `load_related_through` is now expressed on top of it —
the same call with the middle discarded — so the regrouping exists once.

Four tests added, five mutations, no survivors.

## Why

The two functions differ in one respect: whether the intermediate rows are kept.
Written separately they were two copies of the same count-flatten-regroup, which
is two places for an off-by-one to live and two places to fix it. `through` is
now `nested` plus a `flat_map`.

**The depth limit the plan asked for is not here, and building it would have
been building a guard against something unreachable.** The item said "one level
of nesting, then a depth limit with a named refusal rather than unbounded
recursion". There is no recursion. Each level is a *type parameter*:
`load_nested<S, P, C, G>` is two levels, a third would be a five-parameter
function that does not exist, and a caller cannot request depth 1000 because
depth is not a value they pass. A limit that no input can exceed is a limit
nobody maintains and nobody tests.

The concern is real for a *different* API. An `include` list on the wire —
`["comments.author.employer"]` — carries a depth chosen by the request, and that
form needs exactly the refusal described. Nothing in this repository parses one,
so the note is recorded beside `load_nested` rather than implemented against
nothing.

## Alternatives rejected

**A runtime `depth` parameter, so the limit has something to check.**
`load_nested(store, ctx, parents, depth)` would make the plan implementable as
written. It also makes depth dynamic, which means the return type cannot name
the levels — it becomes a tree of `Box<dyn Any>` or a JSON-ish value, and every
caller loses the types that make the current shape checkable. Inventing the
unsafety in order to add the guard against it is the wrong order.

**Keep `load_related_through` as its own implementation.** Less churn on code
committed an hour ago, and the two would stay independent if one later needed a
different grouping. They do not: the middle is the only difference, and
`through_is_nesting_with_the_middle_discarded` asserts the relationship that
the implementation now guarantees.

**Return `Vec<Vec<(C, Vec<G>)>>` versus a dedicated struct.** A struct with
named fields (`child`, `children`) reads better at a call site than `.0` and
`.1`. It is also a third public type for a shape that is a pair, and the tuple
destructures cleanly (`for (join, tags) in ...`). Left as a tuple; if a third
element ever joins it, that is when it earns a name.

## Evidence

Ten tests in `many_to_many.rs` (six for the many-to-many, four for nesting).
`cargo fmt --all` clean, and `clippy --workspace --all-targets` with
`-D warnings` clean — the workspace command rather than `-p`, because the
first run of the narrower one passed and the workspace one failed:
`indexing_slicing` on the test module, which every other test file in this
crate allows at the top and this one did not. Exactly the "green local clippy
is necessary rather than sufficient" case `CLAUDE.md` describes, caught here
rather than in CI only because the workspace command was run before pushing.

Five mutations on the nesting logic, no survivors:

| mutation | caught by |
| --- | --- |
| the pairing is off by one child | `a_child_with_no_grandchildren_is_still_paired`, and two more |
| a child with no grandchildren is dropped | `a_child_with_no_grandchildren_is_still_paired` |
| `through` loses the far rows' order | `through_is_nesting_with_the_middle_discarded`, and two more |
| every parent gets one child | `a_many_to_many_needs_no_new_declaration`, and two more |
| every child paired with the first's grandchildren | did not compile — `Record` has no `Clone`, which is the bound that also stopped the real implementation cloning |

That last row is worth keeping: the mutation I wanted to test was not
expressible, for the same reason the implementation had to record counts rather
than clone. The bound that made the code awkward also made a class of bug
unwritable.

Because `through` is now built on `nested`, the six many-to-many tests are also
a test of the nesting: three of the five mutations above were caught by tests
written before `load_nested` existed.

## What this does not do

Still record-layer only. No client can nest, for the same reason none can do a
many-to-many: the `Related` RPC takes one relationship. A nested load over the
wire is two `Related` calls and a regroup written by hand in each client.

Two levels and no more. Three would be another function, and at that point the
right move is probably the dynamic `include` form — with the depth limit this
entry says is currently unnecessary, because there it would not be.

The claim "two reads" is structural rather than measured. Nothing here counts
round trips at runtime; the `IN`-value assertion in the many-to-many tests
measures a request's size, not its latency, and the deployed benchmark does not
exercise either function.
