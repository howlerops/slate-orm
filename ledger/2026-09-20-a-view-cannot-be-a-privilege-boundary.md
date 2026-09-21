# A view here cannot be a privilege boundary, which is the half the row did not say

- **Date:** 2026-09-20
- **Author:** Claude, designing F5 before any code, as the row asked
- **Touches:** `docs/views.md`, `docs/orm-comparison.md`, `crates/slate-wasm/src/sql.rs`, `crates/slate-wasm/tests/ctes.rs`
- **Kind:** docs

## What changed

`docs/views.md`, settling four decisions and leaving two open. `CREATE` is
refused by name, carrying the one that matters. The comparison's views row says
both halves now. One test, two mutations.

Nothing is built. The row asked for the design first and this is the design.

## Why

The row already named a hazard and it is correct:

> Give a view its own `TableId` so that lookup finds it, and every grant and
> policy check keys on the *view's* id.

Read against `security.rs`, the consequence is sharper than "checks key on the
wrong id". `row_filter_with` does two lookups by `table.id()` — `rls_enabled`
and the policy filter — and for an id nothing enabled and nothing has a policy
for, both come back permissive. The result is not a failure. It is `Expr::True`
over the base table's rows, which is the worst available outcome and the one
that looks like success.

**The half the row did not state is the one a user will actually hit.** A
Postgres view runs with its owner's privileges, which is *why* people reach for
views — "give the analysts a view over the non-sensitive columns". Here that
cannot work: `Grant { role, table, actions }` has no owner for a view to run
as, so a caller needs the grant on every base table, and having it could read
the columns the view leaves out. A view is sugar, not a boundary.

Saying that when somebody writes `CREATE VIEW` is the entire value. Saying it
after they have built a permission model on the opposite assumption is the
failure this note exists to prevent, and it is not a bug anything would catch.

## Alternatives rejected

**Give a view an owner, so it can carry privileges.** The Postgres semantics,
and it means adding an authorisation principal to the security model in order
to ship a convenience feature. Wrong order: if a role needs to read three
columns of five, the honest feature is a column-level grant, which serves that
need directly and does not turn a query alias into a security object.

**Make the view a `TableDef` and special-case `row_filter_with` to follow it
back to the base table.** Fewer moving parts at the declaration end, and it
puts a security-critical redirection inside the function every path depends on
— where forgetting it in one caller is exactly the "fix that inherited the
scope of the finding it was written for" this repository has met five times.
Substitution before planning means `security.rs` learns nothing about views at
all, and **a check that needs no new case cannot get the new case wrong.**

**Allow writes through a simple view** — no join, no aggregate, a projection
that includes the key. Mappable, and it makes "updatable" a property of the
view's shape that a user has to learn, with the boundary in a place nobody will
remember. Postgres offers triggers for the rest and there are no triggers here,
by the deliberate decision in `validation.md`. Refused, naming the base table.

**Decide the CTE half at the same time.** `ctes.md` establishes they are the
same expansion, so it is tempting. Declined: the CTE has no security question
in it, and designing the case *with* the question is what makes the answer
right for both. The reverse would settle the shape in the place with less at
stake.

## Evidence

**Measured, through the workbench's parser:**

```
CREATE VIEW v AS SELECT id FROM books
  -> expected SELECT, INSERT, UPDATE or DELETE, found `CREATE`
SELECT id FROM v
  -> no table named `v` — this database has authors and books and trips and zones
```

The second is the hazard's exact location: a view name fails at
`table_by_name`, and the obvious fix is the unsafe one.

The `row_filter_with` reading is quoted from source in the note rather than
paraphrased, because the argument is about two specific lines.

One test, two mutations, both caught:

```
ok  CREATE falls through to the generic message           -> create_is_refused_by_name_and_says_a_view_is_not_a_grant
ok  the grant warning is dropped from the CREATE message  -> the same
```

The second is the one worth having: a message that merely said "CREATE is not
supported, see docs/views.md" would pass a test checking the keyword and would
lose the only part a reader needs at that moment.

