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

> **Answered.** Every finding below now carries a verdict, written by the
> server side after the fact: **FIXED**, or **ANSWERED** where the conclusion
> was that the wire should not change and the argument is given instead. Two
> were refused (13 and 14) and both got the documentation whose absence was
> half the finding. Where a fix broke one of the pinning tests above, the
> finding says which and what it should assert now — those tests are correct
> to have failed, and are the reason the fix could be believed.
>
> The verdicts are in the findings; the design of the largest one is in
> `crates/slate-server/src/fingerprint.rs` and `docs/topology.md`.

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

## 2. There is no way to check a client's copy of the schema, and `ColumnRef` does not remove the need for one — FIXED

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

### The answer: a fingerprint, but asserted rather than advertised

**Status: fixed.** `SchemaCheck` — `columns` and a 64-bit fingerprint — is
optional on `Query` (so it covers every input of a join, every explain and
every aggregate) and on `Get`, `Insert`, `Update` and `Delete`. The server
checks it before resolving a single ordinal and refuses `INVALID_ARGUMENT`
without reading or writing anything. `crates/slate-server/src/fingerprint.rs`
has the argument in full; the short version is below, including the two places
this ended up differing from the proposal, both of which were forced by the
question "what happens across a migration".

**What is hashed.** The table's name; for each ordinal in order, the column's
name and type; and the ordinals that form the primary key. Nothing else.
Nullability, `DEFAULT`, `CHECK` and foreign keys are out because none of them
addresses a column, and a write that violates one is refused by name at write
time — a better error than a fingerprint mismatch on an unrelated read.
Indexes are out because a hint is advice and an unusable one is already only a
warning, so adding an index for performance must not break a client that never
names it. Table ids, index ids and schema versions are out because a client
cannot state them, and a fingerprint a client cannot compute is a constant it
has to be *told* — the schema on the wire by another route. Your finding 8 is
right that types belong in it, and they are in.

**How it survives a migration.** This was the whole of the design work, and
what makes it possible is a property of the schema layer rather than of the
wire: *an ordinal never moves*. A column is appended; a dropped column keeps
its ordinal for ever and holds nothing; a rename records the previous name and
goes on resolving it. So:

| migration | what happens |
|---|---|
| a column added | the older client's declaration is a **prefix**, and the server hashes exactly that prefix. It keeps working across the deployment |
| a column dropped | ordinal and declaration unchanged, so the fingerprint is unchanged. A client still *writing* the column is refused by name |
| a column renamed | accepted under **either** name. The schema layer promises that code written against the old one keeps working; a check that contradicted it would be checking the wrong thing |
| `DEFAULT`, `CHECK`, foreign keys | not hashed at all |

`crates/slate-server/tests/schema_check.rs` is written as those migrations
rather than as cases: each is two `TableDef`s, v1 and v2, with the claim
computed from v1 and checked against v2.

**Where this differs from the proposal, and why.** You suggested one opaque
string on a response, compared at startup: one field, no round trip. That is
cheaper and it was rejected, on the strength of the table above. With one
opaque value a mismatch is *uninterpretable* — the client cannot tell "your
declaration is wrong" from "the server has one more column than when you were
written" — and the only safe reaction to an uninterpretable mismatch is to
refuse to start. That is the additive migration taking down the fleet, reached
from the other side. The party holding both statements is the server, so the
claim goes to the server, which can be exactly as tolerant as its own schema
rules are, can say which way the disagreement runs, and refuses *the request
that would have been wrong* rather than hoping somebody ran a startup check.
The cost is a field on five requests rather than one on a response, and nine
bytes.

It is also, deliberately, still one-way traffic: a client may assert and be
refused, and nothing tells it what the answer is instead. That is what keeps it
on the right side of the `.proto`'s opening argument, which the file now says.

