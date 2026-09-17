# A path of relationships on the wire

- **Date:** 2026-09-17
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `crates/slate-server/proto/slate/v1/records.proto` (+ the TypeScript copy), `crates/slate-server/src/{service,session}.rs`, `crates/slate-server/tests/related_path.rs` (new), `crates/slate-orm/src/relation.rs` (doc), all three clients and their suites, the three explorer adapters, `examples/explorer/{head.toml,conformance/conformance.py}`, `clients/python/testserver`, `docs/orm-comparison.md`, `site/docs/roadmap.html`, `ledger/README.md`
- **Kind:** feature

## What changed

`RelatedRequest` grew `repeated RelatedStep path`; `RelatedResponse` grew
`repeated Level levels`. One request walks a path of relationships, one read
per level, and the answer comes back flat with a back-reference per level.

Three clients grew `related_path` (the tree, every level kept) and
`related_through` (the far rows, middles dropped). `Limits::max_relation_depth`
bounds the depth, default four. 13 server tests, 6 Python, 5 Go, 5 TypeScript,
3 conformance cases — 83, and the three SDKs agree on all of them.

## Why

`slate-orm` has had `load_related_through`, `load_through` and `load_nested`
since P4, and all three were reachable only from Rust. A Python, Go or
TypeScript caller wanting an article's tags issued two `Related` calls and
regrouped by hand — the same shape as the N+1 this whole layer exists to
prevent, one level up. It was the third capability to stop at the Rust edge
after `Related` itself and predicate writes, and the only one still open.

## The response shape, which was the design

The request is a path; the answer is one `Level` per step, each grouped by the
value that related its rows — and each carrying **`key_ordinal`**: where, in
the *previous* level's rows, that value is found.

That one number is what makes the flat shape work. A client takes a row from
the level above, reads the value at `key_ordinal`, and looks it up in the level
below. No catalog, no schema knowledge, no recursion in the message type, and
the same three lines in all three SDKs rather than three different guesses.

## Alternatives rejected

**A second RPC for nesting.** Rejected in the plan and still rejected: the
grouping contract `RelatedResponse` already specifies would have to be
duplicated, and two messages meaning nearly the same thing is how they drift.

**A recursive `Level` holding `Level`s.** This is the shape the answer actually
has, and it makes every client write a recursive decoder for a structure whose
depth the request already knows. Flat with a back-reference carries the same
tree as a loop instead of a recursion, in three languages.

**Let the client work out which column supplies the next key.** It can: the
relation names a table and a foreign key, and the direction says which side.
But then three clients each reimplement a piece of catalog resolution that the
server has already done, and a client that got it wrong would produce a
plausible answer rather than an error. The server knows; it says so. A mutation
that ignored `key_ordinal` is caught in all three suites.

**Keep `relation` and `path` as two server-side code paths.** Rejected as the
two-paths-to-keep-in-agreement shape this repository keeps rejecting.
Internally there is only ever a path: a `relation` request is normalised to a
path of one before anything resolves, and only the *projection* at the edge
differs. A test asserts the two shapes give the same rows.

**Resolve `relation` when both fields are set.** Refused instead. Two ways to
say what to read, saying different things, is a client bug; picking one
silently is how it reaches production.

**One read view per level.** Rejected for correctness, not cost: two levels
read from two snapshots can show a child whose parent was deleted between them
— a tree that never existed. One view spans the path, with affinity computed
over every table it touches.

**No depth limit, as `load_nested` argues for itself.** That argument is
correct *for that function* — its depth is a type parameter, fixed when it
compiles, and "a limit nobody can exceed is a limit nobody maintains". The same
doc comment then names the form that would need one: a list on the wire whose
depth a request chooses. This is that form. One step is one read, so an
unbounded path is a caller deciding how many times the server goes to storage.
The comment has been updated to point at the limit it predicted.

**Add foreign keys to the existing `authors`/`books` fixtures.** Refused, on
the advice of the comment already sitting beside them: six test files write
books without writing an author, so the constraint would be tested by breaking
tests about something else. Each suite got a *new* table instead.

## Evidence

21 `slate-server` test binaries, 65 across `slate-kernel` and `slate-orm`,
229 Python, the Go suite, 122 TypeScript. `cargo clippy --workspace
--all-targets` clean under `-D warnings`; `cargo fmt --all -- --check` clean.
**83 conformance cases: the three SDKs agree on all of them.** Demo browser
e2e 18/18. Quickstarts, `site/check/docs.py`, and the deployed stack over real
object storage all green.

