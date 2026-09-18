# `NOT IN` was refused because the kernel supposedly had no negation of `Expr::In`; it has `Expr::Not`, and the three-valued objection is standard SQL's behaviour rather than a defect

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `slate-wasm` (`sql.rs`, `lib.rs`, `tests/sql.rs`, `tests/subqueries.rs`, `tests/taxi.rs`), `slate-kernel/tests/point_gets.rs`, `README.md`, `docs/orm-comparison.md`, `site/docs/features.html`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

`WHERE x NOT IN (…)` and `WHERE x NOT IN (SELECT …)` parse and run. They lower
to `Expr::Not(Expr::In { … })` — nothing new in the kernel, nothing new on the
wire, one new spec operator (`notIn`) so a reader of the spec tab can see which
it was.

`NOT EXISTS` stays refused, for the reason `EXISTS` is refused — correlation —
and its message now points at `key NOT IN (SELECT …)`, which it could not do
while that was also refused.

Four documents carried the claim this withdraws.

## Why

Two claims stood behind the refusal, and neither survives.

**"The kernel has `Expr::In` and no negation of it, and adding one is not a
parser change."** (`README.md`.) It has `Expr::Not(Box<Expr>)`, which composes
over any expression including `In`. Nothing needed adding.

**"`IN` is three-valued here — a null in the list makes the answer unknown
rather than false — so negating it does not mean what it looks like."**
(`sql.rs`.) The premise is true and the conclusion does not follow. Standard
SQL's `NOT IN` is three-valued in exactly the same way: `x NOT IN (1, 2, NULL)`
is UNKNOWN for an `x` that is neither 1 nor 2, which is the famous gotcha every
SQL user meets. `Truth::negate` maps unknown to unknown, so `Expr::Not` over
`Expr::In` produces the answer the standard specifies. The surprise belongs to
SQL, not to this implementation — and refusing a standard construct because the
standard is counter-intuitive is a different decision from the one that comment
was making, made without saying so.

There is a narrower fact that makes even the gotcha unreachable here: the front
end has no `NULL` literal, and `resolve_subqueries` drops null candidates when
it renders a subquery's rows. So a null can reach an `IN` list by neither
route, and the three-valued case is currently unconstructible from SQL.

## Alternatives rejected

**Keep the refusal and correct only its stated reason.** The cheap fix, and it
was seriously considered: a refusal with an honest reason ("SQL's `NOT IN` is
surprising and we would rather you wrote it another way") is defensible. It was
rejected because the surprise is unreachable from this front end, so the
refusal would be protecting against a case no query can express — and because
`NOT EXISTS`'s refusal was pointing at `NOT IN` as its rewrite, which made the
pair mutually unhelpful.

**Lower `NOT IN` to `AND`ed `!=` comparisons.** Avoids `Expr::Not` entirely.
Rejected because it is a different predicate: `x != NULL` is unknown for every
`x`, so a null candidate would make the whole conjunction unknown where the
standard makes only the non-matching rows unknown. It also loses the shape that
`EXPLAIN` shows, and multiplies the residual by the list length.

**Teach the planner to use the index for a `NOT IN`** — scan the ranges either
side of each excluded point. Rejected as premature: it is a real optimisation
for a long exclusion list over a clustered key, and it needs a cost model for
"n+1 ranges" that does not exist. The current answer is a scan with a residual,
which is correct and is what the complement of a point set is.

## Evidence

**Three mutations, all killed:**

| mutation | outcome |
| --- | --- |
| `notIn` lowers to a plain `In` (the negation dropped) | killed — `not_in_is_the_complement_of_in`, `not_in_arrives_as_a_negation_and_not_as_a_range` |
| the parser always emits `in`, never `notIn` | killed — three tests |
| `Expr::conjuncts` looks *through* a `Not` | killed — `a_negated_in_is_not_a_point_get_set` |

The third is the one that matters. `conjuncts` treating `Not` as an opaque leaf
is what stops `collect_constraints` from seeing the `In` inside one, and a
build that looked through would plan point gets for exactly the rows the query
*excludes* — returning the complement of what was asked, with no error. That
mechanism was checked before the parser started emitting `NOT IN`, and it now
has a test.

**The partition**, in `not_in_is_the_complement_of_in`: `IN (1, 2)` and
`NOT IN (1, 2)` over a column with no nulls return disjoint sets whose sizes
add to the whole table. A lowering that got the polarity or the null handling
wrong fails that without anyone having to predict how.

**A hypothesis withdrawn, twice over.** The plan test was written expecting the
positive form to use an access path, as a control. Against `author_id` (a
secondary index) it planned a `Table Scan`; against `id` (the primary key) it
also planned a `Table Scan` — on this 4,824-row `MemoryStore` a scan costs
1.603 and two point gets cost 3.00, so the planner is right and the control is
useless. A test whose two arms agree proves nothing about the difference
between them. The plan half of the claim moved to `slate-kernel`'s
`point_gets.rs`, where a fixture exists on which `IN` genuinely becomes point
gets, and the wasm test keeps only what that layer owns — that the negation
arrives as a `Not`. Both halves say so in their comments.

`cargo test -p slate-wasm --no-fail-fast`, `cargo test -p slate-kernel --test
point_gets`, `cargo clippy -p slate-wasm -p slate-kernel --all-targets`,
`python3 site/check/docs.py`: green.

## What this does not do

- **`NOT EXISTS` is still refused**, and correctly: it is correlated, and a
  subquery here runs once. Only its message changed.
- **No client can express it.** `notIn` is a front-end operator; the wire
  carries `Expr`, and the three SDKs build `Expr::In` with no negation helper.
  A client that wants this builds the `Not` itself, which is possible and
  undocumented.
- **The planner does not optimise it.** A `NOT IN` is always a residual over
  whatever access path the rest of the predicate chooses — see the rejected
  alternative above.
- **The unreachable-null argument is a fact about today.** If the front end
  ever gains a `NULL` literal, `NOT IN` will start being able to produce the
  standard's unknown, which is correct but will surprise somebody. That is a
  reason to document it then, not to refuse it now.
- **`docs/correctness.md`'s `NOT IN` cases were not revisited.** They are about
  the security compiler's `Pred::lower`, which builds `Expr` directly and never
  went through this refusal — they were already exercising the path this commit
  makes reachable from SQL, which is a pleasant way to find out the lowering
  was sound.
