# `HAVING count(*) > 5 OR count(*) < 2` now works — on a single table, a join and a chain. The only clause left taking `AND` alone is a join's `WHERE`, and that one has a reason.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `crates/slate-sql/{lib,sql,lower}.rs`, `crates/slate-wasm/src/lib.rs`, `tests/{having,taxi,sql}.rs`, `site/docs/limits.html`
- **Kind:** the second half of the disjunction work

## What changed

`QuerySpec`, `JoinSpec` and `ChainSpec` each gain `having_any_of`, the
`HAVING` twin of `any_of`. Both HAVING loops — the single-table one and the
shared join/chain one — take either connective and refuse a mixture, and the
refusal now **names its clause**, because the same message serves `WHERE` and
`HAVING` and a reader needs to know which one it came from.

The lowering gained `group_predicate(all, any, …)`, which composes both. The
three consumers in `slate-wasm` went through one `having` before and go
through one `group_predicate` now — a second spelling on one of them is how
the joined path came to differ from the single-table one in the first place.

## Why one function rather than two call-site compositions

Every consumer wants the same composition, `(all of these) AND (any of
those)`, and there are three of them. Composing at the call site means
writing it three times and the fourth consumer writing it a fourth way.

Inside it, each ORed term goes through `having` **one at a time** rather than
through a copied loop. The per-term work — the operator table, the literal
parsing, the group-space type lookup — has exactly one implementation. A
second copy is how the two paths come to disagree about what `matches` does.

## Alternatives rejected

**A `connective` flag on `having`.** One field, no second list. Rejected
because `having` is public and three callers pass it positionally; a bare
`bool` at a call site reads as nothing, and the spec would no longer say
which connective it holds without consulting a sibling field.

**Refuse `OR` in a join's `HAVING` and allow it single-table.** The
asymmetry the previous entry left. There is no reason for it: a `HAVING`
resolves against the group space and a group is a group whatever produced it.
The join path needed the same two lines.

**Allow it in a join's `WHERE` too.** Refused, with a reason this time rather
than a deferral. That loop splits its conditions by side so each scan is
narrowed before the hash join runs. A disjunction spanning both sides cannot
be split that way, and one that happens not to span them would be a special
case that works until somebody writes the other kind.

## Evidence

`cargo test -p slate-sql -p slate-wasm`: **240 passed, exit 0.**
`cargo fmt --all -- --check` clean. `site/check/docs.py` passes.

Mutations, via `scripts/mutate.py`:

| mutation | result |
| --- | --- |
| an ORed `HAVING` lowers as a conjunction | caught — `an_ored_having_admits_a_group_either_arm_admits` |
| the ORed `HAVING` terms are dropped | caught — same |
| a mixed `HAVING` is accepted, `OR` after `AND` | caught — `it_refuses_what_it_cannot_answer` |
| a grouped join's ORed `HAVING` is stored as ANDed | **survived**, then caught |

**That survivor is the finding.** The join path routes `having` into one of
two spec fields, and mutating that routing to always-AND passed every test —
because the join's only new test asserted the *refusal* of a mixture, which
the mutation does not touch. Running is not the claim. The join now has the
same union assertion the single-table path has: `both == high + low` over the
taxi fixture, with both arms required to admit something so the equality
cannot hold vacuously.

**And a near-miss worth recording.** Between those runs I read a suite as
green that had not compiled. `cargo test` failed on a missing struct field in
a proptest generator, and my check was `grep -c "FAILED"` — which found none,
because nothing ran. Same shape as the `| tail -5` mistake earlier today, and
the same lesson twice in one session: **count what passed and check the exit
code**, never grep for the word "failed". The runs above report both.

## What this does not do

**A join's `WHERE` still takes `AND` only**, for the reason above. That is
now a decision with a stated cost rather than an unexplained asymmetry.

**No parentheses, so still no nesting.** `HAVING a AND (b OR c)` is
inexpressible, exactly as in `WHERE`.

**The three SDKs cannot send either form.** `any_of` and `having_any_of` are
`slate-sql` spec fields with no gRPC equivalent, so both disjunctions are
reachable from the browser workbench and a view's SQL and nowhere else. One
boundary, now two features deep.

**No workbench example for an ORed `HAVING`.** The `WHERE` form has two; this
has tests and prose. A third example would say little the second does not.

**Nothing measures it.** A disjunction over groups costs what evaluating it
per group costs, and no number here says what that is.