**The hash.** FNV-1a 64 over a length-prefixed canonical form, specified in the
`.proto` in one paragraph. FNV rather than SHA-256 because every language has
to reimplement it byte for byte and ten lines that can be checked by eye beat a
dependency; length-prefixed rather than delimited so that no column name can be
spelled to look like the end of a field. It is a check against drift, not
against an adversary — a client that wants to lie can simply not send one. The
canonical form is pinned in the test against a reference implementation written
in Python and printed there, because a fingerprint that agrees only with itself
proves nothing.

**Your test.** `test_the_width_check_cannot_see_a_rename` still passes — the
server-side check is what closes this, and a Python `Table` that declares
`category` where the catalog has `kind` still passes a *width* check. What
should change is the client: compute the fingerprint (the reference
implementation is in `schema_check.rs`), send it, and the rename is refused at
the point of use. Worth a second test asserting that, and worth keeping the
first as the statement of why a width check is not enough.

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

## 4. The response side has no `ColumnRef`, and puts the width arithmetic back — FIXED

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

**Status: fixed**, as the first of the two: `Row` carries `values` and
`computed = 2`. `computed_offset` on `QueryResponse` was the alternative and is
worse for one reason — it is still an offset, so the client still does the
addition, just with a number it was handed rather than one it wrote down. This
removes the addition.

Putting it on `Row` rather than on `QueryResponse` also means it applies inside
a `JoinedRow`, where it is needed and was not obvious: a join input's query can
compute values too, and that row was flat for the same reason and with the same
consequence. Each input is split at *its own* table's width.

`computed` is empty on a row travelling the other way, and a request that sets
one is now refused rather than having it dropped — an insert that appeared to
accept values it discarded is this failure in the opposite direction.

**Your `slate/rows.py::Row.computed`** can stop doing width arithmetic: the
values are in `row.computed`, positionally, in the order they were requested.
It should probably keep refusing when it has no `Table`, since it still needs
one to name things. Any test asserting that a computed value appears at
`width + i` in `values` now fails, and should assert `row.computed[i]`.

---

## 5. `warnings` exist only on `Explain`, so "my hint did nothing" is undebuggable on the request that ran — FIXED

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

**Status: fixed**, exactly as proposed. `repeated string warnings` on
`QueryResponse`, `JoinResponse` and `AggregateResponse`, on the first message —
the one that is always sent even for an empty result because it carries
`served_by`. Which is the part that makes this an omission rather than a
trade-off: the objection in `service.rs` was that "a stream has no header to
put them in", and it has one.

A warning is still not an error, because a hint is still advice: the query runs
and returns its rows. The refusal side of that ledger is unchanged — a forced
join algorithm that cannot be honoured is still an error, not a warning.

**Your test's second half now fails**, which is the point of having written it.
`test_a_hint_naming_an_index_that_does_not_exist_is_a_warning` should assert
that the warning appears on **both**: on `Explain`, and on the first message of
the query stream that actually ran. Worth adding the control the Rust test has
— the same query with no hint returns an empty `warnings` — so that a client
can treat a non-empty list as news.

---

## 6. The status-code mapping is many-to-one and nothing structured travels alongside it — FIXED

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

**Status: fixed**, as `google.rpc.ErrorInfo` in `grpc-status-details-bin`, which
is what you proposed. Every status carries one: a `reason` token per kernel
variant (`UNIQUE_VIOLATION`, `REPLICA_TOO_STALE`, …), `domain: "slate-orm"`,
and the variant's own payload as metadata — `index` and `table` on a unique
violation, `replica`/`required`/`visible` on a stale replica, `action` on an
access denial, `limit` on a build side too large. `NOT_LEADER` too, with
`leader`, which is not a kernel error at all but is `UNAVAILABLE` alongside
three that are.

Two notes on how it was built. The two well-known messages are declared in this
repository's own `proto/google/rpc/` rather than pulled in as a dependency:
they are twelve lines, the build is deliberately hermetic (`protox` parses the
schema in Rust so nothing needs `protoc`), and what has to agree across
languages is the bytes rather than the crate — your generated `google.rpc`
copies decode these unchanged. And `slate-leader` stays exactly where it is: it
predates this, `errors.py` reads it, and a redirect that only a rich-error
client could follow would be a regression.