**Thirteen mutations, three survivors, all now killed.**

| mutation | result |
| --- | --- |
| the depth limit is off by one | killed |
| the depth limit never fires | killed |
| the path's composition is unchecked | killed |
| `key_ordinal` reports the wrong column | killed |
| a no-key request drops its empty levels | killed |
| both `relation` and `path` resolved by precedence | killed |
| only the first step is authorized | **survived** → killed |
| Python ignores `key_ordinal` | killed |
| Python's far rows are "nodes with no children" | **survived** → killed |
| Go ignores `key_ordinal` | **survived** → killed |
| Go's far rows are "nodes with no children" | killed |
| TypeScript ignores `key_ordinal` | killed |
| the node adapter stops reporting the path | killed (conformance) |

**Lazy authorization.** A mutation authorizing only the first step still
refused the test that asks a half-privileged role to walk the whole path — the
kernel refuses the read too, just later, so the status is identical. What
separates them is a path whose *first* level is empty: resolved lazily, the
second step is never reached and the request **succeeds**. The test that pins
this uses the one parent with nothing related, and it is the only one of the
thirteen that catches the mutation.

**"Nodes with no children" is not "the rows at the bottom".** `related_through`
was written the first way. A middle row that related to nothing has no children
either, so a shelf came back where a copy was asked for. Found by a fixture
with a deliberately bare middle row — the two readings agree on every other
input — and the same fixture now exists in all three suites. `slate-orm`'s
`load_related_through` gets this right by construction, flattening
`(_join, far)` pairs, which is what the corrected version restates as a depth.

**A fixture where the right ordinal is zero proves nothing.** A mutation
ignoring `key_ordinal` and hardcoding `0` passed the entire Go suite, because
that fixture's relating column *was* ordinal 0. Only Python caught it, and only
because its tables are tenant-scoped so `id` sits at ordinal 1 by accident. The
Go and TypeScript fixtures now put a column in front of `id` on purpose, and
the mutation dies in all three.

**A lockfile left behind by the previous commit.** Declaring `protobufjs` as a
direct dependency of the TypeScript client (the entry before this one) updated
that package's own lockfile and none of the two that link it by `file:` —
`examples/deployed/node` and `examples/explorer/backends/node`. CI runs
`npm ci` in the second, which validates the lockfile against what it resolves,
so that job was going to fail on a dependency added a commit earlier. Found
because running the deployed suite refreshed one of the two lockfiles on disk
and it showed up as an unexplained staged change. Both are refreshed here, and
both were verified by deleting `node_modules` and running `npm ci` for real
rather than by trusting `--dry-run`, which reported "up to date" on exactly
this class of mismatch last time.

**Two more real bugs, both found by tooling rather than review.** A
`[[tables.foreign_keys]]` attaches to the nearest `[[tables]]` above it, so
adding the demo's `editions` table above the existing block silently moved
`sale_book` off `sales` — six unrelated conformance cases started refusing on
the next run. And the prebuilt-binary staleness guard refused a stale
`slate-serverd` twice, exactly as it was built to.

## What this does not do

**No predicate, ordering or limit per level.** All three are real and all three
want a level to be a `Query` rather than a `Relation`, which is a much larger
message. The item said it stops here and it stops here.

**The depth is bounded; the fan-out is not.** Four steps is four reads, and the
fourth can return a million rows. `max_returned_rows` does not apply — it caps
a predicate write's answer, not a read's — so a path's response size is bounded
by nothing. That is the same gap `Related` already had, one level deeper, and
it is the next thing to look at if this is put under load.

**Nothing measures it.** One request instead of two is obviously fewer round
trips, and that is arithmetic rather than a measurement. The read-count claim
*is* asserted — a whole path is one request, counted at the stub — but no
number here says what it costs against the two-call version.

**`ErrorInfo.metadata` still is not surfaced**, so a refused path reports
`max_relation_depth` in prose and `INVALID_ARGUMENT` as a token. The limit is
in the message because a caller needs to read it; that is the string-matching
this repository otherwise avoids, and it is the reason the metadata map is
worth surfacing eventually.
