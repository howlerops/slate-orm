# slate-orm

[![CI](https://github.com/howlerops/slate-orm/actions/workflows/ci.yml/badge.svg)](https://github.com/howlerops/slate-orm/actions/workflows/ci.yml)

A **record layer** for [SlateDB](https://slatedb.io) — typed schemas, secondary
indexes, query planning and row-level security over an ordered key-value store
on object storage.

Run a query against it in your browser:
**<https://howlerops.github.io/slate-orm/workbench.html>** — the kernel
compiled to WebAssembly, over 100,000 real New York taxi trips, with the
planner's `EXPLAIN` beside every answer. The
[front page](https://howlerops.github.io/slate-orm/) says what it is, and the
[documentation](https://howlerops.github.io/slate-orm/docs/) is a click from
either.

The closest blueprint is FoundationDB's Record Layer rather than an ORM. The
derive macro is a surface; the value is in the kernel underneath it, which owns
the keyspace, index maintenance, and the point where access policy is enforced.

> **Status: early.** The Rust record layer works end to end and is tested,
> including a cost-based planner, index-only scans, joins, aggregates, read
> replicas and S3-compatible storage. There is a gRPC head node with writer
> leadership, and typed [Python](clients/python), [Go](clients/go) and
> [TypeScript](clients/typescript) clients over it, each tested against a real
> daemon and required to agree with the other two. Everything below runs in CI
> on every push. Nothing here is published to a package registry and none of it
> is production-ready. See [Status](#status).

```rust
use slate_orm::{Aggregate, Expr, Query, Record, Records, SortKey, Value};
use uuid::Uuid;

#[derive(Record)]
#[record(table = "users", id = 1, tenant = "tenant_id")]
struct User {
    #[record(pk)] tenant_id: Uuid,
    #[record(pk)] id: u64,
    #[record(index(name = "by_email", id = 10, unique))]
    email: String,
    nickname: Option<String>,
    age: i64,
}

let txn = store.begin().await?;
txn.insert_record(&ctx, &user).await?;
txn.commit().await?;

// Column ordinals are generated as constants, so a predicate needs no
// fallible name lookup.
let recent: Vec<User> = txn
    .query_records(&ctx, &Query::all()
        .filter(Expr::eq(User::COLUMNS.email, Value::Str("a@example.com".into())))
        .sort_by([SortKey::desc(User::COLUMNS.age)])
        .limit(20))
    .await?;

// Counting and aggregating decode no records, and read no rows when an index
// holds the columns involved.
let total = txn.count_records::<User>(&ctx, &Query::all()).await?;
let oldest = txn
    .aggregate_records::<User>(&ctx, &Query::all(), &[Aggregate::Max(User::COLUMNS.age)])
    .await?;

println!("{}", txn.explain_records::<User>(&ctx, &Query::all())?);
```

## Layout

| crate | what it owns |
|---|---|
| `slate-tuple` | Order-preserving tuple encoding, the closed value model |
| `slate-schema` | Table, column and index definitions; the row body codec |
| `slate-kernel` | Keyspace, record store, planner, executor, RLS/RBAC, statistics |
| `slate-slatedb` | SlateDB backend, S3-compatible storage, read replicas |
| `slate-derive` | `#[derive(Record)]` and generated column constants |
| `slate-orm` | Typed surface; re-exports the rest |
| `slate-server` | gRPC head node, writer leadership over an object-store lease |
| `slate-serverd` | The head node as a binary: one TOML file, no Rust to start it |
| `slate-headbench` | Benchmarks for the head node, against a real one over a socket |
| `slate-testserver` | The Python client's fixture daemon. Lives at `clients/python/testserver`, beside the tests that need it, and is a workspace member because three separate things rotted in it while it was not |

Not Rust, and not in the workspace:

| directory | what it is |
|---|---|
| [`clients/python`](clients/python) | The typed Python client, and [`PROTOCOL-FINDINGS.md`](clients/python/PROTOCOL-FINDINGS.md) — fifteen things the first outside consumer of the wire found wrong or awkward about it |
| [`clients/go`](clients/go) | The Go client |
| [`clients/typescript`](clients/typescript) | The TypeScript client |
| [`examples/explorer`](examples/explorer) | An interactive demo: one database, three SDKs, a switch between them, and a [conformance runner](examples/explorer/conformance) that requires all three to answer identically |
| [`site`](site) | Two static pages. No build step |

The conformance runner is worth singling out. Every client's own suite runs
against the same head node, which catches *a* client being wrong and cannot
catch two clients quietly disagreeing about something the server accepts from
both. It is the only thing here that compares the clients to each other, and it
found three real divergences on its first run.

No `unsafe` anywhere (`#![forbid(unsafe_code)]` in every crate).

## The decisions that matter

### Key encoding is the linchpin

Everything is one ordered keyspace, so a key's encoding *is* its index:

```text
row entry     0x01 <table id : u32 BE> <primary key tuple>
index entry   0x02 <index id : u32 BE> <tenant?> <indexed tuple> <primary key tuple>
```

`slate-tuple` guarantees byte order equals value order. Two properties do the
work, and both were found by property testing before either had callers:

- **Every element's encoding is prefix-free.** With a one-byte string
  terminator, `enc("\xff")` is a proper prefix of `enc("\xff\0")`. Ascending
  order survives that, but a descending column then fails to reverse them, and
  in a mixed-direction key the *following* element's bytes can decide the
  comparison outright. The terminator is two bytes so no element can prefix
  another, which makes "complement the bytes" an exact order reversal.
- **Integer encodings are canonical.** A non-minimal length is rejected on
  decode; accepting it would give one value two encodings and silently break
  the uniqueness of a unique index.

### Unique indexes collide by construction

A unique index with no null in its indexed values leaves the primary key out of
the key and puts it in the value. Two writers racing for the same slot then
write the *same key*, so the store's own write-write conflict detection picks a
winner — no read-check, and no dependence on the isolation level. A null puts
the primary key back into the key, which gives the SQL behaviour that nulls do
not collide with each other.

The read the record store does before a unique write exists only to produce a
useful error instead of a bare retry.

### Index maintenance is atomic, not eventual

Index entries are written in the same transaction as the row. There is no
background index builder to fall behind and no repair path to get wrong. Tests
assert that the number of index entries stays exactly `rows × indexes` across
mixed sequences of inserts, updates, upserts and deletes.

### Security is in the kernel, not in the ORM

This is the part most KV-layer projects get wrong, and it is structural rather
than a matter of care. Policies are not checked by a wrapper a caller could step
around: they are compiled into the predicate the executor evaluates and into the
bounds it scans. Every read and write on `RecordTransaction` takes a
`SecurityContext`, so there is no unauthenticated door — bypassing is possible,
but only by naming `SecurityContext::superuser()`, which is one grep away.

Before any row is touched: an RBAC check against the catalog, a mandatory tenant
restriction on tenant-scoped tables, then the OR of the matching RLS policies.
Writes are checked in both directions as in Postgres — an update may only touch
a row the caller can already see and may only leave behind a row they could have
written, so a policy cannot be escaped by editing your way out of it.

Everything fails closed:

- An empty security catalog denies every non-superuser action.
- RLS enabled with no matching policy yields **no** rows, not all rows.
- A tenant-scoped table refuses a context with no tenant rather than defaulting
  to "show everything".
- A hidden row reports as *missing*, never as *forbidden* — otherwise the error
  code is an existence oracle, and an upsert against a hidden row would be a
  silent overwrite.

**Tenant scoping is physical.** The schema layer requires a tenant column to be
the first primary key column, so row keys partition by tenant for free; index
keys get the tenant prepended. A tenant restriction is a narrower scan, not a
filter applied after reading a neighbour's bytes. There is a pleasant
consequence: because the tenant leads every key, the *policy's own* tenant
predicate is what makes the secondary indexes usable at all. Security narrows
the scan rather than costing a filter pass.

**Three-valued logic is a security property, not a nicety.** Under two-valued
logic `NOT (owner = :caller)` is true for a row whose owner is null, so a
deny-style policy would hand out exactly the rows nobody owns. Evaluation
follows SQL: the comparison is unknown, the negation stays unknown, the row is
withheld.

### The planner costs plans, and can be asked why

Queries are planned against statistics, in units of one object-storage round
trip: opening a scan 1.0, a row from an open scan 0.01, a point read 1.0. The
ratio has a blunt consequence — **a non-covering index scan only beats a table
scan when it selects under roughly 1% of the rows the scan would touch.** That
is not a quirk of the constants; it is what storage where every lookup is a
network round trip implies, and it is why covering an index matters far more
here than on local disk.

Without a cost model the planner preferred whichever index matched the most
equality terms, which on the benchmark corpus chose a plan 30× slower than
ignoring the index. See [`docs/performance.md`](docs/performance.md).

```rust
let plan = txn.explain(&ctx, &table, &query)?;
println!("{plan}");
// Index Only Scan using by_kind on events  (rows=100 cost=2.00)
```

`analyze` gathers the statistics by reading the table. It is an ordinary read,
so it sees what the caller can see — statistics gathered under a restrictive
policy describe that slice rather than the table.

Scan bounds are still treated as an optimisation only. Every conjunct stays in
the plan's residual and is re-checked per row, even when the bounds already
imply it. That costs a little work and buys two things: a bound-derivation bug
can make a scan slow but not wrong, and a mandatory security predicate is
enforced by evaluation rather than by the planner having correctly turned it
into a range.

### Reading a set of keys

`WHERE id IN (…)` over a primary key becomes the reads it actually is, issued
together, rather than a scan looking for them:

| keys | before | after | model says | measured per unit cost |
|---:|---:|---:|---:|---:|
| 10 | 58.8 ms | **2.3 ms** | 1.0 | 2.30 ms |
| 50 | 59.3 ms | **8.1 ms** | 4.0 | 2.03 ms |
| 200 | 59.7 ms | **30.1 ms** | 13.0 | 2.31 ms |

Loading a set of records by id is the commonest thing an ORM does after
loading one, and it was scanning the whole table to do it.

The set is a *candidate*, not a replacement: four hundred keys cost twenty-five
waves where the scan they replace costs three, so the cost model has to be able
to choose the scan. Offering only the reads hid the better plan and let an
unrelated index win by default — which is what the test caught.

### Overlapped reads are cheaper, and the model has to know

An index scan does not wait for each row lookup in turn — it keeps sixteen in
flight — so `n` lookups cost `ceil(n / 16)` round trips, not `n`. The model
charged them one apiece, which made it prefer a 57 ms table scan over a 20 ms
index scan. Fixing that moved the crossover from about 1% of the table to about
6%, and a plan's estimated cost is now within the timer's resolution of its
measured wall time on every shape in the benchmark.

A window narrows the pipeline: fetching sixteen rows to return ten spends six
round trips on rows nobody sees, so the cursor caps its prefetch at
`limit + offset` and the cost model uses the same depth.

### Index-only scans

A query says which columns it needs. When an index holds all of them — its own
columns plus the primary key it carries — the row is assembled from the index
entry and never read:

```rust
// No rows read at all: `by_kind` holds everything this touches.
let query = Query::all()
    .filter(Expr::eq(kind, Value::Str("signup".into())))
    .select([id, kind]);

// Counting reads no columns, so any usable index answers it.
let total = txn.count(&ctx, &table, &Query::all()).await?;
```

The check is over the projected columns *union the predicate's*, and the
security filter is part of that predicate — so a policy on a column the index
lacks forces the row read rather than being skipped by the optimisation.

### Queries

```rust
let query = Query::all()
    .filter(Expr::eq(region, Value::Str("eu".into())))
    .and(Expr::compare(amount, CmpOp::Ge, Value::I64(100)))
    .sort_by([SortKey::desc(amount)])
    .limit(20)
    .offset(40);

let rows = txn.execute(&ctx, &table, &query).await?.collect().await?;

let groups = txn
    .group_by(&ctx, &table, &Query::all(), &[region],
              &[Aggregate::Count, Aggregate::Sum(amount)])
    .await?;
```

`ORDER BY` is planned, not just executed: each access path knows the order it
produces, and a requested order is satisfied when it is a prefix of that. When
nothing produces it the result is materialised and sorted, and the cost model
knows a sort must see every matching row before returning the first.

### Patterns and vectors

```rust
// LIKE, ILIKE, and a regular expression. An anchored pattern becomes scan
// bounds; the rest are filters.
Expr::like(url, "https://example.com/%");
Expr::ilike(title, "%Rust%");
Expr::matches(referer, r"^https?://(?:www\.)?([^/]+)/");
```

Nearest-neighbour search is not a separate engine. A distance is an ordinary
computed value, so k-NN is `ORDER BY` a distance with a `LIMIT`, and the
bounded heap that serves every other `ORDER BY ... LIMIT` serves this one:

```rust
let distance = Query::computed(&table, 0);
let query = Query::all()
    .filter(Expr::eq(tenant, Value::Str("acme".into())))
    .computing([Scalar::column(embedding).distance(target, Metric::Cosine)])
    .sort_by([SortKey::asc(distance)])
    .limit(10);
```

Because it is an ordinary query, a filter, row-level security and a projection
all apply to it without the search path learning what they are — a tenant
cannot be shown its neighbour's nearest neighbours.

This is exact search, not an approximate index: it reads every row the filter
admits. That is the right trade at the scale a filter leaves behind, and the
wrong one for unfiltered search over a very large table. A vector is refused in
a primary key or an index, at schema-definition time, because its order is
deterministic but says nothing about similarity — an index on one would sort
correctly and answer nothing.

### Joins

```rust
let join = Join::equating(Author::COLUMNS.id, Book::COLUMNS.author_id)
    .left(Query::all().filter(Expr::eq(country, Value::Str("UK".into()))));

let pairs: Vec<(Author, Option<Book>)> = txn.join_records(&ctx, &join).await?;
```

A join is **two secured reads**, not one privileged one. Each side is planned
through the same path a single-table query uses, so each is authorised and each
carries its own row filter. A join cannot see a row either side's policy hides,
because it never asks storage for rows — it asks two cursors that already
applied their policies. That is structural, not careful.

All four join types are here. A right or full outer join has to return inner
rows that matched *nothing*, which is not a fact any single probe can establish
— it is the absence of a match over the whole other side. A hash join already
holds one side in memory, so it flags the buckets that were probed and drains
the rest at the end; a nested loop cannot, so the planner will not choose one
there and refuses one that is forced.

A condition only a formed pair can answer goes in `having`, over the ordinal
space `JoinSchema` defines — the left table keeps its ordinals, the right
table's shift past its width:

```rust
let at = JoinSchema::of(&authors, &books);
let join = Join::equating(author_id, book_author_id).having(
    Expr::compare_columns(at.right(published), CmpOp::Lt, at.left(died)),
);
```

It is an ordinary `Expr`, so it goes through the same evaluator and the same
three-valued logic the security filter uses — not a parallel expression type
that would be a second place for those null semantics to drift. It behaves like
SQL's `ON`: a left row whose every candidate is rejected comes back unmatched,
not missing.

Comparing two columns of *different* types is refused rather than answered.
`Value`'s order is type-first — which is what makes the key encoding sortable —
so an integer against a float would order by type and give the same answer for
every row.

The planner picks between a hash join and a nested loop by cost, and both
directions are worth measuring, because the cost model makes a strong claim in
each:

| | rows read | wall | plan |
|---|---:|---:|---|
| 500 actors ⋈ 2500 events | 3,000 | 76 ms | Hash |
| the same, forced to a loop | 3,000 + 2,500 point reads | 158 ms | Nested Loop |
| one actor's events | 5 | 6.6 ms | Nested Loop |
| the same, forced to a hash | 2,500 | 62 ms | Hash |

A scanned row costs a hundredth of a round trip and a probe costs at least one,
so a hash join wins the general case by 2.1×. But "one row against ten thousand"
is what an ORM does all day — load a user, then their orders — and there the loop
wins by 9.4×. The planner gets both right; `explain_join` says which it chose.

### Chains of more than two tables

```rust
let at = JoinSchema::over([&teams, &actors, &events]);
let chain = Chain::from(one_team)
    .join(JoinStep::equating(at.at(0, team_name), actor_team))
    .join(JoinStep::equating(at.at(1, actor_name), event_actor));

let rows: Vec<(Option<Team>, Option<Actor>, Option<Event>)> =
    txn.chain_records(&ctx, &chain).await?;
```

Left-deep and explicit: the tables join in the order written, and there is no
join-order search. That is the part of query optimisation that genuinely needs
one — the space is factorial and the estimates feeding it compound — and doing
it badly would be worse than not doing it, because a wrong order here costs
round trips rather than a constant factor.

A step can join back to any earlier table, not only the one before it, which is
what the positional ordinal space is for. Each step chooses its own algorithm
by the same rule as a two-table join, and each is a secured read, so a policy on
the third table applies as surely as one on the first.

Intermediates are materialised — each step is the next step's build side — so
the accumulated set is bounded at every step rather than left to the allocator.
`step_counts()` reports what each step actually produced, which is the first
thing to look at when a chain is slow and the estimate said it would not be:

```
every team -> actors -> events   2500 rows   3 scans, 3010 rows read   80 ms   [10, 500, 2500]
one team   -> actors -> events    250 rows   2 scans, 3000 rows read   75 ms   [1, 50, 250]
```

### Relationships are loaded, not lazily fetched

The feature an ORM is judged on is `author.books()`. The feature an ORM is
*blamed* for is that `author.books()` ran a query, once per author, inside a
loop nobody noticed writing. Those are the same feature: a lazy accessor cannot
know whether it is being called once or ten thousand times, so it issues one
query either way and the cost lands somewhere the code does not mention.

So there is no lazy accessor. A relationship is *declared* on the type and
*loaded* by a call that takes the whole set of parents at once:

```rust
#[derive(Record)]
#[record(table = "authors", id = 1)]
#[record(has_many(Book, foreign = author_id))]
struct Author { #[record(pk)] id: u64, name: String }

#[derive(Record)]
#[record(table = "books", id = 2)]
#[record(belongs_to(Author, local = author_id))]
struct Book { #[record(pk)] id: u64, title: String, author_id: u64 }

let authors: Vec<Author> = txn.find_records(&ctx, Expr::True, ScanOrder::Ascending).await?;
let books = load_related::<_, Author, Book>(&txn, &ctx, &authors).await?;
// books[i] belongs to authors[i]; an author with none gets an empty vector.
```

One read for any number of authors, because it lowers to `Expr::In` over the
child table — which the planner already turns into point gets or index ranges.
Measured with a store that counts scans: **1 read batched, 3 for the per-parent
loop it replaces**. Not a join, deliberately: a join returns the parent's
columns once per child, so an author with forty books arrives forty times and
is decoded forty times.

Columns are named by **field ident**, not by string, so both sides are checked
by the compiler — the local one in the macro with a span on the attribute, the
foreign one by emitting `Other::COLUMNS.field`. A string would make a typo a
panic on first use, or a relationship over the wrong column that returns
plausible rows for ever.

### Pages are keys, not offsets

`OFFSET n` reads and discards `n` rows — the executor says so in a comment — so
page five hundred costs five hundred pages of reading. Worse, it *counts* rows,
so it is only correct while nothing changes: delete one row ahead of the cursor
between two pages and the reader silently skips one.

```rust
let page: Page<Book> = txn.page_records(&ctx, &Query::all().limit(20)).await?;
let next: Page<Book> = txn
    .page_records(&ctx, &Query::all().limit(20).after(page.next.unwrap()))
    .await?;
```

The cursor is the primary key of the last row, and it narrows the scan range.
Measured over 500 rows in pages of five: **page 99 read 495 key-value pairs by
offset and 5 by cursor**. There is a test that deletes a row between two pages
and asserts the offset reader never seeing row 4, beside the cursor reader
getting it right — because showing only the right answer would not establish
there was a problem.

A cursor pins the access path to the table's own key range rather than letting
the cost model choose: an index yields rows in an order a primary key cannot
describe, so a query that paged correctly on a small table would start returning
wrong pages once it grew. A sorted query, a grouped read and an explicit index
hint are each refused rather than paged wrongly.

The three clients have it too — `page` in Python, Go and TypeScript, over
`Query.after`, `Query.paged` and `QueryResponse.next_cursor`. The **server**
builds the cursor, because building one means knowing which columns are the
primary key and in what order, and one built from the wrong column still pages,
just through the wrong sequence. What that costs is one more refusal: a paged
read whose projection drops a key column has no key to build a cursor from, and
is refused by name rather than served without one — which would be the silent
version, where the caller loops until the cursor is absent, gets none on the
first page, and reads one page of a large table as the whole answer.

### A predicate write says which rows, and can hand them back

`DELETE FROM t WHERE …` in one statement, rather than a query, a round trip and
a write per key — which is N+1 by construction, and not atomic with the query
that found the keys: a row inserted in between is missed, and a row deleted in
between is deleted twice.

```rust
let gone = txn.delete_where(&ctx, table, Expr::lt(STARTED, Value::I64(cutoff))).await?;
let bumped = txn
    .update_where(&ctx, table, Expr::eq(KIND, "page".into()),
                  &[(VIEWS, Scalar::column(VIEWS) + Scalar::literal(Value::I64(1)))])
    .await?;
```

Both return the rows rather than a count — `.len()` is the count. They were
already in memory, because every row touched had to be read to be written, and
a caller that named a *condition* has no other way to learn which rows it hit.
For a delete that is the only moment the answer exists: afterwards the rows are
gone and no read recovers them.

Each assignment is a `Scalar` over the row **as it was read**, so `views =
views + 1` is one write rather than a read, a decision and a write, and two
concurrent increments make two. Assignments apply together rather than
left-to-right, so `a = b, b = a` swaps.

Over the wire this is `DeleteWhere` and `UpdateWhere`, with a `returning` flag,
in all three clients. `RETURNING` is offered on these two and on nothing else,
and the reason is worth stating because its absence elsewhere looks like an
oversight: an `Insert` here is given a whole row and the server applies no
`DEFAULT` to it — the wire's row is full width and its nulls are values a
caller meant — and there is no auto-increment, no trigger and no generated
column. The row written is the row sent, so returning it would hand the caller
its own request back.

### A batch is a round trip, a transaction is a guarantee

Several writes in one request, and the caller has to say which of the two they
mean:

```rust
// Independent: each lands on its own, each reports on its own.
BatchRequest { operations, atomicity: ATOMICITY_INDEPENDENT }
// Atomic: all of them or none, and the first failure fails the request.
BatchRequest { operations, atomicity: ATOMICITY_ALL_OR_NOTHING }
```

`ATOMICITY_UNSPECIFIED` is refused, by name, with a message that spells out
both. That is the whole design: the two differ only when something fails, so a
caller who never chose finds out on the day a write in the middle is rejected —
and discovers then whether the ones before it stayed. A zeroed request has to
mean neither, which is why this is an enum with a refused zero and not a `bool
atomic`.

An independent batch reports one result per operation, and a failure is one of
those results rather than the end of the batch. An atomic one reports none,
because there is nothing to say per operation: they all happened, or the
request failed and none did.

Fifty single-row inserts over loopback against the in-memory store took 46–49
ms; the same fifty as one batch took 4.9–5.1 ms, over five runs — **9.4× to
10.0×**. That is a saving on round trips and request framing, not on the writes:
an independent batch still commits each operation separately, and on a real
network the gap is wider because the round trip is the part that grows.

### Conditional writes, because the store cannot see a lost update

The store detects two writers overlapping in time. The common failure is the
other one: read a row in one transaction, decide something, write it in
another. Nothing overlaps, so nothing is detected, and the second write discards
the first's edit with a value computed before it existed.

```rust
let post: Post = txn.get_record(&ctx, &[Value::U64(1)]).await?.unwrap();
let mut edited = post.clone();
edited.title = "new".into();
txn.replace_record(&ctx, &post, &edited).await?;   // refuses if it moved
```

The whole row is compared rather than a version column, because a version only
detects changes made by writers who *remembered to bump it* — a convention every
call site has to keep, where the one who forgets is the one whose edit is lost.
The check costs no extra round trip: `update` already reads the row to enforce
the row policy.

### There is no date type, and an enum is its own name

The value model is closed — nine types, each chosen because the tuple codec can
order it — so "add a type" is not the answer to a Rust type that needs storing.
The answer is a `Field` impl that maps it onto one that is already there, and
three of those ship:

```rust
#[derive(Enum)]
enum Payment { Cash, #[record(rename = "credit card")] CreditCard }

#[derive(Record)]
#[record(table = "rides", id = 1)]
struct Ride {
    #[record(pk)] id: u64,
    started: Timestamp,          // an i64 of seconds. Nothing on disk changes
    payment: Payment,            // the variant name, in a Str column
    meta: Json<Settings>,        // serde_json, behind the `json` feature
}
```

**`Timestamp` is an `i64` of seconds since the epoch**, and that is the whole
of it: chronological order *is* integer order, already proven by the tuple
property suite, so a new value type would have bought a second ordering to get
right. What the newtype buys is in Rust — an instant is not an id, and the
compiler now says so — and one place to state the unit, because every calendar
function (`Scalar::calendar_part`, `date_trunc`, the timezone lookups) reads
**seconds** and a column holding milliseconds would be answered with a year
around 55000 and no error. There is no timezone in it: which day an instant
falls on is a question about a zone, and `Scalar::in_zone` is where it is
asked.

**An enum stores its variant name, not an ordinal.** Both work; they fail
differently. With an ordinal, *reordering* the variants silently reinterprets
every stored row — and reordering is something people do by accident,
alphabetising a list or inserting in the middle. With a name, *renaming* does
the same damage, but a rename is deliberate and the compiler visits every use
site while you do it. So the name is the choice that fails on the rarer, louder
action, and `rename` exists so the Rust spelling and the stored one can move
apart. The costs are stated rather than hidden: names are longer in every row
and index entry, and an index sorts them alphabetically — `"critical" <
"info" < "warning"`, which is not the order anyone means for a severity, so
that column wants an integer and a hand-written `Field`.

**`Json<T>` is a string that happens to hold JSON**, and every query against it
is a query against that string. There is no path expression and there will not
be one: a document has no useful total order, so it could not be a key, an
index or a range predicate. If a field inside needs querying, it is a column.
Two things are worth knowing and both have tests rather than warnings — it
serializes at construction, not at write time, so the failure lands where a
caller can still handle it; and a `HashMap` has no stable encoding, so two maps
a caller considers equal store as two different strings. `BTreeMap` does not
have that problem, and the test asserts both halves.

### Money is an integer count, and the column says of what

```rust
#[derive(Record)]
#[record(table = "invoices", id = 1)]
struct Invoice {
    #[record(pk)] id: u64,
    #[record(scale = 2)] total: Units,   // Units(1999) is 19.99
}
```

`Value::Decimal` holds units; the **scale lives on the column**. That one
decision is what makes the type cheap and exact at once: it encodes as an
integer, so the ordering is the integer ordering — already proven and fuzzed —
equal values have exactly one encoding as index keys require, and `SUM` is
integer addition with no rescaling.

Measured through the aggregate path, a hundred rows of ten cents:

```
SUM over a decimal column:  Decimal(1000)      exactly $10.00
SUM over the same as f64:   9.99999999999998
```

The cost is that a value cannot print itself — rendering needs the column, and
`Units::to_string_with_scale` takes the scale rather than guessing.

### The schema on disk, and the index that returns nothing

Adding an index to a table that already holds rows does not make queries
slower. It makes them **return nothing**: the planner costs the index as cheap,
scans a key range no write ever wrote into, and answers with no error while the
rows are still there. It is also intermittent — whether it happens depends on
whether the index covers the query — which is worse than if it always did.

A third keyspace (`0x03 <table id>`) records per table what the last migration
left behind: the schema version, a fingerprint of the column layout, and which
indexes are actually built.

```sh
# slate-serverd reconciles before it binds its listener.
[schema]
migrate_on_start = true    # the default
```

Before the socket, not after: a node that binds and then migrates accepts a
connection and answers it wrongly. Turning it off is not permission to serve
without one — the node then *verifies* and refuses to start.

The fingerprint covers what decides how bytes are read — column count, types,
nullability, drops, the primary key, the tenant column, a decimal's scale — and
deliberately **not** names, because a rename moves no bytes and must not look
like a migration.

### Schemas are code, and so are policies

No dynamic DDL. A table is defined once through a builder that validates
everything the kernel later assumes, so nothing downstream re-checks it. Row
bodies carry the schema version they were written at and each column records the
version it was added in, which makes evolution precise rather than positional:
columns added later read back as null (hence must be nullable), and a row from a
schema newer than the running build is an error rather than a silent truncation.

Policies are Rust functions of the security context rather than templates with
parameter substitution. That removes a bug class and matches the rest of the
design.

## Topology

SlateDB gives a single fenced writer with multi-reader scale-out, so this is a
single-writer database head with read replicas — a Neon-shaped system, not a
Spanner-shaped one. That is the honest limitation, and it is deliberate.
[`docs/topology.md`](docs/topology.md) covers it properly, including the
improvements considered and left out.

The short version:

**Reads scale, writes do not.** A `RecordSnapshot` is the read-only counterpart
of a `RecordTransaction`, and a replica-backed store simply lacks the write
methods rather than having them and failing at runtime. Both go through the same
secured read path, so a replica cannot be a way around a policy.

**Consistency is a token.** A commit returns the sequence it landed at, and a
read carrying that sequence may only be served by a view that has reached it.
Threading the highest token seen gives monotonic reads for free.

```rust
let token = txn.commit().await?;
watermark.observe_commit(token);
let snapshot = pool.snapshot(watermark.freshness(), tenant.as_ref()).await?;
```

The token is a *durable* sequence — a replica reads object storage, so
read-your-writes through one requires durable commits.

**Routing is by tenant, not round-robin.** Reads come from object storage, so
cache hit ratio dominates latency far more than balance does; because the
keyspace is tenant-prefixed, one tenant's working set is a contiguous range that
stays warm on one replica. The prefix that isolates a tenant for security is the
one that makes it cacheable. Placement is rendezvous hashing, so losing a replica
moves only its own tenants.

**Conflicts are ordinary, fencing is terminal.** `RecordStore::transact` retries
only what can succeed on a second attempt, with jittered backoff. A second writer
fences the first, and a writer that retried past that would be a split brain, so
`WriterFenced` is a distinct non-retryable error.

Commit latency is bounded by the flush to object storage. `Durability::Visible`
returns as soon as a write is committed and visible to readers but before it is
durable; it is available and documented as losing acknowledged writes if the
writer dies before its next flush, so it is only for callers that can replay.
The default waits.

Isolation defaults to serializable snapshot. The record layer's own guarantees
hold under plain snapshot isolation, because they rest on write-write conflicts
rather than on read tracking; it is application invariants that read one row and
write another which need serializable. The safer level is the default and
stepping down is explicit.

## Storage

Anything speaking S3 works: AWS, MinIO, Tigris, Cloudflare R2. The differences
between them are an endpoint, an addressing style and how credentials resolve,
and they all live in `S3Config`.

```rust
let store = SlateStore::open_s3(
    "/records",
    S3Config::new("records")
        .with_endpoint("http://127.0.0.1:9000")   // MinIO; Tigris and R2 are HTTPS
        .with_credentials("minioadmin", "minioadmin")
        .allow_http(true),
)
.await?;
```

## Benchmarks

- `crates/slate-kernel/examples/perf_report.rs` — query shapes against a
  latency model, in I/O counts as well as milliseconds.
- `crates/slate-slatedb/examples/scan_tuning.rs` — scans against a real S3
  server, counting object-store requests.
- `crates/slate-clickbench` — the 43 official ClickBench queries, all 43 of
  which run, with every independently checkable answer verified against the
  same data read through `pyarrow`. Not comparable
  to published ClickBench scores; run because it is an adversarial workload
  nobody here designed for. See
  [`docs/clickbench.md`](docs/clickbench.md), which also covers the pgrust
  review.

## Testing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo bench                                                # criterion
cargo run --release -p slate-kernel --example perf_report  # I/O counts
```

Tests are written around guarantees rather than API surface:

- The tuple codec's properties are checked at 400k cases: byte order matches
  value order under arbitrary per-column directions, encodings are prefix-closed
  and prefix-free, value equality and encoding equality are the same relation.
- `bounds_never_lose_rows` checks that a planned query returns exactly what
  brute-force filtering returns, across a spread of predicate shapes and both
  scan orders. It was verified to have teeth by breaking `<=` into `<` and
  confirming it caught the lost boundary row.
- A **planner oracle** generates random queries and requires every access path
  to agree: the planner's choice, a forced table scan, and each index in turn
  must return identical rows. An optimiser that changes the answer is not
  optimising. It found three bugs on its first run — see
  [`docs/correctness.md`](docs/correctness.md).
- **Write failures are injected at every position** a transaction writes, and
  after each the store must still satisfy a full index/row consistency check.
  A row and its index entries land together or not at all.
- **Contention is tested by counting.** Eight tasks incrementing one row must
  total exactly the number of increments — a lost update is a number too small,
  a double-apply one too large — and the test also asserts that transactions
  really did conflict, since tasks that never overlapped would pass while
  proving nothing.
- **Untrusted input** — arbitrary bytes into the decoder, and caller-supplied
  `LIKE` patterns and regexes — must return an error rather than panic, hang or
  overflow the stack.
- The oracle extends to **joins, chains and aggregates**: four join types
  against three algorithms, a three-table chain against a hand-written triple
  loop, and grouping against a fold written out over the rows. Each also runs
  over a **tenant-scoped table with a composite key and mixed-direction
  indexes**, since the single-column case exercises none of the keyspace code
  that matters.
- **Writer handover** is covered on both sides of the takeover, including two
  writers genuinely overlapping across a lease change, and replicas are read
  from while the writer commits — a replica may lag, but the state it serves
  must be one the database was actually in.
- **Faults are injected below the storage engine too**, at the object store, so
  a crash lands inside SlateDB rather than above it. What survives must be
  coherent even when committed data is lost.
- The **cost model is calibrated against a real S3 server**, not a latency
  fixture. Doing that found it overcharging scans eighty times and
  undercharging index lookups forty — see
  [`docs/correctness.md`](docs/correctness.md).
- **Plans are snapshotted.** Eighteen query shapes across three table sizes,
  committed as text, so a cost-model change is one reviewable diff rather than
  a series of surprises: reverting one constant reports eleven changed plans at
  once. Borrowed from how Postgres' regression suite works; see
  [`docs/clickbench.md`](docs/clickbench.md).
- Row-level security is checked as a **matrix**, once per access path, rather
  than as a set of scenarios: a policy honoured by the table scan and skipped
  by the k-NN search is a leak, and the paths that skip it are the ones added
  last. Every single-table path is one row of that table.
- A store is closed and reopened to prove rows, index entries, tombstones and
  tenant isolation all survive a restart — including that a covering scan and
  a table scan still agree afterwards, which is how an index that came back in
  a different state from its table would show up. These run over the S3
  protocol as well as in memory, since reopening is exactly where a backend's
  manifest handling could differ.
- The security suite is written around what a caller *cannot* do: probe for
  hidden rows via error codes, reach another tenant by asking explicitly, escape
  a policy by updating out of it, or leak null-valued rows through a negated
  policy.
- Routing is tested on the properties that justify it: a tenant always lands on
  the same replica, tenants spread within 20% of even across four replicas, and
  losing a replica moves *zero* tenants that were not on it.
- Storage is tested over the S3 protocol as well as an in-memory object store —
  the same assertions, both substrates — using an in-process S3 server so the
  suite stays hermetic and needs no Docker daemon. A CI job additionally runs it
  against MinIO.
- What only a real instance can show is tested against one: two concurrent
  writers unable to both take a unique index slot, a replica genuinely observing
  the writer's data, security applying identically on a replica, and a second
  writer fencing the first.
- Performance claims are tested in point reads, not milliseconds — "this reads
  no rows" survives a different machine in a way "this took 2 ms" does not.

Set `SLATE_S3_BUCKET` and friends to point the storage suite at your own MinIO,
Tigris or R2 bucket instead of the in-process server.

## Status

Built and tested:

- [x] Order-preserving tuple codec
- [x] Schema, catalog, row codec with versioned evolution
- [x] Keyspace, storage abstraction, in-memory backend
- [x] Record store: primary key CRUD with atomic index maintenance
- [x] Scan/filter executor and heuristic index selection
- [x] RLS predicate injection and RBAC catalog
- [x] `#[derive(Record)]` and the typed surface
- [x] SlateDB backend, and S3-compatible storage (AWS, MinIO, Tigris, R2)
- [x] Read replicas, read tokens, tenant-affinity routing
- [x] Conflict retry and writer fencing
- [x] Cost-based planning with statistics, `analyze`, and `EXPLAIN`
- [x] Projections and index-only scans; pipelined index lookups
- [x] Aggregates including `COUNT(DISTINCT)`, `GROUP BY`, `ORDER BY`,
      `LIMIT`/`OFFSET`
- [x] `LIKE` and `ILIKE`, with an anchored pattern becoming scan bounds
      rather than a filter
- [x] Regular expressions (`~`, `~*`, `REGEXP_REPLACE`) on a linear-time
      engine, because a pattern is caller input
- [x] Vectors and similarity search: `L2`, cosine and inner-product
      distance as an ordinary computed value, so k-NN is `ORDER BY`
      distance with a `LIMIT` and needs no separate search path
- [x] Scalar expressions — arithmetic, `length`, `CASE WHEN`, `COALESCE`,
      `DATE_TRUNC`, `extract` — computed per row and addressed by ordinal, so
      grouping, sorting and aggregation take them without changing
- [x] `HAVING`
- [x] Inner, left, right and full outer joins, hash or nested-loop by cost,
      both sides secured; conditions spanning both sides
- [x] Chains of three or more tables, in the order written, with per-step plans
- [x] `IN` over a primary key as a set of overlapped reads, not a scan
- [x] Equi-depth histograms, so a range estimate knows which range it is
- [x] Scan readahead on the SlateDB backend: 31x fewer object-store requests
- [x] Bulk writes: `insert_many`/`upsert_many`/`update_many` overlap the
      duplicate-key and unique-index reads (100 rows in 13 ms, down from 223 ms)
- [x] Schema constraints and migrations: `DEFAULT`, `CHECK`, foreign keys with
      referential actions, column drop and rename
- [x] `IN` over a *secondary* index as one range per value rather than one
      range spanning them all
- [x] Partial indexes — an entry only for the rows a predicate admits, declared
      on the schema, maintained by the record store, and read by the planner
      only for queries it can *prove* land inside the predicate
- [x] Expression indexes — a key computed from the row (`lower(email)`,
      `length(url)`) rather than read out of it, with `analyze` evaluating the
      expression so the estimate is measured rather than assumed, and a covering
      scan that answers from the entry it is already holding
- [x] A computed value's inputs are read and are *not* part of the answer.
      `SELECT id, lower(title)` returns `title` as null unless it is asked for;
      an entry keyed on `lower(title)` cannot produce `title`, so no path may,
      or a row's contents would depend on the plan that fetched it
- [x] Grouping over a `Chain`, the n-way case of a grouped join, kernel and
      wire alike — the server's old "grouping a chain is not built" refusal is
      gone rather than left to become a lie. Group keys may
      name any table in the chain, and each step reads only what something
      downstream takes out of its row — so `COUNT(*)` over a chain can be
      index-only, like a grouped join's
- [x] **Withdrawn:** "a grouped join is not costed as grouped". The per-joined-row
      term is added to the hash cost and the loop cost *equally*, so it cancels
      out of the comparison and discounting it for a grouped join would change
      no plan. Grouping does change the plan, through the projection narrowing
      that was already there.
      `the_per_row_term_is_symmetric_so_grouping_cannot_flip_the_algorithm`
      pins the symmetry, so the day it stops holding, the reasoning is caught
- [x] A grouped join and ordered groups on the wire. `AggregateQuery` carries
      a `join` and its own `sort`/`limit`/`offset` over *groups*, with a
      differential against the kernel per shape. Three inputs was refused with
      the reason at the time, because the kernel then grouped a two-table join
      and not a chain; the entry two above is where that stopped being true
- [x] A grouped join, and `ORDER BY`/`LIMIT` over groups — one hash-grouping
      implementation over two sources rather than a second one, and an
      index-only scan still serves a grouped join (`count(*)` per author reads
      zero book rows)
- [x] CI that runs, which it had never done. `.github/workflows/ci.yml`
      existed and was active and had **zero runs**: it triggered on `main` and
      pull requests, and every branch since it was added has been a feature
      branch with no pull request. Eleven jobs now, on every push — the Rust
      workspace, the three client suites, the demo frontend, the pre-commit
      hook's own tests, a workspace-layout guard, the docs page's
      quickstarts, and the conformance runner and a browser e2e over all three
      SDKs. Getting it green found **eleven** real defects, several of which
      exist only away from a developer's machine: two withdrawn or unreachable
      Docker images, a `--bin` filter that silently skipped a binary, a checker
      leaning on ambient installs, a readiness grep defeated by ANSI colour, a
      server bound to `::1` while everything asked `127.0.0.1`, a test harness
      that *skipped* — green — when `cargo` was missing, and committed protobuf
      stubs regenerating differently under an unpinned generator. Deploys: Pages
      publishes `site/` from `main`, and a tag builds `slate-serverd` for two
      targets and attaches them. The release *build* — both targets, the
      aarch64 cross toolchain, and `--check` on the shipped binary against the
      docs page's own TOML — runs on every push as well, so the one thing a
      release workflow is usually first asked to do on release day is already
      answered
- [x] A tagged release. `v0.0.1` carries `slate-serverd` for x86_64 and
      aarch64 Linux, built and attached by `release.yml`. Marked a prerelease,
      deliberately: the status banner above says what this is. The upload step
      was the last thing in this repository still in the state the CI work
      spent a morning getting out of — written, plausible, never executed — and
      it has now executed
- [x] `EXPLAIN` for a grouped read — `ExplainAggregate`, and
      `explain_grouped`/`explain_grouped_join`/`explain_grouped_chain` in the
      kernel. Not the same plan as explaining the read underneath: grouping
      narrows each input's projection to the group keys and the aggregates'
      columns, which is what lets an index answer a `COUNT(*)` without touching
      a row. Running and explaining go through one narrowing function, so an
      `EXPLAIN` cannot describe a plan nothing runs, and one kernel test ties
      the claim to *I/O* rather than to a second plan: explained index-only
      then reads zero rows, explained otherwise then reads some.
      `ExplainResponse` gained `decodes` in the process — without it, two plans
      that decode different amounts of every row printed identical strings
      wherever the access path was unchanged
- [x] Go and TypeScript clients ([`clients/go`](clients/go),
      [`clients/typescript`](clients/typescript)), alongside the Python one.
      Each runs its tests against a real `slate-serverd` started as a
      subprocess — no mocks, because a mock agrees with the client's own
      misunderstandings. All three cover joins, grouped joins, ordered groups,
      schema checks and explaining a grouped read; none covers computed values
      or vectors
- [x] A read-only head node. `Head::read_only` serves reads from replicas with
      no writer store at all, so a node that loses the campaign starts as a
      reader rather than fencing the healthy leader or refusing to run — which
      is what the docs had claimed all along and the library could not do,
      because building a `Head` required opening the writer and that open *is*
      the fence
- [x] `ObjectStoreLease` refuses a store it cannot renew on.
      `object_store`'s `LocalFileSystem` has no conditional update, so a local
      node used to take a lease it could never keep and let the term lapse
      under a healthy leader. Acquisition now probes the capability first and
      steps down terminally rather than polling forever; `backend = "local"`
      uses an advisory `flock`, which is host-local and not safe over NFS. The
      create-only lease that would remove that caveat is specified — and argued
      against — in `filelease.rs`
- [x] A head node binary. One TOML file declares tables, columns, indexes
      (unique, partial and expression), `CHECK`, foreign keys, grants and
      policies — predicates in a small parsed language rather than a
      `Deserialize` mirror of `Expr` that would go a variant short the day
      `Expr` grows one. Authentication has no default: no `[auth]` section is a
      refusal to start, and a mode whose safety rests on something outside the
      process must name that thing before it will bind off loopback
- [x] All fifteen protocol findings from the first outside client answered:
      eleven fixed, two argued and documented, two refused with reasons. A
      schema fingerprint on every request that names a table now catches a
      client whose column names or types have drifted from the catalog — the
      failure that previously filtered the wrong column and returned rows
- [x] Joins, aggregates, `GROUP BY`/`HAVING` and computed columns on the wire.
      A column reference names a *producer* and an index inside it — an input's
      column, the nth computed value, the nth group key, the nth aggregate —
      and the server does every piece of arithmetic. The client never adds a
      table width to anything, so a column added to an earlier table cannot
      silently re-point a later reference, and two refusals the flat model
      cannot express (`HAVING` on an ungrouped column, a computed value named
      from across a join) become kind mismatches rather than in-range ordinals
- [x] A gRPC head node with writer leadership: a compare-and-set lease on one
      object in the same bucket, terminal step-down on `WriterFenced`, reads
      routed by freshness and tenant affinity and stamped with which replica
      served them
- [x] The head node under concurrency: 32,627 point reads/s and 25,953
      streamed rows/s at 128 clients with zero errors, the box taking over at
      ~8 clients for a read and ~16 for a stream. Durable writers share a flush
      exactly — 633 commits/s each still seeing the same 101 ms — and
      `max_transactions` refuses more cheaply than it accepts. An abandoned
      stream really does stop: 0.016 core-seconds against 1.935 for the same
      scans drained
- [x] `TCP_NODELAY` on every server that binds its own listener. `tonic` sets
      it on connections it accepts itself and documents that the setting is
      *ignored* under `serve_with_incoming`, which is what binding your own
      port requires — so the shipped binary and both test servers were serving
      through Nagled sockets. A ten-row streamed query measured 44.00 ms with
      Nagle and 285 µs without, 154x, with a unary call unmoved as the control
      and the kernel's delayed-ACK counter at 1.12 per operation against 0.00
- [x] Benchmarks and a recorded baseline ([`docs/performance.md`](docs/performance.md)),
      the head node included: a gRPC round trip costs ~120 µs of which the head
      node's own work is 7–23 µs, so it is transport rather than conversion;
      a streaming query pays ~43 µs more even for one row; batching a hundred
      writes into one call is 15x; and a `Durable` commit is 101.10 ms flat,
      three orders of magnitude above everything else, because that is
      SlateDB's flush interval and not this repository's to change
- [x] A planner oracle, an access-path security matrix, restart/durability
      tests over both substrates, write-failure injection above *and* below the
      storage engine, contention tests, untrusted-input suites and committed
      plan snapshots ([`docs/correctness.md`](docs/correctness.md)), which
      between them found a covering scan and a point get ignoring the
      projection, a panic on contradictory bounds, a vector that encoded but
      would not decode, and a cost model wrong by three orders of magnitude
- [x] Relationships: `#[record(has_many(...))]` / `belongs_to(...)` emitting a
      `Related` impl, and `load_related` fetching every parent's children in
      **one** read rather than one per parent (measured: 1 scan against 3).
      No lazy accessor, deliberately — the N+1 an ORM is blamed for and the
      feature it is judged on are the same feature. Columns are named by field
      ident, so a typo is a compile error on both sides rather than a panic on
      first use or a relationship over the wrong column
- [x] **A migration runner, for a defect that was returning wrong answers.**
      Adding an index to a populated table made queries through it return *no
      rows* — no error, rows still on disk — and intermittently, depending on
      whether the index covered the query. Schema state now lives in a third
      keyspace, and `migrate::{plan, apply, verify}` back-fill an index with no
      entries (batched and resumable), reclaim a dropped index's entries, and
      refuse a layout change that would reinterpret stored rows. `slate-serverd`
      reconciles **before it binds its listener**, because a node that binds
      first answers a connection wrongly; `[schema] migrate_on_start = false`
      makes it verify and refuse instead, which is not the same as skipping
- [x] Keyset pagination: `Query::after(key)` and `Records::page_records`
      returning a `Page<R>` with the cursor for the next one, so a caller never
      names a key column. Page 99 of 100 read **5 key-value pairs against the
      offset version's 495**, and a row deleted between two pages no longer
      makes the reader skip one — both asserted, the wrong answer beside the
      right one. A sorted query, a grouped read and an explicit index hint are
      refused rather than paged wrongly
- [x] Per-row optimistic concurrency: `update_if_unchanged` and
      `replace_record` write only if the stored row still equals what the
      caller read. The store's conflict detection sees two writers overlapping
      in *time*; this is the read-modify-write across two transactions, where
      nothing overlaps and the second write silently discards the first's edit.
      The whole row is compared rather than a version column, since a version
      only catches writers who remembered to bump it
- [x] An exact decimal. `Value::Decimal` holds a count of the column's smallest
      unit and the **scale lives on the column**, so it encodes as an integer —
      the ordering is the integer ordering, equal values have one encoding, and
      `SUM` is exact. A hundred rows of ten cents sum to exactly `$10.00` where
      the same rows as `f64` give `9.99999999999998`; both are asserted.
      Decimals went into the existing tuple *property* suite rather than one of
      their own, which caught two defects on the first two runs — a missing
      comparison arm that made every decimal compare equal, and a `skip` path
      that did not know the new type code
- [x] A per-call deadline in every client. Python and TypeScript passed none on
      any RPC, so a head node that accepted a connection and then stopped
      answering blocked the caller **for ever** — a failure no error-code
      classification helps with, because no error arrives. Both grew a view
      (`with_timeout`, seconds; `withTimeout`, milliseconds) rather than a
      mutable setting, over the same connection and freshness scope. Go needed
      nothing: every method already takes a `context.Context`, which was a claim
      until `deadline_test.go` checked it. Each client has a test that
      demonstrates the *hang* against a listener that accepts and never speaks,
      beside the one that shows the deadline ending it
- [x] Type mapping without widening the value model: `Timestamp` (an `i64` of
      seconds, so nothing on disk changes), `#[derive(Enum)]` (the variant
      name in a `Str`, with `rename`, because a reorder is the accident and a
      rename is the deliberate act), and `Json<T>` behind a feature (a
      serialized document in a `Str`, serialized at construction so the write
      cannot fail). `Scalar`, `CalendarPart` and the rest are re-exported from
      `slate-orm`, which they were not — a timestamp is only useful if the
      calendar questions are reachable from the same crate
- [x] `SELECT DISTINCT`, lowered to a grouping over the selected columns
      rather than added as an operator — the kernel's `Grouping` with keys and
      no aggregates already yields the distinct combinations. What stood in the
      way was two defaults, not a missing feature: the browser binding turned
      an empty aggregate list into `count(*)`, and the head node refused one
      outright. The first also meant
      `SELECT author_id FROM books GROUP BY author_id` came back two columns
      wide with one the query does not mention; the second meant a client
      wanting distinct values had to ask for a count and discard it. **This
      one does cross the wire**, and there is a test for it in Python, Go and
      TypeScript. `SELECT DISTINCT *` is refused with its reason, since a
      primary key already makes rows distinct
- [x] `SELECT count(*) FROM books` — a grouping with no keys, which the kernel
      had always answered and only the SQL front end refused. Lifting it
      surfaced a second bug the reasoning had missed: the header code tested
      for a grouping differently from the dispatch, so the right values came
      back under the wrong column names
- [x] Uncorrelated subqueries: `WHERE author_id IN (SELECT id FROM authors
      WHERE country = 'US')`. Two reads, not an operator — the inner query
      runs once, its single column becomes the `values` of an ordinary
      `Expr::In`, and the planner's existing handling of that does the rest,
      so none of it reached the kernel. The Spec tab keeps both the subquery
      and the candidate list it produced, which is the thing worth seeing when
      the answer is not the expected one. Browser front end only: the wire
      carries the resolved `IN`, because by the time a spec leaves the tab the
      subquery is already a list
- [x] A candidate type that could never match the outer column is refused at
      parse time. `title IN (SELECT id FROM authors)` errors nowhere
      downstream — `1` renders to text and parses back as the string `"1"` —
      so it would have returned zero rows and looked like a fact about the
      data. Mixed integer widths are allowed: they round-trip, and refusing
      them would refuse a query that works
- [x] **All four features cross the wire.** `Value` carries a `decimal_value`
      — an `int64` count of the column's smallest unit, with the scale staying
      in the catalog — and `UpdateRequest` carries `expected`, the rows as the
      caller last saw them. All three clients have both: `Units` in Python and
      Go, `units()` in TypeScript, and the conditional update as
      `update(..., expected=...)` or `UpdateIfUnchanged`. `slate-serverd` can
      declare a decimal column too, which it could not: `type = "decimal"` was
      refused as "not a type", so the feature existed in the library and not in
      the binary anybody runs.

      Nothing checks a client's declared *scale* against the server's. The
      schema fingerprint deliberately does not hash it — a scale addresses no
      column — so a client declaring `scale=2` against a `scale=4` column
      reaches the right column and renders every value a hundred times too
      small, for ever, with no error at any layer. That is the price of a
      protocol that publishes no schema, and it is the sharpest edge in the
      feature.

      Relationships and pagination were on this list too. `Related` is an RPC
      now and `Query` carries `after`, `paged` and a `next_cursor` on the way
      back; all three clients have `related` and `page`, and the three-SDK
      conformance runner compares them on both. The prediction that the cursor
      "wants the same protocol change" as the other two was wrong in an
      instructive way: it wanted *three* fields rather than one, because the
      server has to be told a cursor is wanted before it can refuse a read it
      cannot build one for

Not built:

- [ ] A read-only node that starts while the leader has not yet migrated warns
      and serves. During that window a query through an unbuilt index returns
      no rows. Closing it means the follower waiting for the leader, which
      needs a way to tell "has not migrated yet" from "there is no leader"
- [ ] `AVG` over a decimal returns a float, as it does over an integer. The
      mean of exact decimals is generally not representable at the same scale,
      so something has to give, and consistency was chosen over refusing. It is
      the one place in that feature where exactness stops
- [ ] Arithmetic on decimals in `Scalar` — `price * quantity` is not
      expressible, because the product of two scale-2 values is scale-4 and
      nothing in the expression layer tracks that
- [ ] A decimal literal in the SQL front end: `WHERE total > 19.99` parses as a
      float and will not match a decimal column
- [ ] Milliseconds. `Timestamp` is seconds because every calendar function
      reads seconds; a millisecond column would need either a second type or a
      scale on the column, the way a decimal has one, and neither is built
- [ ] `chrono` or `time` interop. `Timestamp::from_unix_seconds` and
      `.seconds()` are the whole surface, so converting is the caller's line of
      code rather than a dependency in the record layer
- [ ] Anything inside a `Json<T>`: no path expression, no index on a field, no
      partial update. Deliberate, and the reason is in the type's own docs
- [x] A request id a caller could correlate with a server log line. This was
      blocked on the other half — `slate-serverd` logged its startup and its
      warnings and nothing per request, so an id sent from a client would have
      had nothing to be correlated against. Both halves are built now:
      `[observability] request_log` writes a line per call, and all three
      clients mint a `slate-request-id` per call, send it, and put it on every
      error they raise. A header rather than a proto field, because it belongs
      to the call and not to the query. The server filters it before logging
      it — an id is attacker-controlled text on its way into an audit trail,
      and gRPC blocks a newline in a header but not a space or an `=`
- [x] A retrying `transact` in the Go and TypeScript clients. Python had one
      and the other two left every caller to write their own backoff — the
      divergence the conformance runner cannot see, because it compares answers
      rather than ergonomics. Go has `slate.Transact`, a generic *function*
      rather than a method because Go has no generic methods and a method would
      have to return `any`; TypeScript has `session.transact`. Both retry a
      conflict and nothing else, with Python's defaults
- [ ] `EXISTS`, `UNION`, `INTERSECT`, `EXCEPT`, and a correlated subquery.
      Each refused by name with its reason rather than left to fail as a
      syntax error, and the reasons differ. `EXISTS` is correlated by nature —
      the inner query asks about each outer row, and a subquery here runs once
      — so it is refused with the `IN (SELECT …)` form that expresses the same
      question. `NOT EXISTS` is an anti-join, and so is the `NOT IN` it would
      rewrite to; the kernel has `Expr::In` and no negation of it, and adding
      one is not a parser change. The set operators have nowhere to go: a
      statement compiles to one query spec, which names one table and one
      plan, and two statements separated by `;` get everything but the
      deduplication. A correlated subquery needs no refusal of its own — the
      inner query is parsed against the inner table, so a column of the outer
      one is already "no such column" there
- [ ] `delete_if_unchanged`. Deleting a row somebody else just edited is the
      same class of mistake as overwriting it, and the same argument applies

- [ ] In-place promotion of a read-only node. A writer store is an opened
      database and may only be opened once the lease is won, so promotion is a
      restart — every request handler currently assumes the store it has is the
      store it started with. Specified in [`docs/topology.md`](docs/topology.md)
- [ ] The wire's deep-nesting refusal is inherited rather than written: a
      pathologically nested expression is stopped by prost's decode recursion
      limit, and the conversion functions themselves recurse without a depth
      counter of their own
- [ ] The cost model above 200,000 rows. The 200k calibration reproduces
      (1,223 GETs against 1,217 recorded) and is not an artefact of a warm
      cache, but the loader stops being linear somewhere between 500,000 and
      600,000 rows and four attempts past that never finished, so the
      constants at a million rows are still unmeasured. `POINT_READ_COST` is
      13–19% low at 200k — not enough to change a plan here
- [ ] What that loader cliff is. The row width and SlateDB's default 64 MB
      `l0_sst_size_bytes` line up suspiciously well with where it happens, and
      it is not machine load (400k and 500k took the same time at load 8.6 as
      at 2.5). Reproducible, unattributed, and the size that would settle it is
      the size that will not finish
- [ ] A ~250 µs residual rise in first-row latency near a batch of 125, left
      after the 2.3 ms step turned out to be the socket. Consistent with the
      per-row cost of a larger batch and at the edge of this harness's
      resolution, which is not a demonstration of either
- [ ] Correlated column statistics — selectivities still multiply, which
      assumes the columns are independent. Measured: estimates run up to 20x
      out, and the plan chosen is unchanged in every shape tested, so this is
      not currently worth fixing
      (`cargo run --release -p slate-kernel --example correlation`)
- [ ] Publishing. `slate-client` is not on PyPI and `@slate-orm/client` is not
      on npm; the docs page says so and installs them by path. Each needs a
      credential, a name nobody has claimed, and a decision about stability
      that has not been made, and wiring up a token to find out is the wrong
      order. The Go client needs no registry — a module path is its import
      path — and works today

## License

Apache-2.0
