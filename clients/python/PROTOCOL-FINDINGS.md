# What using this protocol was like

This is the deliverable. The client in `src/slate` is the thing that produced
it: it is the first consumer of `slate/v1/records.proto` from outside Rust, and
what follows is every place I had to guess, work around something, or write a
comment apologising for the wire.

Fifteen of them, ordered by how much I think they matter rather than by how
easy they are to fix. Each says what I measured, because several of my first
guesses were wrong and the wrong ones are recorded alongside the right ones.

Nothing under `crates/` was changed. Where a finding is a behaviour rather than
an opinion, there is a test in `tests/` that pins the behaviour that exists —
not the behaviour I expected — so that a future fix has something to break.

---

## 1. The head node ships no binary, so a client cannot start the thing it is a client of

`slate-server` is a library. `Head::new` takes a `Catalog`, a
`SecurityCatalog`, an `Arc<S: KvStore + KvReadStore>`, a
`Vec<Arc<dyn KvReadStore>>`, a `Leadership` and an `Authenticator`. Every one
of those is a Rust value that only Rust can construct. There is no `main`, no
configuration file, no `--catalog schema.toml`.

The practical consequence is that the first ~700 lines I wrote for this task
were Rust, not Python: `clients/python/testserver` exists solely so that there
is a process to connect to. It also had to restate the fixture schema, because
`crates/slate-server/tests/common/mod.rs` is a test module and is not exported
— so the fixture now exists three times (that module, my `main.rs`, and
`tests/fixture.py`) with nothing keeping them equal.

This is the largest single piece of friction in the exercise and it is entirely
outside the `.proto`. Every other-language client will write the same binary,
differently, and each will get the `Authenticator` choice slightly wrong in its
own way. A `slate-server` binary that reads a catalog from a file — even a
deliberately limited one, even marked "for development" — would remove all of
it.

**Related and smaller:** `HeadConfig::new` requires an `Authenticator` with no
default, and the argument for that (in `auth.rs`) is excellent. But the only
one shipped is `MetadataIdentity::trusting_the_caller_completely()`, so every
deployment that is not behind a mesh has to write one before it can start. That
is the intended outcome; it is worth knowing that it makes "run the server" a
programming task.

---

## 2. There is no way to check a client's copy of the schema, and `ColumnRef` does not remove the need for one

The `.proto` refuses to publish the catalog, and the argument is good: a
`Describe` puts the schema on the wire, and it turns every query into two round
trips or a cache that can go stale — "a stale width being a silently
re-pointed predicate again".

But `ColumnRef` removed the *cross-table* arithmetic, not the *within-table*
ordinal. A request still says "input 1's column 3", and a client only knows
that `title` is column 3 because it wrote that down somewhere. On the Rust side
the derive macro generates those constants from the same declaration the server
serves, so they cannot disagree. **There is no equivalent for any other
language.** A Python `Table` is a hand-written restatement, and if it drifts —
a column inserted in the middle, two same-typed columns swapped, a rename —
every reference past the drift names a different column, the server accepts it,
and the query returns plausible rows.

That is precisely the "same query, different answer" failure the reference
model was introduced to prevent. It has been moved one level, not removed.

What a client can check, and what this one does
(`tests/test_fixture.py`): read one row and compare its width to the
declaration. That catches a column appended or removed. It does not catch a
rename, and `test_the_width_check_cannot_see_a_rename` asserts that it does
not — a `Table` declaring `category` where the server has `kind` passes the
check and then filters on the wrong column with no error anywhere.

Three fixes, cheapest first, none of which puts the schema on the wire:

- **A catalog fingerprint.** One opaque string on `ServedBy` (or any response),
  derived from table names, column names, ordinals and types. A client that
  computes the same string from its own declaration and compares once at
  startup gets the whole property for one field and no round trip. This is by
  far the best value.
- **An expected-width field per request.** `Query.expected_columns`, refused on
  mismatch. Narrower, and it catches the common case at the point of use.
- **`Describe`.** The thing the file rejects. I agree with the rejection; I
  raise it only to say that the alternatives above are not it.

---

