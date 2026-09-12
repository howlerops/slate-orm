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

## Failure partway through a write

`crates/slate-kernel/tests/crash.rs`

A row and its index entries are written together. If a write can fail between
them and leave the result visible, the store has an index entry pointing at a
row that does not exist — a lookup by index then returns a row that was never
there, or returns nothing while a scan returns it. Nothing reports it.

The layer's claim is that this never happens, and nothing tested the claim
because nothing could make a write fail. A `Faulty` store now can: it wraps a
store and fails the *n*th write of a transaction. Running the same workload
with the fault at every position in turn covers each point a real failure could
land, and after each one the store must satisfy a consistency check — every
index entry resolves to a row that exists with the values the entry claims, and
every row appears in every index exactly once.

An insert costs four writes (row, two index entries, commit), counted rather
than assumed so the sweep still covers everything if the write path gains a
step. Inserts, updates, deletes and bulk inserts are each swept. The update
case is the sharpest: a failure between retiring the old index entry and
writing the new one is where a row ends up reachable through its old indexed
value, its new one, both, or neither — so the check is that the row is *wholly*
old or *wholly* new, never a mixture.

The consistency check is itself proven to fail: one test writes a dangling
index entry behind the record layer's back and requires the check to notice and
to name the index.

What this does not cover is a crash *between* SlateDB's own writes. That is
SlateDB's atomicity to keep. This covers the layer that decides what goes into
a transaction together.

## Many writers and readers

`crates/slate-kernel/tests/concurrency.rs`

One test elsewhere had two writers race for a unique index slot. That
established conflicts are detected; it said nothing about behaviour under real
contention.

Counting is the workload, deliberately: a counter incremented N times by C
tasks must end at exactly N×C. A lost update shows up as a number too small and
a double-apply as one too large, and a single integer is hard to satisfy by
accident. Eight tasks × twenty-five increments on one row come out exact, and
the test additionally asserts that **some transaction actually conflicted** —
without that, tasks that never overlapped would pass while proving nothing.

Also checked: exactly one of sixteen racing writers takes a unique slot (not
"one wins" with two writers, which a check-then-write race would also pass);
writers on *disjoint* rows never conflict, since a conflict check too coarse
would be correct and useless; concurrent deletes leave the row gone and its
unique slot free; and a reader running alongside writers compares a table scan
against an index scan in the same transaction, so a write becoming visible in
two steps would show up as the two paths disagreeing.

Run fifteen times over, no flakes.

## Untrusted input

`crates/slate-tuple/tests/untrusted.rs`, `crates/slate-kernel/tests/untrusted_patterns.rs`

Two boundaries take input nobody here wrote: the decoder reads whatever storage
returns, and patterns arrive from whoever wrote the query — which in an
application means whoever filled in a search box.

The decoder's contract is deliberately weak, because a weak contract is one
that can hold: **for any input, decoding returns `Ok` or `Err`**. It must not
panic, run away, or read out of bounds. Nothing is claimed about what it
decodes from nonsense. Bytes are generated biased towards the ones that mean
something to the codec — type tags, the NUL that terminates a string, the 0xFF
that escapes it — because uniform random bytes fail on the first tag and never
reach the interesting code. Also swept: every truncation of a valid encoding,
and a single corrupted byte at every position.

One case is called out on its own. A vector's encoding carries a length, and a
length prefix is a promise the buffer does not have to keep: five bytes
claiming four billion elements must be an error, not an allocation.

For patterns, the properties are that matching terminates, does not overflow
the stack, and that a pattern turned into scan bounds still finds every
matching row. The last is checked as a property against the matcher — if a
value matches the pattern, its encoding must fall inside the derived range —
because the cases that break it are escapes and multi-byte characters at the
boundary, which is to say the ones nobody writes down.

Measuring the regex limits was worth doing rather than assuming. A first
attempt used `"a{1000}".repeat(100)` as an "oversized" pattern; it compiles
fine. The shape that actually matters is `(a{1000}){1000}` — **fifteen
characters** asking for a million-state machine — and what stops it is the
crate's 10 MiB compiled-size limit, not any length check that could be put on
the pattern string. That limit is a property of the engine rather than of this
code, so it is pinned here where swapping the engine would fail.

