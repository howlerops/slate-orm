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

> **Since this audit was written, this is built in the kernel and the record
> layer.** The description below is what was found, kept because the reasoning
> is what justified building it; see P1. It is not yet on the wire, so the
> three clients still have the problem described here.

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
| Generated migrations from a schema diff | Drizzle Kit, Prisma Migrate, Alembic autogenerate | `slate-kernel/src/migrate.rs` plans and applies a diff but nothing *writes* the target catalog for you |
| Generated *types* from the catalog | Drizzle, Prisma | `scripts/codegen.py` generates the schema declaration for all three clients from `slate-serverd --print-schema`, and CI diffs it; what it does **not** generate is a typed row — a query still returns `Value`s, not a `Book` — see below |
| Validations / changesets / lifecycle hooks | Ecto, ActiveRecord, SQLAlchemy events | `CHECK` is a declarative constraint in the catalog and covers part of this; what it cannot do is name a column, report more than one failure, or reach a client. Designed out in [`validation.md`](validation.md), which recommends refusing hooks |
| ~~Automatic `created_at` / `updated_at`~~ | ActiveRecord, Ecto, Prisma | **Built** — `#[record(created_at)]`, or `managed = "created_at"` in the daemon's TOML; see below |
| Soft delete as a first-class concept | ActiveRecord (gems), Prisma (pattern) | partial indexes support `WHERE deleted_at IS NULL` well; no convention on top |
| Window functions | SQLAlchemy, Drizzle, Diesel | aggregates are `Count, CountColumn, Min, Max, Sum, Avg, CountDistinct` |
| CTEs / recursive queries | SQLAlchemy, Drizzle, Diesel | no plan node |
| Set operations (`UNION`/`INTERSECT`/`EXCEPT`) | all | refused by name in the SQL front end; no spec node |
| Views | Drizzle, SQLAlchemy | none |
| Array / list column type | Drizzle, SQLAlchemy, Ecto | `ValueType` has no `Array` |
| Full-text search | Drizzle, SQLAlchemy | none; `LIKE`/`ILIKE`/regex only |
| Factories for seed data | Drizzle, Prisma (seed scripts), ActiveRecord (FactoryBot) | `slate-serverd --seed` loads a static TOML fixture; nothing *generates* rows, and no client or the Rust library can seed at all — see below |

> **Built: automatic timestamps.** `#[record(created_at)]` and
> `#[record(updated_at)]` on an `i64` field, or `managed = "created_at"` on a
> column in `slate-serverd`'s TOML. The store fills both on insert; after that
> `updated_at` moves on every write and `created_at` does not.
>
> Three decisions worth stating, because each had a defensible alternative.
> **The caller's value is discarded**, not merged where null: honouring it is
> what makes a historical import possible and it lets a client break the one
> promise the column makes, silently. An import declares the column unmanaged.
> **It is not a `DEFAULT`**, because a default is a stored `Value` and the
> value wanted is "whatever the clock says now" — widening defaults to hold an
> expression means a second expression language in the schema layer, evaluated
> on a path that evaluates nothing, for two cases. **It is not in the schema
> fingerprint**, on the rule the fingerprint already follows: a client that
> disagrees still reaches the right column and finds out at once, because it
> reads back a value it did not write. That is the test a `CHECK` and a
> `DEFAULT` pass and a decimal's scale fails.
>
> Seconds, so the calendar functions read it. Two writes in the same second
> therefore share an `updated_at`, which means it cannot order writes within a
> second or act as a concurrency token; `update_if_unchanged` is that, and it
> compares the whole row. Nothing crosses the wire and no client changed: the
> server stamps, and a client sends whatever it likes into a slot that is
> overwritten.

> **Built, and narrower than the row it replaces: client codegen.** The row
> used to read "every client hand-declares its schema". That is no longer true:
> `scripts/codegen.py` reads `slate-serverd --config <file> --print-schema` —
> the *resolved* catalog, not the TOML, so it cannot reimplement the schema
> layer's resolution and disagree with it — and writes the declaration for the
> Python, Go and TypeScript clients. The explorer's three adapters use it, and
> a CI step regenerates and diffs before the conformance suite runs, so a
> schema change that nobody propagates is one red line rather than ninety-two
> refused cases.
>
> Two things came out of building it, and both are worth more than the feature.
> `--print-schema` was **not emitting a decimal column's scale** — the one
> property that never crosses the wire and is in the schema fingerprint, so a
> declaration built from that output was refused against any table with a
> decimal in it. The flag whose whole purpose is "what a client in another
> language has to restate by hand" omitted the only field nothing else could
> supply. And the Go and TypeScript adapters were sending **no schema check at
> all** — the clients had supported one since the SchemaCheck item, and the two
> adapters had never been given a declaration to send. Both now do.
>
> What this is not is what Drizzle and Prisma are actually known for. There is
> no generated *row type*: a query still answers a list of `Value`, and turning
> that into a `Book` is still the caller's loop. The generated declaration
> makes the ordinals right; it does not make them disappear. That is the
> remaining half of this row and it is a larger change, because it needs a
> decoder per table in three languages rather than a data literal.

> **Half-built: seeding.** This row said "none" and that was wrong.
> `slate-serverd --seed fixtures.toml` loads rows by column *name* under a
> superuser context, and `seed.rs` argues both choices at length: named rather
> than positional because a column added to the middle of a table silently
> re-points every positional value after it, and a command-line flag rather
> than a configuration section because a file gets copied between environments
> and one that quietly re-seeds a database as a superuser is a failure mode
> worth designing out.
>
> What is genuinely absent is the half the named tools are actually known for.
> There is no *factory*: nothing generates a plausible row, sequences an id, or
> builds a graph of related records, so a fixture of a thousand rows is a
> thousand lines of TOML somebody wrote. And it is reachable only from the
> daemon's command line — the Rust library has no seeding helper and neither do
> the Python, Go or TypeScript clients, so a test suite in any of them writes
> its fixture with the ordinary write path, which is not wrong and is not a
> feature either. The row is rewritten rather than removed: something exists,
> and it is not what the comparison is about.

> **Built: "Per-request logging and metrics".** `[observability] request_log`
> writes a line per call — method, gRPC status, time to the response head — and
> `summary_interval` writes per-method counters on a cadence. Both off by
> default. The row is removed rather than annotated because what the other
> seven offer here is a log line and a counter, and this is a log line and a
> counter. There is a metrics endpoint too now — `metrics_address`, Prometheus
> text format on a port of its own — so what remains absent is `tracing` with a
> subscriber, and that gap is recorded below rather than in a row that reads as
> absent.