## 3. `Freshness.at_least(n)` was not checked against the writer — FIXED

**Status: fixed in `crates/slate-kernel/src/pool.rs` after this was reported.**
The pinning test was written asserting the broken behaviour so that a fix would
have something to break; the fix broke it, and it is now
`tests/test_freshness.py::test_a_sequence_no_view_has_reached_is_refused`,
asserting the promise instead. The pool now holds the writer to the same proof
as every replica: reachable is served, unreachable is `Unavailable`. A writer
that reports no sequence at all is still served, which is the store declaring
it cannot prove the promise rather than the pool declining to check — the one
branch that takes freshness on trust, and it says so.

An older Rust test, `a_freshness_no_view_can_meet_is_refused_rather_than_served_stale`,
had the right name and a weaker assertion: it only checked that the *frozen
replica* did not serve the read, which the unconditional writer fallback
satisfied. The name was right and the assertion was not, which is why nothing
in Rust caught this and a client from outside did.

The original report follows.


The wire says `at_least` means "only a view that has reached this sequence".
`topology.md`'s failure table lists "Replica behind the required token → pool
waits, then falls back to the writer" and "Replica behind and no writer in the
pool → `NoReplicaAvailable`, **not a stale read**".

`ReplicaPool::route` implements the fallback unconditionally:

```rust
// Falling back to the writer costs the scarce resource, but serving a
// read that is knowably too old would break the promise the token makes.
self.writer.as_ref().ok_or(KernelError::NoReplicaAvailable { ... })
```

It returns the writer without asking whether the writer has reached `n`. Asking
for `at_least(2**40)` against a writer at sequence 2 returns rows, with no
error. Measured:

```
asked at_least=2               served_by=(replica='memory', sequence=2) rows=8
asked at_least=3               served_by=(replica='memory', sequence=2) rows=8
asked at_least=1099511627776   served_by=(replica='memory', sequence=2) rows=8
```

The comment above the fallback states exactly the invariant the code does not
check. Every other branch of `route` checks it: the fast path tests
`token.satisfied_by(visible)`, the slow path calls `wait_for_sequence`, whose
default implementation refuses when `visible < sequence`. Only the writer is
exempt, and the exemption is implicit.

Is it reachable in practice? Not from a token this client produced against this
node. It is reachable from a token that outlived its database — persisted
across a restore, replayed from a queue after a rebuild, or carried across a
deployment boundary — and in each of those the answer a client gets is stale
data rather than an error. It is one `visible_sequence()` check to close.

A client *can* detect it: `served_by.sequence` truthfully reports 2. Nothing on
the wire suggests a client should compare it against what it asked for, and no
client will.

---

## 4. The response side has no `ColumnRef`, and puts the width arithmetic back

The whole argument for `ColumnRef` is that a client must never add a table
width to an index. On the way out, it does not. On the way back, it must.

A single-table query's `Row` is one flat `repeated Value`: the table's own
columns, then the computed values, "in the order they were requested". To read
computed value *i* the client computes `table_width + i` — from a width the
protocol declines to publish, with exactly the failure the model exists to
prevent: add a column to the table and every computed value moves one slot,
silently.

`slate/rows.py::Row.computed` is the only width arithmetic in this package. It
is confined to one method, it refuses rather than guessing when it has no
`Table`, and it is a workaround.

What makes this a clear omission rather than a trade-off is that the *other two*
result shapes got the treatment:

| result | shape | client arithmetic |
|---|---|---|
| `JoinedRow` | one `Row` per input | none |
| `Group` | `key` and `values` kept apart | none |
| `Row` with computed values | one flat list | **width + i** |

The `.proto` even says why the other two are split: "the client should not have
to do the arithmetic that `ColumnRef` exists to remove". The same sentence
applies here and the same fix is available — a second `repeated Value computed`
on `Row`, or a `computed_offset` on `QueryResponse`, either of which is
backward compatible.

---

## 5. `warnings` exist only on `Explain`, so "my hint did nothing" is undebuggable on the request that ran

