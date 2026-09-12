# slate-orm

A **record layer** for [SlateDB](https://slatedb.io) — typed schemas, secondary
indexes, query planning and row-level security over an ordered key-value store
on object storage.

The closest blueprint is FoundationDB's Record Layer rather than an ORM. The
derive macro is a surface; the value is in the kernel underneath it, which owns
the keyspace, index maintenance, and the point where access policy is enforced.

> **Status: early.** The Rust record layer works end to end and is tested. The
> gRPC head node and the Python, Go and TypeScript SDKs are not built yet. See
> [Status](#status).

```rust
use slate_orm::{Record, Records, Expr, ScanOrder};
use uuid::Uuid;

#[derive(Record)]
#[record(table = "users", id = 1, tenant = "tenant_id")]
struct User {
    #[record(pk)] tenant_id: Uuid,
    #[record(pk)] id: u64,
    #[record(index(name = "by_email", id = 10, unique))]
    email: String,
    nickname: Option<String>,
}

let txn = store.begin().await?;
txn.insert_record(&ctx, &user).await?;
txn.commit().await?;

// Column ordinals are generated as constants, so a predicate needs no
// fallible name lookup.
let filter = Expr::eq(User::COLUMNS.email, Value::Str("a@example.com".into()));
let found: Vec<User> = txn.find_records(&ctx, filter, ScanOrder::Ascending).await?;
```

## Layout

| crate | what it owns |
|---|---|
| `slate-tuple` | Order-preserving tuple encoding, the closed value model |
| `slate-schema` | Table, column and index definitions; the row body codec |
| `slate-kernel` | Keyspace, record store, planner, executor, RLS/RBAC |
| `slate-slatedb` | SlateDB backend |
| `slate-derive` | `#[derive(Record)]` and generated column constants |
| `slate-orm` | Typed surface; re-exports the rest |

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

### Bounds narrow, the residual decides

The planner is heuristic on purpose — it matches equality terms against an
index's leading columns and allows one range on the next — because a cost model
needs statistics and statistics need a subsystem.

Scan bounds are treated as an optimisation only. Every conjunct stays in the
plan's residual and is re-checked per row, even when the bounds already imply
it. That costs a little work and buys two things: a bound-derivation bug can
make a scan slow but not wrong, and a mandatory security predicate is enforced
by evaluation rather than by the planner having correctly turned it into a
range. Dropping provably-redundant conjuncts is a later optimisation that has to
be argued for rather than assumed.

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

## Testing

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

Tests are written around guarantees rather than API surface:

- The tuple codec's properties are checked at 400k cases: byte order matches
  value order under arbitrary per-column directions, encodings are prefix-closed
  and prefix-free, value equality and encoding equality are the same relation.
- `bounds_never_lose_rows` checks that a planned query returns exactly what
  brute-force filtering returns, across a spread of predicate shapes and both
  scan orders. It was verified to have teeth by breaking `<=` into `<` and
  confirming it caught the lost boundary row.
- The security suite is written around what a caller *cannot* do: probe for
  hidden rows via error codes, reach another tenant by asking explicitly, escape
  a policy by updating out of it, or leak null-valued rows through a negated
  policy.
- The SlateDB integration tests re-check against a real instance the properties
  the kernel suite proves against the in-memory backend — most importantly that
  two concurrent writers cannot both take a unique index slot, which is what
  says the in-memory backend's conflict detection is a faithful stand-in.

## Status

Built and tested:

- [x] Order-preserving tuple codec
- [x] Schema, catalog, row codec with versioned evolution
- [x] Keyspace, storage abstraction, in-memory backend
- [x] Record store: primary key CRUD with atomic index maintenance
- [x] Scan/filter executor and heuristic index selection
- [x] RLS predicate injection and RBAC catalog
- [x] `#[derive(Record)]` and the typed surface
- [x] SlateDB backend

Not built:

- [ ] gRPC head node — the single-writer server the topology above describes
- [ ] Read replicas off SlateDB checkpoints/clones
- [ ] Python, Go and TypeScript SDKs, which need the head node first
- [ ] Migrations beyond additive nullable columns (no column drop or rename)
- [ ] Cost-based planning, covering/index-only scans, joins
- [ ] `IN` as multiple index ranges (today it is a residual filter)

## License

Apache-2.0