The test that guards it is not "`UniqueViolation` says `UNIQUE_VIOLATION`",
which is a rename away from meaning nothing. It is that **no two errors behind
one status code share a reason** — the property is that the details undo the
collapse.

**For `errors.py`:** the hierarchy can now branch below the code without
matching on prose. `AlreadyExists` splits into "this id is taken" and "this
`{index}` is taken" from `metadata["index"]`; `Unavailable` splits four ways.
Nothing you have written breaks — details are additive — so this is a new test
rather than a changed one.

**Done, in all three clients.** `SlateError.reason` (`Error.Reason` in Go,
`error.reason` in TypeScript) now carries the token on a lone failure as well
as a batched one; before this it was populated only inside a batch, and the
comment on the field said so. The three decode the same captured blob in their
own suites, and the conformance runner compares the token on every refusal it
already had, so a client that decodes it differently from the other two fails
there rather than in somebody's logs.

The `metadata` map is deliberately still not surfaced. Its keys vary per
variant — `index` and `table` on a unique violation, `limit` on a predicate
write refused for size — and exposing it means promising something about keys
that differ from error to error. The token alone is what lets a caller branch
below a status code, and it is what three clients can agree on. The split of
`AlreadyExists` and `Unavailable` described above therefore remains available
rather than built: it needs the map, not the token.

---

## 7. A primary key of the wrong arity reads as "not found" rather than as a bad request — FIXED

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

**Status: fixed.** `Get` and `Delete` check the key against the table's own,
before encoding it, and refuse `INVALID_ARGUMENT` naming the key columns — the
same shape the insert path already used. Types are checked as well as arity,
because the tuple codec orders type-first: `i64(1)` where `u64` is declared
encodes to a different key, so it is a guaranteed miss rather than a coerced
lookup, which is the same failure wearing a different hat. A null in a key is
refused for the same reason. The check is in the request handler, so the
transactional path gets it too.

What is deliberately unchanged: a *well-formed* key for a row that is not
there, or for one the caller's policy hides, is still `found: false` and still
indistinguishable. That is right, and it is why the malformed case had to stop
sharing the answer — three different facts had one spelling and one of them was
a client bug that would never be found. The Rust test asserts the control as
well as the refusals, since without it the whole thing could pass by refusing
everything.

**Your local refusal is now belt and braces**, and worth keeping: it is a
better error, sooner, and it still catches the case where the client's own
`Table` is what is wrong. Any test asserting `found is False` for a malformed
key now gets `INVALID_ARGUMENT` instead.

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

**Cross-reference, not a verdict:** the last paragraph of this finding asked
that the fingerprint cover types as well as names. It does — `SchemaCheck`
hashes each column's declared type, so the declaration a client must hold in
order to write at all is now the declaration the server checks. Nothing else
here was acted on; the coercion is still undocumented and this client is still
right to be stricter than it needs to be.

---

## 9. `WriteResponse.affected` carries no information for `Insert` and `Update` — ANSWERED

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

**Status: answered — the first of your two, and not the second.** The comment
now says it in full: read `affected` as "how many rows the server agreed to",
with a line per operation saying which of the three carries a fact the caller
did not already have (only `Delete`), and why the other two cannot be anything
else — both are all-or-nothing, so any number other than `len(rows)` would mean
the response arrived after a partial write, which cannot happen.

Making it `optional` and unsetting it was rejected. It would turn the common
`affected == len(rows)` assertion into language-specific handling of an absent
field, in exchange for making a true statement slightly more emphatic; and it
is a wire break for every client already reading it. The field is not wrong,
the reading of it was, and a comment is the right size of fix for that.

