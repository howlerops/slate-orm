# `Expr::Or` has existed since expressions arrived and nothing could produce one. Four entries recorded that. `WHERE a = 1 OR b = 2` now works.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `crates/slate-sql/{lib,sql,lower}.rs`, its tests, `crates/slate-wasm/tests/{sql,examples,having,taxi}.rs`, `site/workbench.js`, `site/docs/limits.html`
- **Kind:** closing the most-cited capability gap in the backlog

## What changed

`QuerySpec` gains `any_of: Vec<FilterSpec>`. The parser fills it when a
`WHERE` is written with `OR`, and the lowering turns it into `Expr::Or`.

A clause is **all `AND` or all `OR`**. A mixture is refused by name, in both
orders. Single-table reads and a subquery's inner read take `OR`; a join's
`WHERE` and every `HAVING` still take `AND` only, and their refusals now give
the real reason rather than pointing at a `WHERE` that no longer refuses it.

## Why two lists rather than a tree

The obvious design is a recursive `FilterTree`. Rejected: it expresses
`a AND (b OR c)`, which nobody has asked to write, and it costs a recursive
shape in the Spec tab, the panel, the round-trip renderer and every consumer
of a `QuerySpec`. Two flat lists express a disjunction of comparisons — a
real subset, and the one with **no precedence question in it**.

That mattered more than the expressiveness. There are no parentheses in this
grammar. `a AND b OR c` is `(a AND b) OR c` in SQL and reads as
`a AND (b OR c)` to a good half of everyone, and a query language embedded in
a demo is the wrong place for a visitor to discover which reading they hold.
Refusing the mixture removes the question rather than answering it quietly.

Lowering ANDs the two lists rather than treating them as exclusive, so a spec
carrying both means `(all) AND (any)` — what the nesting would mean. The
parser never produces both today; writing the lowering for the widened shape
means widening it later cannot silently change what an existing spec means.

## Alternatives rejected

**Leave it refused.** The standing position, and the reason four entries
recorded it as a gap rather than a decision: the kernel has the operator, the
planner handles it, and the only thing missing was a spelling. A refusal is
honest when the thing underneath does not exist. This one did.

**Support it on joins too.** The join path has its own `WHERE` loop that
splits conditions by side so each scan is narrowed before the hash join. A
disjunction spanning both sides cannot be split that way, and deciding what
happens to one that does not span them is a design question. Refused, with
the reason.

**Support it in `HAVING`.** A `HAVING` term resolves against the group space
through one flat list, and the resolver would need the same widening. It is
the obvious next step and it is not here.

## Evidence

`cargo test -p slate-wasm -p slate-sql`: **239 tests, 0 failures.**
`cargo fmt --all -- --check` clean. `site/check/docs.py` passes.

Mutations, via `scripts/mutate.py` — five, all caught:

| mutation | caught by |
| --- | --- |
| a disjunction lowers as a conjunction | `a_disjunction_admits_a_row_either_arm_admits` |
| the ORed conditions are dropped entirely | same, and the shape test |
| `AND` after `OR` is accepted | `the_other_order_of_mixing_is_refused_too` |
| `OR` after `AND` is accepted | `and_and_or_cannot_be_mixed_in_one_where` |
| an ORed `WHERE` is stored as ANDed conditions | the two lowering tests |

**The round-trip property found a real asymmetry on its first run with `OR`
in it.** A one-element `any_of` renders as `WHERE a = 1` — no connective in
the text — so it parses back into `filters` and the spec does not survive the
trip. Neither side is wrong: a one-element disjunction *is* a one-element
conjunction and `filters` is the canonical spelling. The generator now only
produces `any_of` with two or more, and the reasoning is in a comment where
the next person will hit it.

**And it caught a wrong verdict of mine.** Checking what the site says about
the grammar, the limits page reads "stopping at `UNION`, `EXISTS` and anything
correlated" — which is true. During the triage I had marked the caveat "No
`EXISTS`, no set operators, no correlation" as **closed**, citing a completed
task whose *title* is "Subqueries, EXISTS and UNION in the SQL front end".
That task refused them; it did not build them. The verdict is now `deliberate`
with the refusal messages as its citation. This is exactly the failure mode
the triage entry named — "a wrong `closed` is invisible" — found by doing
adjacent work rather than by any check.

## Where a reader meets it

- The workbench has a new example, **"Either of two zones"**, and the kitchen
  sink has a third statement, because `OR` cannot be folded into a clause that
  already uses `AND` — which is itself the point the comment makes.
- `the_kitchen_sink_uses_everything_it_claims_to` asserts the third statement
  fills `anyOf` with three conditions and leaves `filters` empty. Running is
  not the claim: a parser that quietly ANDed it would return fewer rows and
  still run.
- The limits page gains a paragraph, including the part a user will feel: a
  disjunction is a **filter, not an access path**, because `a = 1 OR b = 2`
  has no single key range.

**The runs themselves**, as `mutate.py` recorded them:

- `ledger/mutations/20260925T204924-crates-slate-sql-src-lower-rs.json` — crates/slate-sql/src/lower.rs
- `ledger/mutations/20260925T204938-crates-slate-sql-src-sql-rs.json` — crates/slate-sql/src/sql.rs

## What this does not do

**No `OR` on a join or in `HAVING`.** The two largest remaining pieces, both
refused by name with their real reasons. Three open caveats still say so and
they are still accurate.

**No parentheses, so no nesting.** `a AND (b OR c)` is inexpressible, and the
spec is shaped so that adding it later is a widening rather than a
reinterpretation — but it is not done, and "later" has no plan.

**The planner does nothing with a disjunction.** It arrives as one opaque
conjunct and is evaluated per row. `a = 1 OR a = 2` on an indexed column
*could* become two point gets — `IN` already does exactly that — and nothing
here notices the equivalence.

**Nothing measures it.** No benchmark; the claim is about what is
expressible, not about what it costs.

~~**No client can send one.** `any_of` is a `slate-sql` spec field, and the
gRPC `Query` has no equivalent — the three SDKs build predicates from typed
builders that have no disjunction either. So this is reachable from the
browser workbench and from a view's SQL, and from nowhere else. That is the
same boundary arrays and windows each stopped at first, and it is the obvious
next increment.~~

**Withdrawn, 2026-09-25, and wrong on both halves.** `Expr.disjunction` is
field 9 of the wire's `Expr` and has been since the protocol carried
expressions; `convert.rs` maps it to `Expr::Or` and back. All three clients
have a builder — Python's `any_of` and `|`, Go's `Disjunction`, TypeScript's
`or`. A client could always send a disjunction.

What was missing was only a way to *write* one as SQL text, which is what
this commit added. I generalised from the front end to the whole system
without checking, and nothing executed the claim either way until
`clients/python/tests/test_disjunction.py`, which now does. The correct
caveat is the narrower one below.

## Correction, 2026-09-25

**A client's disjunction is now executed, not assumed.**
`clients/python/tests/test_disjunction.py` sends one through a real head node
in a `WHERE` and in a `HAVING`, and asserts the answer is the union of the
arms — with the arms required to differ, so a server that ANDed them would
fail rather than coincide.

What remains true and narrower: **Go and TypeScript have the builder and no
live test of it.** Python's is the only one executed against a server.