> **Built: "Batch" and "`RETURNING` on a write".** Both rows are removed rather
> than annotated, on the same grounds as keyset pagination: both were correct
> when written. `Batch` is an RPC with a required `atomicity` and lives in all
> three clients; `RETURNING` is `WriteResponse.rows` behind a `returning` flag,
> on the two *predicate* writes only — see P3, where the item turned out to be
> a narrower feature than it was written as.

> **Built, and removed: the two relationship rows and the join-paging row.**
> Many-to-many and nested loading were built in Rust (P4, P5) and reachable
> only from there, so these rows were once narrowed to say *on the wire* with
> the proto as their evidence. N1 closed that: a request carries a path of
> relationships and the answer comes back one level per step, in all three
> clients. N3 closed the third: `JoinQuery` carries `after` and `paged`, and a
> page of a join is a page of its driving table.
>
> Removed rather than annotated, on the same grounds as keyset pagination and
> `Batch` above: the evidence each row cited — "`RelatedRequest` carries one
> `Relation`", "`JoinQuery` has `offset = 3` and no cursor field" — is now
> false, and a table of gaps whose evidence is false is worse than no table.
> The reasoning is not lost; it is in N1 and N3 below, and in
> `docs/paging-a-join.md`.
>
> That was the third time a capability existed in Rust a release ahead of the
> three clients, which is why N1 was first. **The relationship family is now
> closed** — `load_related`, `load_one_related`, `load_related_through`,
> `load_through` and `load_nested` all have a wire path, and every one of them
> is exercised by the conformance corpus.
>
> Not the same as "nothing is Rust-only", which was the first thing written
> here and is false. **Migrations are Rust-only**: `slate-kernel/src/migrate.rs`
> plans and applies a schema diff and none of the eighteen RPCs carries one, so
> a deployment that wants a migration runs Rust or runs nothing. That is a
> deliberate shape rather than an oversight — a migration is a privileged
> operation against a whole catalog, and putting it on the same wire as a
> tenant's reads is a decision nobody has argued for yet — but it is a thing
> the clients cannot do, and the row below about *generated* migrations is
> about something else again.

> **Built: "Keyset pagination on the wire".** `Query.after` carries the cursor,
> `Query.paged` asks for the next one, and `QueryResponse.next_cursor` returns
> it. All three clients have `page`. The row is removed from the table above
> rather than left with a note, because unlike the chains row it was correct
> when it was written.

> **Withdrawn: "Chains (3+ table joins) on the wire".** This table listed it as
> missing, on the evidence that "proto has `Join`, no chain RPC; `chain`
> appears only in comments". Both halves are literally true and the conclusion
> is wrong. `JoinQuery.inputs` is `repeated JoinInput`, `service.rs` says
> *"a join or a chain: one request shape, two kernel paths"* above the handler
> that routes them, and all three clients have chain tests already — Go's
> `TestGroupingAChain`, Python's `test_grouping_a_chain`, TypeScript's
> `"grouping a chain"`. A chain needs no separate RPC because a chain is a join
> with more inputs.
>
> A `grep` for a *name* is weak evidence of a missing *capability*, and this is
> what that looks like when it goes wrong.
>
> The real gap behind the row was narrower and is now closed: the three-SDK
> conformance runner did not compare the clients on a chain, because the demo's
> `/api/join` was hard-coded to two tables. There is a `/api/chain` now and five
> cases over it.

## Refused, with the reasoning

Not gaps. Each of these is a decision with a written argument, and the argument
is the thing to attack if you disagree.

- **Lazy loading.** The single largest source of accidental N+1 in every ORM on
  the list. Not having it is why `load_related` has no fallback to fall into.
