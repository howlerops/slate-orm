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

## 3a. Refusing everything else is free, and that decides the build order

Decision 1 says a view must not be a `TableDef`. Tracing what that *already*
buys, before any code:

```rust
pub fn table_by_name(&self, name: &str) -> Option<&TableDef> {
    self.tables.iter().find(|t| t.name() == name)
}
```

Every path that turns a request's table name into a `TableDef` goes through
that function — `service.rs`'s `table`, and the two converters in
`convert.rs` — and `scripts/check_handlers.py` rule 1 already holds every
handler to one of them. So **a view held outside the `Catalog` is refused by
every path that exists, without a line being written.** Not by a check
somebody added: by there being nothing to find.

That is decision 4 (writes refused) delivered by construction rather than by
vigilance, and it also refuses joins, chains, aggregates, paging and every
other surface — which is the correct default for all of them today.

**So the build order is: declare first, read last.**

1. `[[views]]` in the TOML, parsed and validated at load, stored in a registry
   beside the `Catalog` and not in it. At this point a view can be declared
   and every use of one is refused. That state is complete and safe, not a
   half-feature.
2. Opt *one* read path in, deliberately, by resolving the name to the view's
   **base** `TableDef` and `AND`ing the view's predicate onto the caller's.
   Authorisation and RLS then see the base table, which is decision 1's whole
   point.

Doing it the other way round — teaching the reader about views and then
remembering to refuse the writers — is the shape every finding in
`security-review.md` has: a fix that covered one path of several. This order
has no such intermediate state.

**The one thing it costs** is the refusal's wording. A declared-but-unusable
view answers "no table named `recent_books`", which is wrong — it exists, and
it is not usable *there*. That is a message to improve, not a hole, and
improving it means naming views in a place that can afford to know about them.

### Where step 1 landed

`crates/slate-serverd/src/views.rs`, resolved in `run` beside
`schema::catalog` and published by `--print-schema` under a `views` key. The
subset a view may be is narrower than this section implies and deliberately
so: a `WHERE` and nothing else. A projection, a sort, a limit, an offset, a
grouping, a computed column and a window are each refused at load with a
reason — the module docs argue each one, and the short version is that a
projection would make the caller's ordinals *view* ordinals and everything
else changes what a row is, so there would be no base row for a policy to
admit.

The refusal list is not what decides. `beyond_a_where` serialises the spec and
refuses any key outside `table`, `filter`, `filters`, so a field added to
`QuerySpec` later is refused rather than silently accepted; the named list only
picks the better sentence for the cases somebody has thought about.

Step 2 is open, and the wording cost above is still unpaid.

## 4. Writes through a view are refused

An updatable view needs a rule for mapping a written row back onto base rows,
and for a view with a projection, a join or an aggregate there is no such rule
that is not a guess. Postgres refuses most of them too and offers triggers for
the rest; there are no triggers here — `validation.md` designs hooks out
deliberately — so there is nothing to offer.

Refused, and the refusal should name the base table, because the useful next
step is to write to that.

## Open, and deliberately not decided here

**Where a view is declared — answered, and the answer dissolves the
objection.** The paragraph this replaces said a `[[views]]` block holding SQL
would make the daemon's schema loader depend on the SQL front end "which lives
in `slate-wasm` — a dependency direction nothing has needed yet", and that the
alternative of a `QuerySpec` in TOML is unreadable. The second half still
holds. The first half was true as stated and wrong in what it implied, because
it took the crate's *name* for a fact about its contents.

Measured:

| | lines |
| --- | --- |
| `crates/slate-wasm/src/` | 7,384 |
| lines mentioning `wasm_bindgen` | **5** |

Those five are one `use`, one `inline_js` shim for `performance.now()`, and
two attributes on `Playground` and its constructor. `sql.rs` — 3,402 lines,
the whole parser — mentions none: it imports `QuerySpec` and its neighbours
from the crate root, `TableDef` from `slate-schema` and `ValueType` from
`slate-tuple`, and nothing else. The only genuinely wasm-specific thing in the
manifest is a target-scoped `uuid` feature, already commented as such.

So the SQL front end is not browser code that a daemon would be reaching
*into*. It is portable Rust that happens to be housed in the binding crate
because the binding is what needed it first. Extracting a `slate-sql` crate —
the parser, the spec types, the lowering — leaves `slate-wasm` as the thin
`#[wasm_bindgen]` shell it nearly already is, and lets `slate-serverd` depend
on the parser exactly as the browser does. There is no new direction: both
become peers over a shared crate.

That is a refactor of some size and it is not free. But it is ordinary work
with a known shape, rather than the architectural objection this section
recorded, and calling it blocking was wrong. `[[views]]` holding SQL is the
readable surface, and the way to get there is to move the parser to where both
callers can see it.

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
