# What the other ORMs have that this does not

A feature audit of `slate-orm` against SQLAlchemy 2.0, Drizzle, Prisma, Ecto,
ActiveRecord, Diesel and SeaORM, and a plan for the gaps worth closing.

Every "we do not have this" below was checked against the source, not
remembered. Where a claim is about the wire it was checked against
`crates/slate-server/proto/slate/v1/records.proto`; where it is about the
record layer, against `crates/slate-orm/src/`; where it is about the kernel,
against `crates/slate-kernel/src/`. The grep that established each one is named
so the next reader can re-run it rather than trust this file — which will go
stale, and this paragraph is the instruction for what to do when it has.

## How to read this

Three categories, and the difference between them matters more than the list:

- **Missing** — a thing developers expect, that nothing here forbids. Work.
- **Refused** — a thing we could build and have decided not to, with the
  reasoning written down. Not work; a decision to re-examine only if the
  reasoning stops holding.
- **Forbidden** — a thing the architecture rules out. One writer, no SQL on the
  wire, a kernel with no planner hooks for it. Work only if the architecture
  changes, which is a different conversation.

A feature list that does not draw those lines reads as a backlog of failures.
Most of what follows is the first category, and the first two items in it are
worth more than the rest put together.

## What we already have that is competitive

Stated first, because a gap list with no baseline is not an audit. Against the
seven ORMs above, `slate-orm` is at or ahead of the field on:

- **A cost-based planner with `EXPLAIN`.** None of the seven has one — they all
  delegate planning to the database. Ours chooses between table scan, index
  scan, point gets, `IN` ranges, `LIKE` prefix bounds and covering scans, with
  histograms for range selectivity, and says which it picked.
- **Row-level security compiled into the predicate and the scan bounds**,
  rather than checked by a wrapper. Every read and write takes a security
  context; there is no unauthenticated door. ActiveRecord's `default_scope` and
  SQLAlchemy's filtered relationships are conventions a caller can step around.
- **Index entries written in the same transaction as the row**, with no
  background builder and no repair path.
- **Batched relationship loading with no lazy fallback.** `load_related`
  dedupes the key set and issues one read; there is no N+1 to fall into,
  because there is no lazy load to accidentally trigger. That is a stronger
  guarantee than Ecto's `preload` or SQLAlchemy's `selectinload`, both of which
  sit beside a lazy loader that fires when you forget.
- **A conformance runner that requires three client SDKs to return
  byte-identical answers.** No ORM in the list does this because none of them
  has three clients.
- **Schema fingerprinting on every request**, so a client whose declaration has
  drifted from the catalog is refused rather than answered from the wrong
  column.

## Missing: the two that matter most

### 1. Predicate writes — `UPDATE … WHERE` and `DELETE … WHERE`

**Evidence.** `grep -n "message DeleteRequest" -A 14` on the proto: delete
takes `repeated Row primary_keys` and nothing else. The proto's own comment on
`UpdateRequest` says it plainly: *"An update is a row replacement: it names
whole rows by their primary keys and writes them entire. There is no
`SET column = expression`, and no predicate form."* `grep -rn "fn delete_many\|delete_where"`
across `crates/` returns nothing.

**What that costs.** `DELETE FROM sessions WHERE expires_at < now()` is not
expressible. The caller has to query the keys, carry them back, and issue a
delete per key — which is N+1 by construction, is not atomic with the query
that found them, and races anything writing in between. Every ORM in the list
has this; it is close to the first thing anyone reaches for.

Worse is the *update* half. Without `SET views = views + 1` the only way to
increment a counter is read-modify-write, and two callers doing that lose one
increment. `update_if_unchanged` makes the loss *detectable* — it is real
optimistic concurrency and it works — but detecting a lost update and retrying
is a worse answer than not losing it, and it is the answer we currently force.

**Why it is not just a client feature.** The kernel has `update_many` and
`delete`, both keyed. A predicate form needs the executor to scan, apply the
policy predicate, and write — inside one transaction, with index maintenance
per row. That is kernel work, not a convenience wrapper, and doing it in a
wrapper is exactly the non-atomic version we already have.

### 2. Relations do not cross the wire