The upsert case you raise is genuinely lost — 1 for a replacement and 1 for a
creation — and the reason is worth recording: distinguishing them needs a read
the write path does not do, so reporting it would cost a round trip per row to
tell the caller something they can find out with a `Get`.

---

## 10. `Freshness` has two ways to spell "any", and the second is the field a zeroed struct produces — FIXED

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

**Status: fixed**, with exactly the trick you point at. There is a one-value
`Unit` enum now, and `Freshness.any` and `Freshness.latest` are both `Unit`, so
`latest: false` is not a thing that can be sent. A client with nothing to say
leaves the whole message absent, which has always meant `ANY`; a `Unit` arm
carrying anything but `UNIT` is refused rather than read as a selection, which
is what makes the false spelling unrepresentable rather than merely
discouraged.

**Two others had the same defect and were fixed with it**, because leaving two
of three is the kind of inconsistency the next client reports: `AccessHint`'s
`bool table_scan` and `JoinAlgorithm`'s `bool nested_loop`. The second is the
clearest case — `convert.rs` had an explicit arm reading `nested_loop: false`
as "no algorithm forced", with a comment pointing at `Freshness.latest` as the
precedent. That arm is gone.

Field numbers 1 and 3 on `Freshness`, 1 on `AccessHint` and 2 on
`JoinAlgorithm` are `reserved`, not reused. Both are varints, so an old
client's `false` would decode as `UNIT` on a new field and select the arm it
was trying *not* to select; reserving makes it a parse failure instead.

**Breaks your client's spelling of all three.** `freshness.py` must send
`any=UNIT` / `latest=UNIT` (or, better, send nothing for `ANY`), and anything
constructing a table-scan hint or a forced nested loop must do the same.
`test_freshness.py`'s assertion that `Freshness(any=False)` returns every row
should become: an absent `Freshness` returns every row, and a `Unit` arm
carrying a value the server does not know is `INVALID_ARGUMENT`.

---

## 11. `build_limit` reads like a property of the request and is a property of one algorithm — ANSWERED

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

**Status: answered — the sentence is there**, and it is more than one, because
the consequence you found deserves stating: lowering `build_limit` protects a
node only on the plans the planner costed as hash joins, and on the others it
does exactly nothing. The comment now names the algorithm in its first line,
says what a nested loop does with it (nothing, there is no build side), points
at `ExplainJoin` for finding out which each input got, and says that a caller
who *must* have the bound should force `hash_build` and take the plan they
asked for.

Not made an error, and not made to apply to a nested loop. A nested loop holds
nothing, so there is nothing there to bound; a limit that pretended to would be
a worse lie than the one the comment fixed. `docs/topology.md` carries the same
sentence.

Your `test_resource_exhausted_when_a_build_side_is_too_large` is right to force
`hash_build_left` and should keep doing so — that is the documented way to get
the bound, not a workaround any more.

---

## 12. A transactional read is materialised, and the wire cannot say so — ANSWERED

`Sessions::query` cannot stream from inside a transaction, so it collects the
whole result and replays it through the same batched `QueryResponse`. The wire
shape is identical, so a client cannot tell.

That means the same `Query` has two different memory profiles on the server
depending on whether a `transaction` field is set, and the difference — a whole
result set held in the head node — is invisible in the `.proto`. It caught me
out only because I read `session.rs`. `QueryResponse` or `QueryRequest` should
say it.

