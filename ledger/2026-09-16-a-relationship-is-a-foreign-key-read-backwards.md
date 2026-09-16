# Relations cross the wire, named by the foreign key that already declares them.

- **Date:** 2026-09-16
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-server` (`records.proto`, `service.rs`),
  `clients/python` (`client.py`, `_proto/*`, `testserver`, `tests/fixture.py`,
  `tests/test_related.py`)
- **Kind:** feature

## What changed

A `Related` RPC. Given a set of parent key values it returns the related rows
grouped by the value that related them, in one read, in either direction.
Python gets `Session.related(...)`; Go and TypeScript do not yet.

The relationship is named by `(table, foreign_key, direction)` and resolved
server-side against the catalog. The testserver gains `libraries`/`shelves` —
a parent and child with a real foreign key — because nothing in the existing
fixture had one.

## Why

P2 of the ORM audit, and the larger of the two gaps it found. `load_related`
batches and dedupes and has no lazy fallback to trigger by accident, which is
better than the shape SQLAlchemy, Ecto and Prisma all have — and it was
reachable only from Rust. A Python, Go or TypeScript caller wrote the two-step
fetch by hand, and the one who forgot wrote the N+1 the Rust API exists to
prevent.

## Alternatives rejected

**A new `RelationDef` in the catalog**, which is what the plan in
`docs/orm-comparison.md` assumed. Reading the code to build it showed the
information is already there: `books.author_id -> authors` is a foreign key,
"an author's books" is that key read backwards, and `Catalog::referencing`
already walks it — `deletion_closure` has used that for cascades all along. A
second declaration of the same fact creates the possibility of the two
disagreeing, and a catalog claiming a relationship where no constraint enforces
it describes rows the database will not keep. The plan was wrong and is
corrected.

The cost is real and is written up: a relationship *not* backed by a foreign key
cannot be named on the wire, and the Rust `Related` trait does allow one.

**Relations as a field on `QueryRequest`.** Fewer messages, and it conflates two
things: the response shape is grouped by relating value rather than by row, and
folding that into `QueryResponse` would put an optional grouping on every read.

**Client-side resolution** — the client sends columns, the server filters.
Keeps the server ignorant of relationships, and makes each client describe the
relationship for itself, which is precisely how three clients come to describe
it differently. Naming it is what the conformance runner can check.

**Repeating the rows per parent in the response.** Simpler for the caller, and
it undoes on the response the saving the deduplicated read just made — on a
has-many over a popular parent that is most of the bytes. Grouped by key, with
the caller mapping its own parents on.

**Adding the foreign key to `books`.** The obvious fixture, and it would have
changed what an insert into `books` is allowed to do. Six test files write
books without writing an author first, so the constraint would have been tested
by breaking tests about something else.

**Refusing every composite foreign key.** The first realistic schema reached for
had one, because tenancy puts the tenant in the key: `(tenant_id, library_id)`
against `(tenant_id, id)`. Refusing those would refuse the commonest
multi-tenant shape. It does not need refusing — `tenant_filter` forces
`tenant_column = principal.tenant` onto every read of a tenant-scoped table, so
the tenant is pinned before the relating filter applies and matching on it again
is a tautology. The relating column is the one key column that is *not* the
tenant; two non-tenant columns is a genuine composite and is refused by name,
because the filter compares one column against one list.

## Evidence

**Nine Python tests against a real daemon**, and the whole suite is 181 passing.
Two are load-bearing beyond "it returns rows":

- `test_it_is_one_request_however_many_parents` counts calls at the stub. Fifty
  parents, one request. A version that looped would return identical rows, so
  counting is the only way to see the claim.
- `test_the_rows_agree_with_the_same_question_asked_as_a_query` is an oracle:
  the same answer assembled from three ordinary filtered reads. Not a
  restatement of the seed data, so a `related` that dropped or duplicated a row
  disagrees with it.

**Seven mutations. Four caught, three survivors, and none of the three is a
missing test** — which is a claim that needs its own justification, because the
usual reading of a survivor is a gap:

- *Keys not deduplicated.* The saving is invisible in the answer: grouping is by
  the row's own value, so a repeated candidate creates no repeated group. It was
  extracted into `distinct_keys` and unit-tested there — fifty parents over two
  libraries collapse to two candidates, sorted, and empty stays empty. Making
  the saving observable was the fix; testing the answer never could be.
- *The empty-key guard removed.* Same class. Skipping the read when there are no
  parents changes no answer, only whether an `IN ()` scan is paid for. Covered
  at the helper level by `no_keys_is_no_candidates`; the guard itself is a
  performance decision whose failure mode is a wasted scan.
- *`authorized_table` swapped for the plain lookup.* Correctly unobservable. The
  kernel authorises again inside the planner and, as the comment on
  `authorized_table` already said, that is the check that actually protects the
  rows; the handler's is defence in depth. `test_a_caller_without_the_grant_is_refused`
  passes either way and should — the caller is denied, from one layer or the
  other. Written here rather than resolved, because resolving it would mean
  either deleting a deliberate redundancy or asserting on which layer refused,
  and the client cannot see that.

Chasing the second survivor found a real wart and fixed it: the empty path
returned `served_by: in_transaction()` even outside a transaction, reporting a
read that did not happen to a caller who may be tracking a watermark from it. It
is unset now.

`cargo fmt --all -- --check` clean, `cargo clippy --workspace --all-targets`
clean, Python stubs regenerated with the pinned `grpcio-tools`.

## What this does not do

**Go and TypeScript do not have it.** So the three-SDK conformance runner does
not cover it either, which is the assertion the plan named as the one that
matters — three clients returning byte-identical answers. Until those land, this
is one client's feature and the divergence risk the runner exists to catch is
unguarded.

**One relationship per call, one level deep.** No nesting; that is P5.

**No composite relating key**, beyond the tenant case above.

**The Rust `Related` trait and the wire disagree about what a relationship is.**
The trait takes any two ordinals; the wire needs a foreign key. A relationship
expressible in Rust may not be expressible from a client, and nothing detects
that — it shows up as an `InvalidArgument` naming the keys that do exist.