**Evidence.** `#[derive(Record)]` accepts `has_many` and `belongs_to` and emits
a `Related` impl (`crates/slate-derive/src/lib.rs:629`). The proto mentions
relations once, in an unrelated comment (`grep -cin "relat\|preload\|include"`
→ 1). The Python client's public methods are `insert, update, delete, get,
query, join, aggregate, explain, transaction, transact` — no relation, no
include, no preload.

**What that costs.** The batched relationship loading described above — one of
the genuinely better things here — is reachable **only from Rust**. A Python,
Go or TypeScript caller who wants an author's books writes the two-step fetch
by hand, and the one who forgets writes the N+1. We built the good answer and
then did not ship it to the three audiences most likely to need it.

## Missing: the rest, roughly in order of what a user notices

| Gap | Who has it | Evidence it is absent here |
| --- | --- | --- |
| Chains (3+ table joins) on the wire | all | proto has `Join`, no chain RPC; `chain` appears only in comments |
| Keyset pagination on the wire | Drizzle, Prisma (`cursor`) | `Page`/`next` exist in `slate-orm/src/ext.rs:31`; no cursor field in the proto |
| Batch: several independent statements, one round trip | Drizzle `batch`, Prisma `$transaction([…])` | `grep -rn "fn batch\|Batch" crates/slate-server/src/` → nothing |
| Many-to-many / `has_many through` | all | `Related` is one `local`→`foreign` ordinal pair; derive accepts only `has_many`/`belongs_to` |
| Nested / recursive eager loading | Ecto, SQLAlchemy, Prisma | `load_related` is one level; no nesting |
| `RETURNING` on a write | Drizzle, Prisma, Ecto | `WriteResponse` carries an affected count and no rows |
| Generated migrations from a schema diff | Drizzle Kit, Prisma Migrate, Alembic autogenerate | `slate-kernel/src/migrate.rs` plans and applies a diff but nothing *writes* the target catalog for you |
| Client codegen from the catalog | Drizzle, Prisma | every client hand-declares its schema; `SchemaCheck` catches drift at run time instead of compile time |
| Validations / changesets / lifecycle hooks | Ecto, ActiveRecord, SQLAlchemy events | `grep -rcn "validate\|before_save\|Changeset" crates/slate-orm/src/` → nothing |
| Automatic `created_at` / `updated_at` | ActiveRecord, Ecto, Prisma | nothing in the derive macro or the kernel |
| Soft delete as a first-class concept | ActiveRecord (gems), Prisma (pattern) | partial indexes support `WHERE deleted_at IS NULL` well; no convention on top |
| Window functions | SQLAlchemy, Drizzle, Diesel | aggregates are `Count, CountColumn, Min, Max, Sum, Avg, CountDistinct` |
| CTEs / recursive queries | SQLAlchemy, Drizzle, Diesel | no plan node |
| Set operations (`UNION`/`INTERSECT`/`EXCEPT`) | all | refused by name in the SQL front end; no spec node |
| Views | Drizzle, SQLAlchemy | none |
| Array / list column type | Drizzle, SQLAlchemy, Ecto | `ValueType` has no `Array` |
| Full-text search | Drizzle, SQLAlchemy | none; `LIKE`/`ILIKE`/regex only |
| Seeding / fixtures / factories | Drizzle, Prisma, ActiveRecord | none |
| Per-request logging and metrics | all | already recorded in the README: `slate-serverd` logs startup and warnings, nothing per request |
| Retrying `transact` in Go and TypeScript | — | already recorded in the README; Python has one |

## Refused, with the reasoning

Not gaps. Each of these is a decision with a written argument, and the argument
is the thing to attack if you disagree.

- **Lazy loading.** The single largest source of accidental N+1 in every ORM on
  the list. Not having it is why `load_related` has no fallback to fall into.
- **Unit of work / identity map / dirty tracking** (SQLAlchemy's `Session`).
  Implicit flush ordering is the hardest thing in SQLAlchemy to reason about,
  and the whole point of this layer is that a write is a write. The explicit
  `transact` closure covers the cases people actually use it for.
- **`NOT IN` and `NOT EXISTS`.** `Expr::In` is three-valued: a null in the list
  makes the answer unknown rather than false, so the negation a reader expects
  and the one the kernel would give differ exactly where nulls are involved.
  Written up in `crates/slate-wasm/src/sql.rs`.
- **SQL on the wire.** The wire carries a spec. That is what makes the planner,
  `EXPLAIN` and the security compilation possible at all.

## Forbidden by the architecture

- **Multi-writer and cross-shard transactions.** One writer holds the lease.
- **`UNION` across two independently planned statements with deduplication.**
  There is nowhere for the dedup to happen.

## The plan

Six pieces, ordered by value over cost. Each names what it is, how it will be
tested, and where it stops. The first two are the ones worth doing whatever
else happens.

### P1 — Predicate writes in the kernel

`delete_where(context, table, predicate)` and `update_where(context, table,
predicate, assignments)`, where an assignment is a column and a `Scalar` over
the row's own values — so `views = views + 1` is one write, not a round trip.

*Build.* A new executor path that reuses the existing scan: plan the predicate
as a query, walk the cursor, and for each row do the same index maintenance the
keyed path does, inside the caller's transaction. The policy predicate is ANDed
in by the same code that does it for reads, so a row the caller cannot see is
a row the caller cannot delete — and that needs its own test, because it is
the security-relevant half.

*Test.* An oracle: `delete_where(p)` must leave exactly the rows a full scan
filtered by `!p` would leave, over generated schemas, compared against a
brute-force fold. A second oracle for `update_where`. Then the RLS matrix,
extended: for every access path, a predicate write must not touch a row the
policy hides. Then a concurrency test: two callers incrementing the same
counter with `SET n = n + 1` must both be reflected, which is the property
read-modify-write cannot offer. Mutation-test each one.

*Stops at.* No `UPDATE … FROM`, no predicate write across a join. One table.

### P2 — Relations on the wire and in the three clients

A `Related` RPC, or a `relations` block on `QueryRequest` — the design choice
is whether the server does the second read or the client does. Server-side is
one round trip and puts the dedup where the data is; client-side keeps the
server stateless about relationships. **Recommendation: server-side**, because
the round trip is the thing being saved and the deduplication is already
written (`related_filter` sorts and dedupes, and returns the `Expr` so the
saving is observable).

*Build.* Carry the relationship declaration in the catalog rather than in each
client, so the three clients name a relationship instead of describing it —
which is also what stops them from disagreeing about it.

*Test.* Extend the three-SDK conformance runner: the same eager load through
Python, Go and TypeScript must return byte-identical answers. That runner found
three divergences on its first run and is the right place for this. Plus a
test that asserts the read count — the whole claim is "one read, not N", and a
claim nothing measures is a claim nothing keeps.

*Stops at.* One level, one relationship per request, to start. Nesting is P5.

### P3 — Chains, keyset pagination and `RETURNING` on the wire

Three small things that are all "the kernel has it, the wire does not". Each is
a proto message and a handler, and each already has kernel tests.

*Test.* Conformance runner for all three. For keyset pagination specifically,
the property that matters is that paging through with a cursor visits every row
exactly once under concurrent inserts — which `LIMIT`/`OFFSET` cannot promise
and is the reason the cursor exists.

*Stops at.* `RETURNING` returns the rows as written, not a projection.

### P4 — Many-to-many, and `has_many through`

`#[record(has_many(Tag, through = ArticleTag))]`. Two batched reads rather than
one; the join table's rows are the intermediate key set.

