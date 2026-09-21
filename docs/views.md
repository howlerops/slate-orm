# A view here cannot be a privilege boundary, and that is the whole design

`orm-comparison.md` lists views as missing and names a hazard rather than a
plan:

> Grants and policies are keyed on `TableId` (`Grant { table: TableId }`,
> `Policy { table: TableId }`), and a query resolves its table by name through
> `Catalog::table_by_name`. Give a view its own `TableId` so that lookup finds
> it, and every grant and policy check keys on the *view's* id — a caller
> granted the view reads the base table's rows with the base table's policy
> never consulted. The safe shape is that a view expands to its underlying spec
> *before* planning, so the base table's id is what reaches the policy, and
> that has to be structural rather than a convention somebody remembers.

That is correct and it is only half of the design. Expansion-before-planning
settles *where* the policy comes from. It does not settle the question a user
arrives with, which is **whether being granted a view is enough**. In Postgres
it is — a view runs with its owner's privileges, which is precisely why views
are used as a privilege boundary. Here it cannot be, and that has to be said
out loud rather than discovered.

This note settles four things and leaves two open.

## What it does today, measured

```
CREATE VIEW v AS SELECT id FROM books
  -> expected SELECT, INSERT, UPDATE or DELETE, found `CREATE`

SELECT id FROM v
  -> no table named `v` — this database has authors and books and trips and zones
```

The second is the interesting one: a view name fails at `table_by_name`, which
is exactly where the hazard lives. The obvious fix — put the view in the
catalog so the lookup succeeds — is the wrong one, for the reason above.

## 1. A view is a named `QuerySpec`, not a `TableDef`

The hazard is not a bug to avoid; it is a consequence of the representation. A
view that is a `TableDef` has a `TableId`, and a `TableId` is what
`SecurityCatalog::row_filter_with` looks up:

```rust
if self.rls_enabled.get(&table.id()).copied().unwrap_or(false) {
    let applicable: Vec<&Policy> = self
        .policies
        .iter()
        .filter(|p| p.applies_to(context, table.id(), action))
```

Give the view an id and both lines key on it: RLS reads as *off* for the view
(nothing enabled it), and no policy applies. The result is not an error. It is
`Expr::True`, over the base table's rows.

**So a view must not be a `TableDef`.** It is a name bound to a `QuerySpec`
over base tables, resolved by substitution before planning. After substitution
there is no view left — the spec names `books`, `row_filter_with` receives
`books`'s `TableDef`, and every existing check applies unchanged. Nothing in
`security.rs` has to learn what a view is, which is the property worth having:
a security check that needs no new case cannot get the new case wrong.

The tenant filter and the soft-delete filter come along for free, from the same
`TableDef` and the same function.

## 2. A view is sugar, not a grant — and users will expect otherwise

**A caller must hold the grant on every base table the view reads.** Not on the
view.

This is the opposite of Postgres and it is the only answer available. A view
that carried its own privileges would need an *owner* whose rights it runs
with, and there is no owner concept here: `Grant { role, table, actions }` has
a role and a table and nothing that could play that part. Inventing one to
support views would be adding an authorisation principal in order to add a
convenience feature, which is the wrong order.

The cost is real and is the reason this section exists. "Give the analysts a
view over the non-sensitive columns" is *the* reason people reach for views,
and it does not work here — the analysts would still need a grant on the base
table, and having it they could read the columns the view omits. Anyone
expecting the Postgres behaviour gets a security posture they did not intend.

**So the refusal, the documentation and the eventual error message all have to
say this**, and saying it late — after somebody has built a permission model on
it — is the failure mode. Column-level grants are the feature that would
actually serve that use, and they are a different item.

## 3. The composition is an `AND`, and the order does not matter

A view carries a predicate; the caller adds one; the security layer adds a
tenant filter, a policy filter and a not-deleted filter. All five are
conjunctions, and `and` is associative, so the *result* does not depend on the
order they are combined in.

What does depend on order is **which `TableDef` the security filter is built
from**, and decision 1 settles it: after substitution, the base table's. That
is the sentence the comparison row was reaching for, and it is worth separating
from the composition question it looks like.

There is one asymmetry. The view's predicate is part of the *query*; the policy
filter is part of the *system*. If a view's predicate and a policy contradict,
the answer is no rows, and that is correct — a view is not permitted to widen
what a policy admits, and an `AND` cannot.

## 4. Writes through a view are refused

An updatable view needs a rule for mapping a written row back onto base rows,
and for a view with a projection, a join or an aggregate there is no such rule
that is not a guess. Postgres refuses most of them too and offers triggers for
the rest; there are no triggers here — `validation.md` designs hooks out
deliberately — so there is nothing to offer.

Refused, and the refusal should name the base table, because the useful next
step is to write to that.

## Open, and deliberately not decided here

**Where a view is declared.** The catalog is built from TOML in
`slate-serverd` and from a builder in Rust; a view is a query, and neither
surface has a way to write one down. A `[[views]]` block holding SQL would make
the daemon's schema loader depend on the SQL front end, which lives in
`slate-wasm` — a dependency direction nothing has needed yet. The alternative,
a `QuerySpec` written out in TOML, is unreadable. This is the question that
blocks building, and it is a packaging question rather than a security one.

**What `EXPLAIN` shows.** The expanded plan mentions the base table, which
tells a caller what the view is made of. `EXPLAIN` is already privileged
(`Action::Explain` is excluded from `Action::ALL` on purpose), so this is
consistent rather than a new leak — but "consistent" is an argument, and nobody
has checked whether a plan over an expanded view says anything a caller with
`Explain` should not see.

## What this note does not do

**It builds nothing**, and it deliberately does not decide the CTE half either.
`ctes.md` establishes that a single-reference, non-recursive CTE is the same
expansion, so whichever is built first sets the shape for both — which is the
argument for designing before building, and for designing the *view* case,
because it is the one with the security question in it.

**The measurements are two parse results.** Everything else here is read off
`security.rs`, `catalog.rs` and `orm-comparison.md`. No view was built and
nothing was demonstrated failing, because there is nothing to fail yet.

**It does not survey what other ORMs do about the privilege question.** The
Postgres comparison is from memory of its documented behaviour, not from a
tested Postgres. If that is wrong, decision 2's *conclusion* is unaffected —
this system has no owner concept either way — but the warning's framing is.

**It says nothing about materialised views.** A different feature with a
different cost model: storage, staleness and a refresh path. Not considered.

**It does not decide whether a view may reference another view.** Substitution
composes, so it would probably work; "probably" is doing load-bearing work in
that sentence, and the depth limit question that `ctes.md` raises for nesting
applies here too.