## Correlated columns: wrong, and so far harmless

`crates/slate-kernel/examples/correlation.rs`

Selectivities multiply, which assumes independence. Real data is full of
columns that are not independent, so the estimates are wrong by construction.
The question worth answering is not whether they are wrong but **how wrong, and
whether it changes the plan** — an estimate ten times too small that still
picks the same access path costs nothing.

Two columns over the same domain, with `b == a` at a tuneable probability:

| correlation | predicate | estimate | actual | error | plan penalty |
|---:|---|---:|---:|---:|---:|
| 0.00 | `a=1 AND b=1` | 50 | 48 | 1.04x | 1.05x |
| 0.50 | `a=1 AND b=1` | 50 | 534 | 0.09x | 1.00x |
| 0.90 | `a=1 AND b=1` | 50 | 901 | 0.06x | 1.00x |
| 1.00 | `a=1 AND b=1` | 50 | 984 | **0.05x** | 1.00x |
| 0.90 | `a=1 AND b=2` | 50 | 6 | 8.33x | 1.00x |
| 1.00 | `a=1 AND b=2` | 50 | 0 | ∞ | 1.03x |

The estimate is off by up to **twenty times**, and unboundedly in the other
direction where correlation makes a conjunction impossible. The plan penalty —
what the chosen plan really costs against the cheapest available, both computed
from measured row counts through the same cost model — stays at **1.00x**.

The reason is worth stating, because it says where the risk actually is: plan
choice is driven by the *bound* each access path derives, which is a
single-column selectivity estimated from single-column statistics and therefore
right. Independence error lands on the estimate of the final row count, after
the residual, and lands on every candidate roughly equally — so it moves the
numbers without moving the ranking.

`nested_loop_cost` multiplies the outer side's estimated row count by the cost
of one probe, which looked like the one place the error has a direct lever on a
decision. Measured, it does not fire: with a correlated filter on the outer
side and its estimate twenty times too small, the planner still chooses a hash
join at every correlation and both domain sizes. That prediction was wrong, and
saying so is cheaper than hunting for a configuration that would have confirmed
it.

**Conclusion: multi-column statistics are not warranted on this evidence.** The
shapes measured are single-table access-path choice and one join shape (a large
table against a small lookup). A join between two large sides, each with a
correlated filter, is not measured and is where to look first if this ever does
bite.

## What is still not proven

Stated plainly, because a document like this is otherwise an advertisement.
Everything that was on this list a round ago has moved above it; what remains is
what genuinely has not been done.

- **A real crash, as opposed to an injected write failure.** `crash.rs` proves
  the record layer never puts a row and its index entries in separate
  transactions. It does not kill a process mid-`fsync` and restart it — that
  boundary belongs to SlateDB, and taking it seriously means fault injection
  inside the storage engine rather than above it.
- **Writer fencing during a live handover.** Fencing is tested against a real
  instance, but as a sequence: writer A, then writer B, then A discovers it is
  fenced. Two writers genuinely overlapping across a lease change, with
  in-flight transactions on both, is not exercised.
- **The reader pool under load.** Routing is tested on its properties and
  replicas are tested for correctness, but nothing runs a pool hot enough for
  lag, eviction and affinity to interact.
- **A real fuzzer.** The untrusted-input suites are property tests with hostile
  generators, which is most of the value for a few seconds per run. They are not
  coverage-guided, so they will not find the input that needs eleven specific
  bytes in a row.
- **Correlated columns in a join between two large sides.** Measured and found
  harmless everywhere it was measured; this is the shape where the mechanism
  could still bite, and it is the first place to look if it ever does.
- **Scale.** ClickBench runs a million rows in memory. Nothing large has been
  run against real object storage, so the cost model's 2.2 ms round trip is
  still a fixture constant rather than a measurement at size.