*Test.* Oracle against the equivalent explicit two-step, over generated data.

### P5 — Nested eager loading

One level of nesting, then a depth limit with a named refusal rather than
unbounded recursion — the same shape as the subquery depth note.

### P6 — Batch: several statements, one round trip

A `Batch` RPC taking a list of independent operations. Distinct from a
transaction: a batch is a network optimisation, a transaction is an atomicity
guarantee, and conflating them is how people end up thinking they have one when
they have the other. The RPC should make that distinction impossible to miss —
probably by requiring the caller to say which they mean.

*Test.* Measure it. The claim is "fewer round trips", so the test is a round
trip count and a wall-clock comparison against the same operations issued
singly, reported with spread.

## What this plan does not do

It does not close the whole table. Window functions, CTEs, views, arrays,
full-text search and set operations are all real absences and none of them is
in the six. They are planner and kernel work of a different size, and doing any
of them before predicate writes and wire relations would be building the
interesting thing instead of the needed one.

It also does not add validations, changesets or lifecycle hooks. Those are a
design question rather than a missing function — where does validation live
when three clients in three languages share one catalog? — and the honest
answer is that we have not worked it out. Probably the catalog, as declarative
constraints beside `CHECK`, so the answer is the same in all three. That is a
design note somebody should write before any of it is built.

Automatic timestamps and soft-delete conventions are deliberately left out of
the six as well: both are sugar over things that already work (a default, a
partial index), and sugar is the right thing to add *after* the shape of the
write path stops changing, not during.