- **Unit of work / identity map / dirty tracking** (SQLAlchemy's `Session`).
  Implicit flush ordering is the hardest thing in SQLAlchemy to reason about,
  and the whole point of this layer is that a write is a write. The explicit
  `transact` closure covers the cases people actually use it for.
- **`NOT EXISTS`.** Correlated by nature, like `EXISTS`, and refused with the
  `NOT IN (SELECT …)` form that asks the same question.

  **`NOT IN` was on this list and is now built**, on a claim withdrawn: that
  `Expr::In`'s three-valued rule made "the negation a reader expects and the
  one the kernel would give differ exactly where nulls are involved". Standard
  SQL's `NOT IN` is three-valued in exactly the same way — the surprise is
  SQL's, not this implementation's — and `Truth::negate` maps unknown to
  unknown, so `Expr::Not` over `Expr::In` is the standard's own answer. The
  part that needed checking rather than asserting was the planner:
  `Expr::conjuncts` stops at a `Not`, so no access path is derived from the
  `In` inside one, which is the only correct answer since the complement of a
  set of points is not a range.
- **SQL on the wire.** The wire carries a spec. That is what makes the planner,
  `EXPLAIN` and the security compilation possible at all.

## Forbidden by the architecture

- **Multi-writer and cross-shard transactions.** One writer holds the lease.
- **`UNION` across two independently planned statements with deduplication.**
  There is nowhere for the dedup to happen.

## The first plan — six pieces, all built

Six pieces, ordered by value over cost. Each named what it was, how it would be
tested, and where it stopped. **All six are now built**, and each carries a
block above its original text saying what it became; the original text is kept
underneath because the reasoning is what justified the work, and because two of
the six turned out to be different features than they were written as.

Three of them were also worth more as a lesson than as a feature. P3's
`RETURNING` shrank to the predicate writes once it was clear that an insert's
`RETURNING` over this wire hands the caller its own request back. P4's
many-to-many needed no new capability at all — it is the composition of two
relationships the derive already emitted. P5's depth limit was refused on
inspection: a nesting level is a type parameter, so there is nothing to limit
until an `include` list arrives as data.

**The recurring finding across all six is not in any of them.** Five separate
times on this branch the code was right and the *check* was the problem: a
`protoc` guard that had never once run because no CI job installed `protoc`, a
`max_batch_operations` that shipped as a field, a config key and an `if` with
nothing exercising any of the three, a durability check served by a replica one
poll behind, a mutation harness whose test filter did not match the test it
needed, and four mutation survivors that were all the same unasserted schema
claim in three languages. That ratio — five check defects to roughly two code
defects — is the thing to carry into the next six.

### P1 — Predicate writes in the kernel — **built**

> Done, in `RecordTransaction::delete_where` and `update_where`, and reachable
> from the record layer as `Records::delete_records_where` and
> `update_records_where`. Fifteen kernel tests and two record-layer ones;
> eighteen mutations, no survivors. ~~Still kernel-only: nothing crosses the
> wire yet, which makes it P3's neighbour rather than finished work.~~ **No
> longer true:** `DeleteWhere` and `UpdateWhere` RPCs carry both, with
> `RETURNING`, and all three clients call them. Twelve wire tests, eight to
> nine per client, five conformance cases. The kernel methods now return the
> rows rather than a count — `.len()` is the count — because they were already
> in memory and a caller that named a condition has no other way to learn what
> it hit.

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

### P2 — Relations on the wire and in the three clients — **built**

> Done, as a `Related` RPC — the server-side option, as recommended. All three
> clients have it, on a session and inside a transaction, and seven cases in
> the three-SDK conformance runner compare them: an empty group, a repeated
> key, both directions, and the parents read as `reader`, where the row policy
> hides a book and every client has to lose it identically. Nine tests in
> Python, eleven in Go, ten in TypeScript, and thirteen mutations across the
> three client implementations with no survivors.
>
> The read-count assertion the plan asked for is
> `TestRelatedIsOneRequestHoweverManyParents` in Go and
> `test_it_is_one_request_however_many_parents` in Python: fifty parents, one
> request, counted at the transport rather than inferred.
>
> One correction to the plan below: *"carry the relationship declaration in the
> catalog"* was already true. A foreign key encodes both directions, so no new
> schema type was needed, and adding one could only have disagreed with the key
> it duplicated.

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

### P3 — Chains, keyset pagination and `RETURNING` on the wire — **built**

> The chains third of this is withdrawn: they have been on the wire since
> `JoinQuery.inputs` became repeated, and all three clients test them. See the
> withdrawal above the P1 section. What was real is now built — the conformance
> runner compares the three on a three-table chain, five cases, all four join
> types plus a `reader` whose row policy hides a book *and* the sale hanging
> off it.
>
> Keyset pagination is built too: `Query.after` and `Query.paged` on the
> request, `QueryResponse.next_cursor` on the response, `page` in all three
> clients, and eight conformance cases including the three refusals.
>
> ~~`RETURNING` is what is left of this item, and is still real:
> `WriteResponse` carries an affected count and no rows.~~ Built, and narrower
> than the item assumed. `WriteResponse.rows` and a `returning` flag exist, on
> the two *predicate* writes only. The item said "returns the rows as written,
> not a projection", which takes for granted that rows-as-written differ from
> rows-as-sent. Over this wire they do not: the proto's `Row` is full width and
> its nulls are values a caller meant, so no `DEFAULT` is applied, and there is
> no auto-increment, no trigger and no generated column. `RETURNING` on an
> insert would hand the caller its own request back. On a predicate write it is
> the opposite — the caller named a condition, and for a delete the answer
> stops existing the moment the write lands.
>
> So all three thirds are now done, and one of them turned out to be a
> different feature than it was written as.

### P3 — Chains, keyset pagination and `RETURNING` on the wire

Three small things that are all "the kernel has it, the wire does not". Each is
a proto message and a handler, and each already has kernel tests.

*Test.* Conformance runner for all three. For keyset pagination specifically,
the property that matters is that paging through with a cursor visits every row
exactly once under concurrent inserts — which `LIMIT`/`OFFSET` cannot promise
and is the reason the cursor exists.

*Stops at.* `RETURNING` returns the rows as written, not a projection.

### P4 — Many-to-many, and `has_many through` — **built**

> Done, and smaller than it looked. `load_related_through` has the bounds
> `P: Related<J>, J: Related<C>` and nothing else — a many-to-many *is* the
> composition of the two relationships the derive already emits, so the
> capability needed no new declaration. `#[record(has_many(Tag, through =
> ArticleTag))]` exists and emits a `Through` impl, but only as a *name*: a
> blanket impl over the two halves is refused by `E0207` because the join type
> is unconstrained, so which table stands in the middle has to be stated. Six
> tests, six mutations; the wrong join table is a compile error rather than a
> wrong answer.

### P4 — Many-to-many, and `has_many through`

`#[record(has_many(Tag, through = ArticleTag))]`. Two batched reads rather than
one; the join table's rows are the intermediate key set.

*Test.* Oracle against the equivalent explicit two-step, over generated data.

### P5 — Nested eager loading — **built**

> Done as `load_nested`, which returns each parent's children paired with
> *their* children. `load_related_through` is now one line of it — the two
> differ only in whether the middle is kept — so there is one regrouping rather
> than two.
>
> The depth limit this item asked for is not there, and should not be: each
> level of nesting is a type parameter, so a call's depth is fixed when it
> compiles and a caller cannot ask for a thousand without writing a thousand
> types. The refusal would guard nothing. It *would* be needed for an `include`
> list whose depth arrives as data — `["comments.author.employer"]` on the wire
> — and nothing here parses one.

### P5 — Nested eager loading

One level of nesting, then a depth limit with a named refusal rather than
unbounded recursion — the same shape as the subquery depth note.

### P6 — Batch: several statements, one round trip — **built**

> Done, as a `Batch` RPC with a required `atomicity`.
> `ATOMICITY_UNSPECIFIED` is refused with a message naming both options, which
> is the "impossible to miss" this item asked for: the two guarantees differ
> only when something fails, so a zeroed request must mean neither.
>
> `INDEPENDENT` reports one result per operation and a failure is one of those
> results; `ALL_OR_NOTHING` reports none, because they all happened or the
> request failed. An atomic batch may join a caller's open transaction and does
> not commit it; an independent one inside a transaction is refused as two
> contradictory requests.
>
> Measured, as the item required: fifty single inserts over loopback took 46–49
> ms and the same fifty as one batch took 4.9–5.1 ms across five runs, **9.4×
> to 10.0×**. Round trips are counted rather than timed — 50 against 1, by
> construction — and the wall clock is reported rather than asserted, because a
> threshold on a shared runner is a flake waiting for a slow morning.

### P6 — Batch: several statements, one round trip

A `Batch` RPC taking a list of independent operations. Distinct from a
transaction: a batch is a network optimisation, a transaction is an atomicity
guarantee, and conflating them is how people end up thinking they have one when
they have the other. The RPC should make that distinction impossible to miss —
probably by requiring the caller to say which they mean.

*Test.* Measure it. The claim is "fewer round trips", so the test is a round
trip count and a wall-clock comparison against the same operations issued
singly, reported with spread.

## The next plan

Six again, ordered by value over cost, and drawn from what the last six left
behind rather than from the table above. Four of them are debts the previous
plan created: a capability that stops at the Rust edge, a response with no
ceiling, a token only one path can see, and a demo that shows none of it. That
is what a plan looks like when the features landed and the edges did not.

Where an item rests on reasoning rather than a run, it says so. A hypothesis
here is labelled a hypothesis, and the first step of that item is to make it
fail or withdraw it.

### N1 — `through` and nested loading on the wire — **built**

> **Done**, as the recommended shape: `repeated RelatedStep path` on the
> request, and a flat `repeated Level levels` back. Three clients grew
> `related_path` (the tree, keeping every level) and `related_through` (the far
> rows, dropping the middles), and the conformance corpus grew three cases —
> 83, and the three SDKs agree on all of them.
>
> **The response shape was the part worth designing, and the thing that made it
> work is one number per level.** Each `Level` carries `key_ordinal`: where, in
> the *previous* level's rows, this level's grouping key is found. That is what
> lets a client rebuild the tree without holding the catalog — take a row from
> the level above, read the value at that ordinal, look it up — and it is the
> same three lines in Python, Go and TypeScript rather than three different
> guesses about the schema.
>
> **A path needs a depth limit, and `load_nested` was right that it did not.**
> That function's own documentation argued it needed none, because each level
> is a type parameter and "a limit nobody can exceed is a limit nobody
> maintains" — and then named the form that would need one: a list on the wire
> whose depth a request chooses. This is that form. One step is one read, so an
> unbounded path is a caller deciding how many times the server goes to
> storage. `Limits::max_relation_depth` defaults to four, and the Rust comment
> that predicted it now points at it.
>
> Three things that went wrong and are worth keeping:
>
> - **A middle row that related to nothing.** `related_through` was first
>   written as "the rows with no children", which is wrong: a shelf with no
>   copies has no children either, so it came back where a copy was asked for.
>   It is "the rows `len(path) - 1` levels down". Caught by a fixture with a
>   deliberately bare middle row; the two readings agree on every other input.
> - **A fixture where the right ordinal is zero proves nothing.** A mutation
>   ignoring `key_ordinal` entirely passed the whole Go suite, because that
>   fixture's relating column *was* ordinal 0. Only the Python suite caught it,
>   and only because its tables are tenant-scoped so `id` sits at ordinal 1 by
>   accident. Both other fixtures now put a column in front of `id` on purpose.
> - **A `[[tables.foreign_keys]]` attaches to the nearest `[[tables]]` above
>   it.** Adding the demo's `editions` table above the existing key silently
>   moved `sale_book` off `sales`, and six unrelated conformance cases started
>   refusing. The runner found it on the first run after the change.
>
> *Stops at* a path of declared relationships, as written below. No predicate,
> ordering or limit per level.

`RelatedRequest` carries one `Relation` and `repeated Value keys`. P4 and P5
built `load_related_through`, `load_through` and `load_nested`, and every one of
them is reachable only from Rust. A Python, Go or TypeScript caller who wants an
article's tags issues two `Related` calls and regroups by hand — which is the
same shape as the N+1 this whole layer exists to prevent, one level up.

This is the third time: `Related` itself, predicate writes, and now these. The
first two were caught a commit later and closed. This one has been open since
P4 landed, and it is first for that reason rather than because it is the
largest.

*Build.* The design question is whether `RelatedRequest` grows a `repeated
Relation` — a path, resolved level by level, one round trip — or whether a
second RPC carries nesting. **Recommendation: a path on the existing request**,
because the server already deduplicates each level's key set and a second RPC
would duplicate the grouping logic that `RelatedResponse` already specifies. The
response then needs a shape for "groups of groups", which is the part worth
designing carefully: a flat list of levels with parent-key back-references
avoids nesting the message type, and is what the Rust `load_nested` signature
`Vec<Vec<(C, Vec<G>)>>` flattens to anyway.

*Test.* The conformance corpus, which is where a three-way disagreement about
grouping will show up and nowhere else. A read-count assertion per level, the
same way `TestRelatedIsOneRequestHoweverManyParents` does it for one level —
the claim is "one read per level, not N", and the levels are where that claim
gets easier to break. An oracle against the two-step the clients write by hand
today.

*Stops at.* A path of declared relationships. No predicate on an intermediate
level, no ordering or limit per level — both are real and both want the level
to be a `Query` rather than a `Relation`, which is a bigger message than this.

### N2 — A ceiling on `returning` — **built**

> **The hypothesis reproduced, exactly as written below**, at 8,000 rows of
> about a kilobyte each: `OutOfRange`, *"decoded message length too large:
> found 8423749 bytes, the limit is: 4194304 bytes"*, and **zero rows left in
> the table**. The delete committed and destroyed all 8,000; the caller got an
> error naming a decode limit, which says nothing about the write.
>
> Fixed as `Limits.max_returned_rows`, default 10,000, checked in the kernel's
> `matching_rows` — the one place both predicate writes decide which rows they
> touch, and upstream of every write on every path. The cap is on the
> *answer*: a `DELETE … WHERE` over a million rows is still a million rows
> deleted, and only asking for them back is refused.
>
> Two corrections to the item as written. The refusal is a `KernelError`
> rather than a server check, because it has to be inside the transaction and
> that is the error type the transaction closure returns — and putting it in
> `matching_rows` made it one mechanism across the lone, batched and
> in-transaction paths instead of three. And "refusing after the scan but
> before the commit is the only ordering that keeps the caller's world
> consistent" was *too weak*: for a lone write a rollback saves it either way,
> but inside a caller's open transaction there is no rollback to lean on, and
> the write stays in their buffer. A mutation moving the check after the write
> loop is caught by exactly one test — the in-transaction one — which is why
> that test earns its place.
>
> Ten mutations, one survivor, and the survivor was an equivalent mutant: it
> *added* a post-write check while the real one still stood, so the code was
> unreachable. Established by building the non-equivalent version — pre-check
> removed, post-check in its place — and watching it fail by name.

**Hypothesis, not a finding.** `WriteResponse` is unary and `returning` puts
every matched row in it. Nothing in `Limits` bounds the count — `grep` for
`max_encoding_message_size` and `max_decoding_message_size` across
`crates/slate-server/src/` and `crates/slate-serverd/src/` returns nothing, so
tonic's defaults apply: no encode cap, a 4 MiB decode cap at the client.

If that reading is right, `delete_where(…, returning = true)` over a large match
**commits and then fails to deliver**: the rows are gone, the server encodes a
response the client refuses, and the caller sees a decode error for a write that
succeeded. That is worse than a refusal, because the error says nothing about
the write having landed — it is exactly the unknown-outcome case the error
taxonomy tells callers not to retry, manufactured by us out of a known outcome.

*Build.* First reproduce it, at whatever row count crosses 4 MiB; if it does not
reproduce, withdraw this item in place and say what actually happens. If it
does: a `max_returned_rows` in `Limits`, checked **before** the write applies,
refused as a resource limit naming the count and the cap. Refusing after the
scan but before the commit is the only ordering that keeps the caller's world
consistent.

*Test.* The refusal, with the cap forced off to confirm the test fails by name.
A test that the refused write did not land, which is the actual property. The
ordering matters more than the limit and is the thing to mutate.

*Stops at.* One number, on the server. Streaming a predicate write's rows back
the way `Query` streams is the general answer and is a different RPC shape.

> **Where it actually stopped**, beyond the above: the cap is a row count, and
> what makes a response undeliverable is bytes. One row holding a large enough
> blob passes a count cap of 10,000 and fails to encode anyway. A byte-exact
> bound means encoding the rows before the write is allowed to commit, which is
> a different and larger change.
>
> And the memory cost the count cap does *not* address: `matching_rows`
> collects every matched row whether or not `returning` was asked for, so a
> `DELETE … WHERE` over ten million rows still materialises ten million rows.
> That predates this item and is untouched by it. It belongs in
> `ExecutionLimits` beside `max_sort_rows`, as a per-store memory guard rather
> than a per-request answer cap — the two are different limits with different
> defaults, and merging them would either refuse legitimate large deletes or
> leave the wire defect open.

### N3 — Keyset paging over a join or a chain — **built**

> **Done**, and not as a refusal. The design note the item demanded is
> `docs/paging-a-join.md`, written before the code; the one line it concludes
> with is **a page of a join is a page of its driving table**. The cursor is
> input 0's primary key, `limit` counts input-0 rows, and every joined row
> those rows produce comes back with them.
>
> **The property is one line because the design was chosen so it would be.**
> Every joined row derives from exactly one input-0 row — that is what
> left-deep means — so "every input-0 row is read by exactly one page" gives
> "every joined row is returned by exactly one page", and the first is what
> `Query::after` already holds on that table's own key range. Paging is
> implemented as giving input 0 the window it already honours; there is no new
> execution mode.
>
> **Three measurements chose it, and two contradicted the code's own comments.**
>
> 1. Only two of three algorithms keep the driving side's key order, and none
>    does for a right or full outer join. So the cursor is *not* a position in
>    the output — one that was would page differently depending on what the
>    cost model picked that day.
> 2. A side's `limit` and `offset` are **honoured, not ignored**, and the three
>    algorithms agree exactly on which rows a windowed side yields. Both
>    `Join`'s doc comment and `JoinInput.query`'s field comment said the
>    kernel ignored them. Nothing was user-visibly broken — the wire refuses
>    those fields — but the stated reason was false, and this design rests on
>    the true behaviour. Both are corrected.
> 3. A side's `sort` is honoured where that side streams and silently dropped
>    where it is hashed. Worse than either, and the real reason the wire
>    refuses it.
>
> Sixteen mutations, two survivors, both closed. The survivors were missing
> tests rather than bugs, and both were the same shape as N1's: a fixture where
> the cost model always picked the hash join left the nested loop's boundary
> tracking untested, and a test comparing the *union* over pages said nothing
> about where the pages divided, so a chain that ignored its page size passed.
>
> **A mutation also found a defect in the test suite itself.** Dropping the
> cursor made one test **hang** rather than fail: it walked pages in an
> unbounded loop and the cursor came back non-empty forever. A test that hangs
> under a mutation hangs CI, which is strictly worse than one that fails. Every
> walk in the file is bounded now, and the bound is an assertion.
>
> *Stops at* what the note says: no per-level predicate or ordering, and the
> page's *fan-out* is unbounded even though its depth in driving rows is not.

`Query` has `after` and `paged`; `JoinQuery` has `offset = 3` and no cursor.
Paging a join is therefore offset paging, which is the thing P3's cursor exists
to replace — a row inserted between pages shifts every later page by one, and a
caller walking a join sees a row twice or not at all.

*Build.* The design question is what a join's cursor *is*, and it has no obvious
answer: the driving side's key is not unique after a fan-out, so a cursor over
it either skips the rest of a group or repeats it. The honest options are a
composite cursor over the ordering columns plus a tiebreaker from each input's
key, or a refusal that says paging a join needs an `ORDER BY` whose columns are
unique and names why. **Write that down before building either.** An undesigned
cursor that is subtly wrong under concurrency is worse than an offset that is
obviously wrong under concurrency.

*Test.* The property P3 named and did not get to test on a join: paging through
with a cursor under concurrent inserts visits every row exactly once. That
property is the whole item; if a design cannot be tested that way, it is the
wrong design.

*Stops at.* Whatever the design note concludes. This item may legitimately end
as a named refusal.

### N4 — The reason token on the lone path — **built**

> **Done in all three clients**, and N2 is what made it worth doing rather than
> merely true. `PREDICATE_WRITE_TOO_LARGE` is the token for a refusal whose
> whole point is that the caller can tell it apart from every other
> `RESOURCE_EXHAUSTED` — a join build, a group count, a distinct count, a sort
> — and act on it. Verified before building: the token was on the wire, in
> `grpc-status-details-bin`, and `SlateError.reason` read `""`.
>
> Each client decodes the same blob captured off a running head node, so the
> three are checked against one artefact rather than three transcriptions of
> the format. The conformance runner compares `reason` on every refusal case it
> already had, which turned five existing cases into three-way token
> comparisons without inventing a sixth.
>
> Three things worth knowing, each of which decided the implementation:
>
> - **Python cannot generate `google.rpc`.** The obvious implementation breaks
>   `import slate` outright for anyone who also has `googleapis-common-protos`
>   — `grpcio-status`, any `google-cloud-*`, the OTLP exporter — because two
>   descriptors for `google/rpc/error_details.proto` that are not identical are
>   a hard error in protobuf's default pool, and this repository's copy
>   declares one message where upstream declares ten. Demonstrated, then
>   avoided: `_details.py` declares the three messages in a pool of its own.
> - **Go must use `genproto`, and its own generated copy was a trap.**
>   `clients/go/internal/pb/google/rpc` existed, was imported by nothing, was
>   generated by nothing, and *panics the process at init* if imported —
>   grpc-go already registers `google/rpc/status.proto` through `genproto`.
>   Demonstrated, then deleted.
> - **The wiring is a separate test from the decode.** A mutation that stopped
>   `fromRPC` asking for a token survived every decoder test in the Go suite
>   and was caught only by the conformance runner. All three clients now have a
>   test that goes through their error constructor.
>
> *Stops at* the token, as written below. Also stops at `NOT_LEADER`: it is a
> token now, but `slate-leader` stays the discriminator, because a client that
> reads the trailer and not the blob still follows the redirect and a working
> mechanism is not worth churning.

All three clients drop `grpc-status-details-bin`, each with a comment saying so.
The stable reason token therefore reaches a caller only when the failure was
*batched*, because a batch has to put it in the message body. Three clients now
have a `reason` field populated for exactly one kind of failure, with the
asymmetry written on the field.

*Build.* Decode the details blob on the lone path, in three languages. Python
and TypeScript already carry the generated types; Go does too. The work is
small and was skipped because nobody had needed it, which is a reason to do it
now rather than a reason it stays skipped.

*Test.* Conformance cases comparing `reason` across the three for a lone
failure, which is the only check that stops one client decoding it differently.
Mutation: drop the decode, in each, and a named case must fail.

*Stops at.* The token. Not the rest of the details message.

### N5 — The demo shows none of the last three features — **built**

> **Done.** Three panels, and each shows the thing its feature is *for* rather
> than that the feature exists:
>
> - **Predicate writes** runs `DELETE … WHERE` or `UPDATE … SET … WHERE` and
>   lets a visitor turn `returning` off. With it: the rows as they were. Without
>   it: a count, and a note saying that is all that is left of them — which is
>   the argument for `returning` made by its absence, as the item asked.
> - **Batches** runs the same three writes, one of which collides, under *both*
>   atomicities and shows them side by side. `independent` succeeds with the
>   failure as one of its outcomes and leaves three rows; `all-or-nothing`
>   fails the call, has no per-operation outcomes to report, and leaves one.
>   The difference is the only thing a batch has to teach.
> - **Relationships** walks `sales → books → editions` — two steps in opposite
>   directions, one request — and toggles between the tree and the far rows.
>   Book 12 has no editions, so the tree shows a level that is present and
>   empty where `through` shows nothing at all.
>
> Four new e2e checks, in the suite that already ran in CI. 22 pass.
>
> **Two bugs in the panels, both found by the e2e rather than by looking.** The
> two atomicities were two concurrent queries against one database, and the
> handler clears and re-seeds the same rows — so they raced, and whichever
> arrived second saw the other's half-finished state and came back a refusal.
> They run in sequence now, which is also the only way the numbers they report
> mean anything. And a `<For>` over a freshly built tuple array gave every item
> a new reference each render, so one of the two columns rendered its heading
> and nothing else; two columns are now written out rather than looped.
>
> **The expected counts were guessed and wrong** — 2 and 0 against the actual 3
> and 1. Read off the adapter in the end, which is what the test should have
> done first.
>
> *Stops at* the demo, as the item said. No new server surface: every endpoint
> these panels call already existed and was already compared by the corpus.

The frontend has no UI for predicate writes, for batch, or for relationships
beyond one level. The adapters serve `/api/batch` and the corpus compares it; a
visitor cannot see it. Three features shipped, tested from three clients, and
invisible on the page that exists to show the surface.

*Build.* A predicate-write control that shows the rows it returned — that is
the feature, and a count would show nothing that a delete-by-key does not. A
batch control that runs the same operations under both atomicities and shows the
difference, because the difference is the only thing a batch has to teach.

*Test.* The browser e2e, which already exists and already runs in CI.

*Stops at.* The demo. No new server surface comes out of this item; if one is
needed, that is a finding and belongs in its own entry.

### N6 — The numbers nothing here measures — **built**

> **Done**, and the first claim came back the *opposite* way round.
> `examples/batchbench` runs 100 single inserts against one batch of 100, five
> runs, once per client, against one head node. Python **31.6×**, Go **21.4×**,
> TypeScript **34.0×** — against the Rust wire test's 15×.
>
> **The prediction below is withdrawn.** It said a client's multiplier would be
> *smaller* than the wire's because clients add per-request work a batch does
> not save. The arithmetic was backwards: that work is per *request*, so it
> multiplies the one-at-a-time arm by a hundred and the batched arm by one, and
> widens the gap. A client pays more per round trip than `tonic` does, which is
> why saving round trips is worth more to a client and not less.
>
> **The cap question, decided from the number rather than guessed.** The
> default cap is 1,000 operations; a batch that size is ~30 ms of the measured
> work, and the refused round trip that a client-side check would save is one
> RPC — about 1 ms, or 3% of it. Three percent does not buy a client-side copy
> of a server-configurable limit: two numbers that can disagree is a client
> refusing a batch a differently configured server would have taken, which is a
> worse failure than a wasted millisecond. Left server-side, as this item
> allowed.
>
> *Stops at* writes on a loopback. Reads are not measured, and the absolute
> numbers are the conservative end because a loopback makes the round trip a
> batch saves as cheap as it ever gets.

Two claims currently rest on inference:

1. **Batching helps a client.** The 9.4×–10.0× is a Rust wire test. The clients
   add per-request work batching does not save — schema claims, value encoding,
   per-operation table resolution — so their multiplier is smaller by an unknown
   amount. A README that says "a batch is a round trip" is making a claim about
   the clients on the strength of a measurement that did not go through one.
2. **A batch over the cap costs a round trip.** `max_batch_operations` is
   enforced server-side only. No client checks length before sending.

*Build.* One benchmark per client, same shape as the Rust one: N singles against
one batch of N, reported with spread across five runs. Then decide the cap
question *from the number*: if the refused round trip is cheap relative to the
batch it would have sent, a client-side copy of a server-configurable limit is
two numbers that can disagree, and the right answer is to leave it. Do not guess
which way that goes before measuring.

*Test.* The measurement is the test, reported as a range and not asserted
against a threshold — a wall-clock assertion on a shared runner is a flake
waiting for a slow morning, which is why the Rust one reports rather than
asserts.

*Stops at.* Reporting. No CI gate on a timing number.

### Smaller, and owed

Not plan items; things a session should pick up when it is already in the file.

- ~~**The deployed harness reads without demanding a snapshot.**~~ **Built.**
  `check.py` takes the sequence the writer reports for its first count and pins
  every later read to it with `Freshness.at_least`, then passes it through
  `expected.json` so the Go and TypeScript arms fold it into their session
  watermark with `Observe` / `observe` — which is what those two clients have
  instead of a per-read `freshness`, an asymmetry with Python that this
  recorded rather than worked around. `at_least` and not `latest`, because
  `latest` is the writer alone and would have sent every read to the writer,
  leaving the three replica checks passing by asking a replica nothing.
  Honestly: the staleness this defends against was observed once in CI and
  **could not be reproduced** — a replica poll 27× longer than the configured
  one produced no stale read at either scale. What is demonstrated is that the
  pin is enforced, not that it was needed on any run since. The failed
  reproduction also contradicts a measurement in `storage.rs` and that is
  written up as an open question in the example's README.
- ~~**No retrying `transact` in Go or TypeScript.**~~ **Built.** Go has
  `slate.Transact`, a generic free function because Go has no generic methods
  and a method would have to return `any`; TypeScript has `session.transact`.
  Both retry a conflict and nothing else, with the same defaults as Python —
  five attempts, 5 ms doubling to 500 ms, full jitter — because three clients
  disagreeing about how hard they try is its own bug.
- ~~**No per-request logging or metrics in `slate-serverd`.**~~ **Built.**
  `[observability] request_log` is a line per request and `summary_interval` is
  per-method counters on a cadence, both off by default, both written to stderr
  in the shape every other line the process emits already has. A `tower` layer
  rather than a `tonic` interceptor, because an interceptor sees the request
  and not the response, so it can log that a call arrived and not how it ended.
  What it still is not: `tracing` with a subscriber. There **is** a
  `/metrics` endpoint now — `[observability] metrics_address`, off unless set,
  Prometheus text format on a port of its own, announced as `METRICS <address>`
  beside the `LISTENING` line. It exports the counters and the head latency as
  a *histogram* rather than as the summary line's three quantiles, and that is
  the whole reason it exists: everything on that line is cumulative over the
  process's life, and a cumulative quantile cannot be subtracted to get the
  last five minutes or added to get three nodes. Bucket counts can. The `le`
  boundaries are each the exact ceiling of an internal bucket — 2ⁿ−1
  microseconds, so `le="0.001023"` rather than `le="0.001"` — because a round
  boundary falls inside a bucket and the count at it would be an interpolation
  labelled as a measurement. It has no authentication, which the docs page says
  plainly rather than papering over with a token in the same config file.
  It **does** report latency quantiles now — a p50, p90 and
  p99 beside the mean, from a per-method log-linear histogram eight buckets to
  the octave, which is what turns "something is slow" into "the slow thing is
  one call in a hundred". They are printed `p99_head<=` because a bucketed
  answer is an upper bound: within an eighth of the truth, never under it, and
  clamped to the exact `slowest_head` so a quantile cannot print above a
  maximum on its own line. It still times to the response *head*, so a streamed
  read's rows are not in any of these numbers.

  A failure raised in a **trailer** — a streamed read that answered rows and
  then died — is counted now, and counted apart as `late=`. It used to read as
  a success, on a comment claiming that catching it was "a per-row cost on the
  streaming path". That was asserted rather than measured and was wrong twice:
  a frame is a *message*, so at `rows_per_message = 256` the check runs once
  per 256 rows, and it costs 4.7 ns a frame — 375 ns on a 20,000-row scan whose
  measured drain is 41.3 ms, in a table whose own row-to-row spread is 3 ms.
  `late` is reported separately from `failed` because the two are different
  problems: a head failure is a request the server refused, and a late one is
  a scan that broke under it.
- ~~**No request id to correlate a client call with a server log line.**~~
  **Built.** A `slate-request-id` header, not a proto field: it belongs to the
  call rather than to the query, and adding it to nineteen request messages to
  say one thing would be the wrong shape. All three clients mint one per call
  — a UUID's randomness in hex — send it, and put it on every error they raise,
  so a caller holding a failure can grep the daemon's log for its line. A
  failure that never reached the server carries one too, and an id with no
  matching line is itself the answer: the call did not arrive.

  The server **filters it** to `[A-Za-z0-9._:-]` and 64 characters before it
  reaches a log line, which is the part worth knowing. It is
  attacker-controlled text going into an audit trail. gRPC refuses a newline in
  a header — measured, not assumed — but accepts a space, a quote and an `=`,
  so an unfiltered id of `x status=0` would give a reader two `status=` to
  choose between. Forged lines in a log are worse than none.

### W1 — Decimals and conditional updates across the wire — **built**

> The largest thing the first two plans left half-finished, and the only one
> that was half-finished on *purpose*: `Value::Decimal` and
> `update_if_unchanged` were both built in the kernel, exercised by the Rust
> ORM, and reachable from none of the three clients. A test existed to pin the
> absence — `value_to_proto` turned a decimal into the string
> `<unrepresentable decimal>`, and the test's own doc comment said it should
> fail when the field arrived and be rewritten rather than relaxed.

`Value` gains `decimal_value`, an `int64` count of the column's smallest unit;
the scale stays in the catalog, because a scale on the wire lets a client and
the catalog disagree about what a stored number means, which is the one thing a
decimal type exists to prevent. `UpdateRequest` gains `repeated Row expected`,
dispatching to `update_if_unchanged` on all three write paths — autocommit,
session actor, batch — and refusing an `expected` that is neither empty nor
exactly as long as `rows`, because zipping and stopping at the shorter turns a
caller's mistake into a silent partial condition.

All three clients have both. `Units` (Python, Go) and `units()` (TypeScript)
carry units and nothing else, with a renderer that takes the scale as an
argument; a `decimal.Decimal`, a JS `number` and a Go float are refused rather
than scaled, because there is no schema on the wire to reconcile their scale
against the column's. The conditional update is `update(..., expected=...)` in
Python and `UpdateIfUnchanged` in Go and TypeScript, where `update` is variadic
and cannot take an optional argument — an asymmetry recorded in all three
READMEs rather than smoothed over.

`slate-serverd` could not declare a decimal column at all: its `value_type`
listed eight types where `ValueType` has nine, so `type = "decimal"` was
refused as "not a type". That is the part worth remembering — the feature
existed in the library and was unreachable from the binary anybody runs, and it
was found by trying to write a test fixture rather than by any check.

Two mutations found missing tests, both the same shape and both about the
*transaction* path: deleting the expected rows there left Go and TypeScript
green, because each had a transaction test and each tested the happy path — an
unconditional update of an unchanged row and a conditional one do exactly the
same thing, so only a stale row tells them apart. The three-SDK corpus now has
both cases for that reason.

**What it did not do, and what closed it.** Nothing checked a client's declared
scale against the server's: the fingerprint did not hash it, because a scale
addresses no column, so a client with it wrong reached the right column and
rendered every value off by a power of ten, for ever, with no error anywhere.
That was called the sharpest edge in the feature, and it was.

It is closed, by making a decimal's scale the one thing in the fingerprint
that addresses no column. The rule it breaks is real and the reason to break it
is that the failure this prevents is *worse* than the failure the rule is
about — a wrong ordinal reads the wrong column and shows, a wrong scale reads
the right one and does not. What makes the exception affordable rather than
merely tempting is specific to a scale: changing one is already a refused
migration, so hashing it cannot invalidate a fleet the way hashing a `CHECK`
would, because there is no such change to make. It is hashed only for a decimal
column, so a table without one hashes as it did and no client of such a table
needs rebuilding.

### W3 — `delete_if_unchanged`, kernel to clients — **built**

> The last row of the README's optimistic-concurrency list, and one sentence
> long there: "Deleting a row somebody else just edited is the same class of
> mistake as overwriting it, and the same argument applies."

`RecordTransaction::delete_if_unchanged` in the kernel, `Records::remove_record`
over it, `DeleteRequest.expected` on the wire dispatching on all three write
paths, and `delete(..., expected=...)` / `DeleteIfUnchanged` in the three
clients — the same shape W1 gave the update, because the argument is the same
one.

**Where it stops being the update's twin**, which is the whole design content
of this item. A plain delete reports an absent key as `affected: 0`, because
"make sure this is gone" is idempotent and a caller asking that wants no error.
A conditional delete refuses it. The caller named what it expected to find, so
"somebody got there first" is an answer it wants rather than a count it will
read as success — which means `affected` for a conditional delete is always the
number of keys sent, since anything less would report a state the call already
refused. And the refusal is `NOT_FOUND` rather than `ABORTED`: a row that moved
can be re-read and the decision remade, a row that is gone cannot, so a caller
with a retry loop on `ABORTED` would spin. The three-SDK corpus compares that
distinction, which is the one place the three clients' error taxonomies meet on
a code no other endpoint produces.

**What it does not guard.** The named row and nothing else. A cascade may still
remove children the caller never saw, and there is no version of the field that
could cover them: the caller does not know what the deletion closure contains,
and the closure is deliberately computed without the row policy.

### W4 — decimal arithmetic in `Scalar`, and a decimal literal in SQL — **built**

> Two README lines, and they read as unrelated: "`price * quantity` is not
> expressible" and "`WHERE total > 19.99` parses as a float". They are the same
> item, because they are the same fact — a decimal is a count of a unit the
> *column* names, and neither the expression layer nor the parser knew that.

**The rule, in one sentence: an expression is expressible when its answer is
still a count of the same unit its operands were counts of.** `price *
quantity` is (cents times a plain count is cents), `price ± discount` is when
both are at one scale, `price / parts` is (truncating toward zero, because
there is no scale to round to). `price * discount` is not — cents times cents
is hundredths of a cent, at a scale no column has and nothing on the wire could
carry. Everything in the second set is refused at plan time by
`Scalar::decimal_scale`, before a row is read, with a message naming what to
write instead.

The refusals are the design content. The alternative — a `Value::Decimal {
units, scale }` that makes everything expressible — was rejected because the
scale would then live in two places and the order-preserving key encoding
depends on it living in one. Writing that down is what makes the refusals
defensible rather than arbitrary.

**Where the check lives, and the third thing this found.** It started in
`Reads::execute`, which every read passes through — and `explain` does not, so
a query planned cleanly, `EXPLAIN` returned a plan, and the read then failed.
A plan for a query that cannot run is worse than an error, because it looks
like an answer. Moving it into `Reads::plan` fixed that and made the join-side
and chain-step cases fall out for free: two explicit calls had been added for
those, on the belief that a side is planned rather than executed, and removing
them changed no test — which is what said they were dead. Mutation testing
found that; reading did not.

**And a defect found by reading.** `Mul` and `Div` asked only whether each side
*had* a scale. A decimal literal has none, so `price * Decimal(3)` read as
"money times a plain number" — the expressible case — while the evaluator saw
units times units, which has no arm, and returned `Null` on every row. Plan
time yes, run time null: the exact shape the plan-time check exists to
prevent, in the one place it was not asked. `Add` never had it because it
consults `mentions_decimal`; `Mul` and `Div` now do too.

The SQL half is `Value::decimal_from_str`/`decimal_to_string` in `slate-tuple`,
which split on the point rather than parsing an `f64` and multiplying: the
shortcut reads `"8.20"` as **819** cents, and that is a test in the tree rather
than a claim in a comment. Three silent wrong answers went with it — `WHERE
price > 19.99` returned nothing (the literal became a string, which sorts below
every decimal), `HAVING sum(price) > 60.00` admitted nothing (the aggregate's
type table said "integer or float"), and `UPDATE` failed on any table with a
decimal in it (the read-modify-write renders a row to text and reparses it, and
a decimal rendered as `Decimal(895)`). All three were found by giving the
workbench fixture a `price` column, which is the argument for doing that rather
than testing the literal against a purpose-built schema.

**What it does not do.** `AVG` over a decimal is still a float, deliberately.
The three SDKs can build a decimal expression but know nothing about scale, so
each adapter writes `Units(50)` having read the schema by eye; the conformance
corpus compares them on three such expressions and one refusal, which catches a
client that sent the *integer* 50. A client that believes the column is scale 4
is caught by the fingerprint instead — see W1 above, where that hole is closed
rather than merely named.

## What neither plan does

Neither closes the whole table. Window functions, CTEs, views, arrays,
full-text search and set operations are all real absences and none of them is
in either six. They are planner and kernel work of a different size, and doing
any of them before predicate writes and wire relations would have been building
the interesting thing instead of the needed one. That argument still holds for
the second six, for a narrower reason: every item in it is an edge the first
six left unfinished, and finishing what shipped beats starting what has not.

It also does not add validations, changesets or lifecycle hooks. Those are a
design question rather than a missing function — where does validation live
when three clients in three languages share one catalog? — and the honest
answer is that we have not worked it out. Probably the catalog, as declarative
constraints beside `CHECK`, so the answer is the same in all three.

That note now exists: [`validation.md`](validation.md). It agrees about the
catalog and disagrees about what the work is. `CHECK` already *is* a
declarative constraint in the catalog, so the rule language is not the gap —
what is missing is that a check cannot name the column it is about, cannot
report more than one failure at a time, and is not published to any client, so
every validation costs a round trip and no generated type can reflect one. It
also found, by running the examples rather than reading the parser, that the
kernel's regex node is unreachable from a check: `lang/pred.rs` has keywords
for `like`, `ilike` and `in` and none for `matches`, so format validation is
limited to `LIKE` patterns. Lifecycle hooks it recommends refusing outright.

Automatic timestamps and soft-delete conventions were left out of both, and
the ordering argument for that held: sugar is the right thing to add *after*
the shape of the write path stops changing, not during. The write path changed
twice in the first six — predicate writes, then batch — which is the evidence
for that ordering rather than a restatement of it. Timestamps were built once
it had stopped.

**The reason given here for calling them sugar was wrong, and building them is
what showed it.** This said they were "sugar over things that already work (a
default, a partial index)". A `DEFAULT` cannot express automatic timestamps: a
default is a stored `Value`, and the value wanted is whatever the clock says at
the moment of the write. Making a default hold that means an expression
evaluated per write in a schema layer that evaluates nothing — a second
expression language, for two cases. So `created_at` is not sugar over a
default; it is the thing a default cannot be, and it lives in the store's one
write choke point instead. The partial-index half of the claim is still
untested, because soft delete is still unbuilt.

Generated migrations and client codegen are also out of both, and are the two
table rows most likely to be worth a plan of their own next. Codegen in
particular would turn `SchemaCheck`'s run-time drift refusal into a
compile-time one in three languages, which is a bigger and better change than
anything in the second six — and a bad reason to delay the six edges that are
already half-built.