An ignored index hint and a clamped `build_limit` are both, in the `.proto`'s
own words, "things the server did with the request that the request did not ask
for". They are attached to `ExplainResponse` and `JoinExplainResponse` and to
nothing else. `Query`, `Join` and `Aggregate` throw them away —
`service.rs::query` literally binds them to `_warnings`.

So a caller who hints an index that was renamed gets a slower query and no
signal. To find out, they must issue a *different* RPC and trust that the
planner made the same decision on a request that ran at a different moment
against a possibly different view.

`QueryResponse` and `JoinResponse` already have a first message that is always
sent even when empty, because it carries `served_by`. A `repeated string
warnings` beside it costs one field and closes this.

Pinned in `tests/test_explain.py::test_a_hint_naming_an_index_that_does_not_exist_is_a_warning`,
which asserts both halves: the warning appears on `Explain`, and the same
request run for its rows returns them with nothing said.

---

## 6. The status-code mapping is many-to-one and nothing structured travels alongside it

`status.rs` is careful and well argued about *which* code each kernel error
gets. What it cannot do is preserve the distinction after the collapse:

| code | kernel errors behind it | what a client would do differently |
|---|---|---|
| `UNAVAILABLE` | `WriterFenced`, `ReplicaTooStale`, `NoReplicaAvailable`, `Storage` | retry elsewhere / retry later / lower your freshness / page someone |
| `ALREADY_EXISTS` | `DuplicatePrimaryKey`, `UniqueViolation` | "this id is taken" vs "this **email** is taken" |
| `NOT_FOUND` | `UnknownTable`, `RowNotFound` | a bug in my code vs a row that was deleted |
| `PERMISSION_DENIED` | `AccessDenied`, `TenantRequired`, `RowCheckFailed` | fix the grant / attach a tenant / the row failed a CHECK |

The `ALREADY_EXISTS` row is the one that hurts. An application that inserts a
user and wants to say "that email address is already registered" cannot tell
which constraint fired, and `UniqueViolation`'s own payload — which index — is
exactly what it needs.

The distinctions are recoverable only from `details()`, which is prose. This
client does not match on it, and `errors.py` says why: a message is not an
interface, it is not tested for stability anywhere in the server, and a client
that branches on it breaks silently when somebody improves the wording. So the
hierarchy in `slate/errors.py` stops at the code, and the finer distinctions
are unavailable to every consumer of this protocol.

The standard fix is `google.rpc.ErrorInfo` in the status details: a `reason`
string (`UNIQUE_VIOLATION`), a `domain`, and a metadata map (`index`,
`table`). It is one well-known message and it makes the whole of `KernelError`
addressable without putting any of it in the `.proto`.

The one structural discriminator that *does* exist — the `slate-leader` trailer
— is the proof that this works: it is the only refinement `errors.py` makes on
top of a status code, and it is the only one available.

---

## 7. A primary key of the wrong arity reads as "not found" rather than as a bad request

Measured:

```
get docs [u64(1)]                -> found=True
get docs [u64(1), str("x")]      -> found=False      <-- two values, one-column key
get docs []                      -> found=False      <-- no values at all
```

`GetRequest.primary_key` is a `Row`, and `values_from_proto` decodes it without
checking it against the table's key. A client bug becomes an absent row.

That is worse here than it would be elsewhere, because `found: false` is
*deliberately* indistinguishable from "a row your policy hides" — the `.proto`
says so, and it is the right call. It means a malformed key is
indistinguishable from a legitimate miss and from an authorisation outcome, all
at once. The write path does not have this problem: an insert with the wrong
number of values is refused with `table 'docs' expects 4 column(s), row has 1`.
`Get` and `Delete` should say the same thing.

This client refuses it locally (`Table.key_types` checks the arity) with a
message naming the key columns, which is a workaround, not a fix — the next
client will not.

---

## 8. Writes require the exact declared type; predicates coerce. I guessed the opposite and measured it

This is the finding I got wrong first, so it is written up as what happened.

`Value` has both `int64_value` and `uint64_value`, and `slate_tuple::Value`
orders type-first. I assumed a filter carrying the wrong integer width would
silently match nothing, designed `slate.values` around refusing an ambiguous
`int`, and then measured it:

