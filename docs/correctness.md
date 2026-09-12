# How this is known to be right

Performance claims come with numbers. Correctness claims usually come with a
test count, which measures effort rather than coverage. This note is about the
handful of tests that carry real weight, why each is shaped the way it is, and
what they found.

## The problem with a passing test suite

Every bug found here so far was found by something *other* than the test that
should have caught it:

- The silent `ORDER BY` bug — a sort column that was never decoded, so every
  row compared equal and the sort did nothing — was found by running ClickBench,
  not by the sorting tests.
- The codec accepting a vector on encode and refusing it on decode was found by
  a new integration test, not by the codec's own property suite, because the
  property generator had never been extended to produce vectors.
- A 50% performance regression across every query was nearly shipped because
  two microbenchmarks came back identical; they measured the wrong path.

The common thread is that all three tests were written by whoever wrote the
code, and therefore probed the cases that were already in mind. The tests below
are built to not have that property.

## The planner oracle

`crates/slate-kernel/tests/oracle.rs`

A query optimiser has exactly one obligation: to be invisible. If two plans for
the same query return different rows, the optimiser is not choosing between
plans, it is choosing between answers — and it does so silently, returning
plausible rows in a plausible order.

So the oracle generates random queries and requires every access path to agree.
Three properties, in decreasing order of how much they prove:

1. **Every access path returns the same rows.** The same query is run under the
   planner's own choice, under a forced table scan, and under each index in
   turn. Nothing is assumed about what the right answer is — only that there is
   one. This needs no oracle in the usual sense, which is what makes it strong.
2. **Scan bounds never lose a row.** The rows a plan returns must be the rows
   the predicate admits. Bound derivation (in the planner) and row filtering
   (in `Expr::admits`) are separate code reaching for the same set, so a
   disagreement is real even though both are ours.
3. **Sorting and paging agree with the obvious implementation.** With a total
   order, `ORDER BY`/`LIMIT`/`OFFSET` must match sorting the rows and slicing
   them. The comparator is written out again in the test rather than shared, so
   the two are independent statements of the same rule.

Every generated sort has the primary key appended, which matters more than it
looks: without it, `ORDER BY size LIMIT 5` has many correct answers, and two
access paths may each return a different one entirely legitimately. A property
test that does not pin down ties tests the tie-breaking, not the engine.

There is also a test asserting the *generators* work — that most generated
predicates select some but not all of the rows. A property suite whose
predicates all match nothing passes every check and proves nothing.

### What it found, on the first run

**A covering index returned a column nobody asked for.** Reconstructing a row
from an index entry filled in every column the index happened to hold. Asking
for `SELECT id WHERE kind LIKE 'kind-0%'`, a table scan left `size` null and
the `by_kind_size` index returned its value. Same query, two answers, decided
by the optimiser.

**A point get ignored the projection entirely.** An equality on the whole
primary key becomes a point get, which read and decoded the entire row. So
whether a column came back depended on whether the planner reached the row by
key or by index.

Neither returns *wrong rows*, and both would be easy to wave away as harmless
extra data. They are not harmless: they make a row's contents depend on the
plan, which means an application reading a column can get a value or a null
depending on a cost estimate, a statistic, or how many rows happen to be in the
table that day. That is among the worst bug shapes to debug in production.

**Contradictory bounds panicked.** `WHERE id > 28 AND id < 0` is legal SQL that
matches nothing. The bounds derived from it run backwards, and `BTreeMap::range`
panics on a backwards range. Any application building filters dynamically could
bring the process down with an ordinary query.

The fix is in the planner, which already *knows* the range is empty: an empty
range is now `Access::Nothing`, which is both correct and the cheapest possible
plan. `MemoryStore` also guards against a backwards range, on the principle
that a store which panics on caller input is one bug away from a denial of
service regardless of who is at fault.

All three are pinned as named tests as well as being reachable by the
generator, because a property test only rediscovers a bug if it happens to
generate that shape again.

## The row-level-security matrix

`crates/slate-kernel/tests/rls_matrix.rs`