**Status: answered — it says it**, on `QueryRequest.transaction`, which is the
field that decides it. It states the fact (a read inside a transaction is
materialised), the reason (a cursor borrows the transaction, which lives inside
a task that owns it, so nothing can stream out), the consequence (the result
set is in the head node's memory before the first message leaves), and what to
do instead (a large read goes outside a transaction, where it streams and the
channel gives back pressure; inside one is for the small read that decides what
to write next). It also notes that `Join` behaves the same way and that
`Aggregate` is materialised either way, because grouping folds every row before
any group is final.

Not made visible in the response. A flag saying "this one was materialised"
would be a field every client parses to learn something it already knows — it
set the `transaction` field itself — and the honest fix for "the wire cannot
say so" is for the wire to say so where the decision is made.

---

## 13. `Get` is the only unbatched operation — ANSWERED, and refused

`Insert`, `Update` and `Delete` all take `repeated`. `Get` takes one key. A
client fetching fifty rows by key either makes fifty round trips or rewrites
the request as a `Query` with an `IN` over the primary key — which works, and
the planner turns it into point gets, but it means the natural operation has
two spellings with different shapes and only one of them is obvious.

Given that `insert_many` exists precisely because overlapping reads across a
batch is the whole win, a `repeated Row primary_keys` on `GetRequest` looks
like the same win on the read side.

**Status: refused, and documented.** The premise is right — overlapping the
reads across a batch is the whole win — and it is the reason not to add this.

`insert_many` overlaps because the *kernel* overlaps: it issues the
duplicate-key reads together. There is no `get_many` under it to do the same,
and the thing that already gets the overlap on the read side is the second
spelling you found: `Query` with `IN` over the primary key, which the planner
turns into the point reads it actually is, sixteen in flight. A `repeated Row
primary_keys` on `GetRequest` could not reproduce that. `Expr::In` names one
column, so the rewrite is available for a single-column primary key and not for
a composite one, and the server would fall back to a loop — giving a batch
operation that costs one round trip on some tables and fifty on others, with
nothing in the response to say which. That is a worse thing to have on a wire
than an asymmetry, and it is the same complaint as finding 11 one level up: a
field that reads as a property of the request and is a property of one plan.

What *was* wrong is the half of the finding that says "two spellings with
different shapes and only one of them is obvious". That is a documentation
failure and it is fixed: `GetRequest` now opens by saying that it takes one key
on purpose, that the batched read is `IN` over the primary key, and why the
obvious-looking alternative is not offered. `docs/topology.md` lists it under
Not built with the same reasoning.

A kernel `get_many` would change the answer — it would make a batched `Get` the
fast path on every key shape — but that is a kernel change and this is a wire
finding.

---

## 14. There is no write by predicate — ANSWERED, and refused

Every write names whole rows or whole primary keys. `DELETE FROM docs WHERE
kind = 'x'` is a query followed by a batch delete, in the client, and is atomic
only if the client remembers to open a transaction. `UPDATE ... SET size =
size + 1 WHERE ...` cannot be expressed at all — the wire has no way to say
"this column, this expression", only "here is the whole new row".

That may well be deliberate (the kernel's `update` is a row replacement too),
but it is not stated anywhere, and it is the first thing anyone coming from SQL
will look for. A sentence under `UpdateRequest` saying that an update is a
replacement and there is no predicate form would save the search.

**Status: refused, and the sentence is there** — which is what the finding
actually asked for, and it was right to ask.

`UpdateRequest` now opens with it: an update is a row replacement, there is no
`SET column = expression` and no predicate form; `UPDATE ... SET size = size +
1 WHERE kind = 'x'` is a query followed by an update of the rows it returned,
and it is atomic if and only if the client opens a transaction around the pair.
A delete by predicate is the same shape. `DeleteRequest` and the crate docs
agree.

Why not build it. Two different things would have to exist. "This column, this
expression" is the kernel's expression evaluator applied to a *write*, which
the kernel does not have and which is not this crate's to add. A write that
walks a cursor is something the head node would have to implement over rows it
had already streamed — which is the same objection that keeps a grouped join
out: a second implementation of something the kernel owns, in the layer above
it, with nothing to be an oracle against. Doing it here would mean the
predicate write and the ordinary write could disagree about row-level security,
about `CHECK`, or about a foreign-key closure, and nothing in the test suite
would be positioned to notice.

The client-side composition is not a workaround so much as the honest shape of
it, and the wire now says so rather than leaving it to be discovered.

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