```
docs.id is u64, docs.size is i64
id  == u64(1)    planner=1  table_scan=1
id  == i64(1)    planner=1  table_scan=1     <-- coerced, and consistently
size >  u64(25)  planner=4  table_scan=4  by_size=4
size >  i64(25)  planner=4  table_scan=4  by_size=4
IN over a u64 column with i64 values -> 2, on every access path
```

Comparisons and `IN` coerce numerically, and — importantly — they agree across
the planner's choice, a forced table scan and a forced index. There is no
access-path divergence here, which is the failure that would have mattered.

The write path is the opposite, and is strict:

```
insert docs with id as i64(900) -> INVALID_ARGUMENT:
    column `id` on table `docs` expects u64, got i64
```

So the actual requirement is: **a client must know each column's declared type
in order to write at all**, and the protocol publishes no types. The refusal is
loud and names both types, which makes it a development-time problem rather
than a production one — much better than I assumed. But it is another thing
`Table` has to state locally and cannot verify, and it compounds finding 2: the
fingerprint suggested there should cover types as well as names.

Two notes on what I did with this:

- This client still refuses a bare `int` where no type is declared (a computed
  value, a group key, an aggregate) and requires `i64(...)` / `u64(...)`. That
  is stricter than the server needs. I kept it because the coercion is a
  measured kernel behaviour, not a documented wire contract — the `.proto` says
  nothing about it — and a client that depends on undocumented leniency breaks
  when it is tightened. `tests/test_wire_behaviour.py` pins the coercion so that
  a change is visible.
- If the coercion were documented, this constraint could be relaxed and the API
  would be noticeably nicer.

---

## 9. `WriteResponse.affected` carries no information for `Insert` and `Update`

The field is documented as "How many rows the write found to act on". For
`Delete` that is true and useful — it counts the keys that existed, and a row
the caller's policy hides counts as absent.

For `Insert` and `Update` the server returns `rows.len()`: the number it was
handed. Both operations refuse the whole batch rather than applying a prefix,
so `affected == len(request.rows)` is a tautology and the round trip carries no
new fact. An upsert over an existing row also reports 1, so it does not
distinguish an insert from a replacement either.

Either the comment should say "echoed for an insert and an update", or the
field should be `optional` and unset there. As it stands the natural reading —
"how many rows changed" — is wrong for two of the three writes.

---

## 10. `Freshness` has two ways to spell "any", and the second is the field a zeroed struct produces

`freshness_from_proto` reads `Level::Latest(false)` as `Freshness::Any`, and
the comment gives the right reason: "`latest: false` is what a client that
zeroed the field sends. It is not a request for the writer, and reading it as
one would send every such read to the scarcest resource in the deployment."

That reasoning is sound and the outcome is still a wire where
`Freshness { latest: false }` means something other than what it says, in a
file whose opening argument is that an unset `oneof` must never be defaulted
because "a `Value` with no kind set is most likely a client built against a
newer schema".

The file already contains the fix it applied elsewhere: `NullValue` is a
one-value enum precisely so that "the client sent a null" cannot be confused
with "the client sent nothing". `any` and `latest` could each be a one-value
enum for the same reason, and then `latest` would be unrepresentable as false.

Confirmed on the wire: `Freshness(any=False)` returns every row, i.e. `ANY`.

---

## 11. `build_limit` reads like a property of the request and is a property of one algorithm

The comment — "Rows a build side may hold in memory before the read is refused"
— is accurate, and a caller will still read the field as a cap on the join.

It only bites a **hash** build side. On a nested loop there is no build side and
the limit does nothing. Writing
`tests/test_errors.py::test_resource_exhausted_when_a_build_side_is_too_large`
meant discovering that the fixture is small enough for the planner to choose a
nested loop, so the test had to force `hash_build_left` before
`RESOURCE_EXHAUSTED` was reachable at all. A caller lowering `build_limit` to
protect a node would get no protection at all on the plans that happen to be
nested loops.

Worth one sentence in the field comment.

---

## 12. A transactional read is materialised, and the wire cannot say so