"Security is in the kernel" is a claim about *every* way a row can leave the
store, and it is only as strong as the weakest path. A policy honoured by the
table scan and skipped by the top-N heap or the k-NN search is not a partial
success — and the paths that skip it are exactly the ones added last and tested
least.

So it is a matrix, not a set of scenarios. Every single-table read path appears
in one list with the same question asked of it: run as a user whose policy
admits two of five rows, does it return those two and nothing else? Currently
twelve paths — table scan, secondary index, descending scan, point get, point
gets from `IN`, the bounded top-N heap, computed columns, `LIKE`, `ILIKE`,
regular expressions, nearest-neighbour search, and a covering projection.

The three rows the policy hides are chosen to break a different shortcut each:
another user's row in the same tenant, a row with a **null** owner (which a
negated or coerced comparison lets through), and the same principal's id in a
different tenant.

Two details worth copying:

- The vector fixture puts the query point nearest the rows the caller may
  **not** see, so a search that ranked before filtering would return them
  first rather than by luck not at all.
- The matrix collects every failure before asserting, rather than stopping at
  the first. When a change breaks the policy it usually breaks it on more than
  one path, and a run that reports one cell hides how far it spread.

Aggregates get their own tests, because an aggregate is the easiest place to
leak without returning a single forbidden row: `COUNT(*)` over rows the caller
cannot read still tells them how many there are. So does a `GROUP BY` key —
a group for another user's id discloses that they have rows even with the rows
withheld. `HAVING` and `analyze` are checked for the same reason.

Joins and chains keep their policy tests next to the rest of their behaviour,
in `join.rs` and `chain.rs`.

### Proving the matrix can fail

A security test that has never failed is a security test that might assert
nothing. Running the whole matrix as a superuser — who has no policy — must
fail every cell, and does, each with a message naming the path and the leaked
row. That check was run once by hand; it is not kept, because a test that
asserts the store leaks when asked to is a strange thing to leave lying around.

## Restart and durability

`crates/slate-slatedb/tests/restart.rs`

Every other test in that crate builds a store, uses it, and drops it — leaving
the most basic claim a database makes entirely unexercised. Worse, it leaves
*index* durability unexercised, and an index that survives a restart in a
different state from its table is not a lost row, it is a wrong answer returned
confidently.

A restart here means closing the `SlateStore` and opening a new one over the
same object store: memtable, block cache, catalog, cursors all gone; the object
store carries over. What is checked:

- Committed rows come back unchanged.
- A unique index still *refuses a duplicate* — the constraint, not just that
  the entries decode.
- An index scan and a table scan still agree, which is how a divergence between
  the two would surface.
- A transaction that was never committed leaves nothing behind, including no
  index entry pointing at a row that does not exist.
- A delete stays deleted, and frees its unique-index slot. Tombstones are
  exactly the thing a restart can lose.
- Tenant isolation holds, since it is a property of the stored keys.
- Writes made *after* a restart survive the next one.
- A covering scan agrees with the table after an indexed value was updated.

And a control: a store opened over a *different* object store sees nothing.
Without it, all of the above could pass because data reached the second store
through some ambient process state, which would make "survives a restart" mean
nothing at all.

## What is still not proven

Stated plainly, because a document like this is otherwise an advertisement:

- **Crash mid-write is not tested.** The uncommitted-transaction case is the
  deterministic half. Killing a process between the row write and the index
  write, and asserting recovery, needs fault injection that does not exist here.
- **Concurrency is barely tested.** One test has two writers contend for a
  unique index slot. There is no interleaved-writer stress, no reader pool under
  load, and no fencing exercised during a real handover — which makes the
  writer-fencing and conflict-retry logic simultaneously the least-exercised
  code and among the most consequential.
- **Untrusted bytes are not fuzzed.** `decode` parses whatever storage returns,
  and patterns are caller input. A fuzz target asserting decode never panics
  and never hangs is roughly a day's work and has not been done.
- **The restart tests run over an in-memory object store**, not the S3 path.
  The abstraction is the same, so the logic is covered, but the S3 protocol
  path is not exercised across a restart.
- **Correlated columns.** Selectivities multiply, which assumes independence.
  That makes estimates wrong on correlated data, and nothing currently measures
  how wrong.