**The cited-docs guard added an hour ago is already load-bearing**: `docs/views.md`
is cited from a Rust string literal, and `check_cited_docs.py` now reports 53
citations rather than 50, all resolving. That is the guard doing the job it was
written for on its first new citation.

`cargo test -p slate-wasm --test ctes`: 5 passed. `python3 site/check/docs.py`:
holds together.

## What this does not do

**It builds nothing**, and the note names the thing that blocks building: there
is nowhere to *declare* a view. The catalog comes from TOML in `slate-serverd`
and from a builder in Rust; a view is a query, and neither surface can write
one down. A `[[views]]` block holding SQL would make the daemon's schema loader
depend on the SQL front end in `slate-wasm`, a dependency direction nothing has
needed. That is a packaging question, not a security one, and it is unanswered.

**The Postgres comparison is from memory of its documented behaviour**, not
from a tested Postgres. Decision 2's conclusion does not rest on it — this
system has no owner concept either way — but the framing of the warning does.

**Nothing checks that `EXPLAIN` over an expanded view is safe.** The plan names
the base table, which tells a caller what the view is made of. `Action::Explain`
is already excluded from `Action::ALL`, so the posture is consistent; consistent
is an argument, and nobody has looked at what an expanded plan would actually
print.

**`CREATE` is refused as a whole word**, not just `CREATE VIEW`. `CREATE TABLE`
and `CREATE INDEX` get the same message, which is right about the first clause
(this front end queries a catalog rather than defining one) and says more about
views than either needs. A reader typing `CREATE INDEX` gets a paragraph about
grants.

**Materialised views are not considered at all** — a different feature with
storage, staleness and a refresh path.

**Nested views are not decided.** Substitution composes, so it would probably
work, and "probably" is carrying weight in that sentence; the depth question
`ctes.md` raises for nested arrays and nested CTEs applies here too.

---

## Addendum, same commit: a CI failure the local checks could not see

`b817c4b` went red on one job — the conformance runner — with:

```
src/values.ts(22,39): error TS2366: Function lacks ending return statement
  and return type does not include 'undefined'.
```

The demo's Node adapter has its own `encode(value: Value)` switch, and adding
`"array"` to the client's `Value` union made it non-exhaustive. Real, and
caught by CI rather than here.

**Why `scripts/check.sh` passed on the same tree is the finding.** The adapter
imports `@slate-orm/client`, which resolves to `clients/typescript/dist` — a
*build artifact*. Locally that directory predated the new variant, so the
switch still looked exhaustive; CI runs `npm run build` first and got the real
union. Demonstrated by doing it in order:

```
examples/explorer/backends/node$ npx tsc -p tsconfig.json   -> exit 0
clients/typescript$ npm run build
examples/explorer/backends/node$ npx tsc -p tsconfig.json   -> error TS2366
```

This is the same class as the `SLATE_SERVERD` staleness guard — a prebuilt
artifact older than its source turns a check into a test of the past — and
`check.sh` had it for TypeScript with nothing saying so. A `ts-client-build`
step now runs before `ts-adapter-types`, which is what CI does; rebuilding is
cheaper than a second staleness rule and removes the question rather than
reporting it. `ts-adapter-types` also now passes `-p tsconfig.json`, matching
CI exactly, because two commands that differ are two checks.

**All three adapters got the arm**, not just the one that failed. Go and Python
are not exhaustiveness-checked, so they would have gone on emitting
`{"unknown": "slate.Array"}` and `{"unknown": "Array"}` silently — which is
word for word what happened when `books` grew an embedding, and their `vector`
arms carry the comment saying so. The comment predicted this and the code did
it again; three adapters that encode identically is a property nothing checks
except the conformance runner, and only for values it actually sends.

**What this still does not do**: no conformance query reads the `posts` table,
so the three new arms are compiled and unexercised. They agree by construction
— each tags elements recursively through its own `encode` — and by reading,
not by a run. Making the runner touch an array column is the honest close and
is not done.