`Sessions::query` cannot stream from inside a transaction, so it collects the
whole result and replays it through the same batched `QueryResponse`. The wire
shape is identical, so a client cannot tell.

That means the same `Query` has two different memory profiles on the server
depending on whether a `transaction` field is set, and the difference — a whole
result set held in the head node — is invisible in the `.proto`. It caught me
out only because I read `session.rs`. `QueryResponse` or `QueryRequest` should
say it.

---

## 13. `Get` is the only unbatched operation

`Insert`, `Update` and `Delete` all take `repeated`. `Get` takes one key. A
client fetching fifty rows by key either makes fifty round trips or rewrites
the request as a `Query` with an `IN` over the primary key — which works, and
the planner turns it into point gets, but it means the natural operation has
two spellings with different shapes and only one of them is obvious.

Given that `insert_many` exists precisely because overlapping reads across a
batch is the whole win, a `repeated Row primary_keys` on `GetRequest` looks
like the same win on the read side.

---

## 14. There is no write by predicate

Every write names whole rows or whole primary keys. `DELETE FROM docs WHERE
kind = 'x'` is a query followed by a batch delete, in the client, and is atomic
only if the client remembers to open a transaction. `UPDATE ... SET size =
size + 1 WHERE ...` cannot be expressed at all — the wire has no way to say
"this column, this expression", only "here is the whole new row".

That may well be deliberate (the kernel's `update` is a row replacement too),
but it is not stated anywhere, and it is the first thing anyone coming from SQL
will look for. A sentence under `UpdateRequest` saying that an update is a
replacement and there is no predicate form would save the search.

---

## 15. Two documentation claims are stale, and both sent me looking

Minor, and grouped because they cost the same thing: time spent looking for
something that is already there, or believing something that is not.

**`docs/topology.md`, under Not built:**

> **`update_many`.** `insert_many` overlaps its reads across a batch; a
> multi-row update still costs a round trip per row, here as in the kernel.

`service.rs::Write::apply` calls `transaction.update_many(context, table,
rows)`, with a comment saying it "reads whether each row exists in one wave,
the same as `insert_many`". The commit `54e7ec9 Add update_many` is in the log.
The Not-built list is otherwise reliable enough that I believed it and designed
around a per-row cost that is not there.

**`README.md`, in the status banner (line 13):**

> The gRPC head node and the Python, Go and TypeScript SDKs are not built yet.

The head node is built, is in the crate table sixteen lines below, and is
checked off in the Status section at line 626 — "A gRPC head node with writer
leadership". Only the banner disagrees. Since this work adds `clients/python`,
the banner needs a second edit anyway.

---

## What I could not exercise

Stated so the gaps are visible rather than absent.

- **`UNKNOWN` / `CommitTimedOut`.** Needs a storage layer that can time out a
  commit; `MemoryStore` cannot. So the single most important line in
  `slate/errors.py` — that `UnknownOutcome` is not `Retryable` — is asserted
  as a property of the hierarchy and never against a real status.
- **`DATA_LOSS` / `CorruptIndexEntry`.** Needs a corrupted store.
- **A real fence.** `WriterFenced` is reachable only by opening a second
  `SlateStore` over one object store. My test server uses `MemoryStore` and a
  fake lease, so `NotLeader` is tested through a node that never won the lease
  rather than through one that lost it. The trailer and the code are the same;
  the step-down transition is not covered.
- **A real replica.** The "stale replica" is an empty in-memory store that
  refuses every `wait_for_sequence`, copied from the server's own
  `freshness.rs`. Nothing here observes a replica that genuinely catches up, so
  the wait-and-then-succeed path of `ReplicaPool::route` is untested from a
  client.
- **Vectors, UUIDs, bytes and the time scalars.** `Value` carries them and
  `slate/values.py` encodes them, but the fixture schema has no column of those
  types, so nothing round-trips one through the server. `Metric`, `TimeUnit`,
  `distance`, `extract`, `date_trunc`, `regexp_replace`, `case` and `coalesce`
  are built and type-checked and never executed.
- **Concurrency beyond two writers.** The conflict test contends by
  construction with a barrier between exactly two transactions.
