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
accident. Eight tasks × twenty-five increments on one row come out exact.

The harder half is proving the writers *contended* at all, rather than running
one after another and passing vacuously. That is settled by construction, not
by observation: a first phase holds all eight tasks at a barrier after each has
read the counter and before any of them writes, so all eight hold the same
stale read when they are released. Exactly one may commit; the other seven must
be refused, and refused **as conflicts** — a store that failed them for some
unrelated reason would satisfy a bare count while proving nothing. Moving the
write into a fresh transaction after the barrier, so the read no longer pins a
serialisation point, lets five of the eight through and the test says so.

Also checked: exactly one of sixteen racing writers takes a unique slot (not
"one wins" with two writers, which a check-then-write race would also pass);
writers on *disjoint* rows never conflict, since a conflict check too coarse
would be correct and useless; concurrent deletes leave the row gone and its
unique slot free; and a reader running alongside writers compares a table scan
against an index scan in the same transaction, so a write becoming visible in
two steps would show up as the two paths disagreeing.

Run sixty times over at ten binaries in parallel on four cores, and ten
full-suite runs. No flakes — see "When the tests are the flaky thing" for the
one there used to be.

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

## Joins, chains and aggregates

`crates/slate-kernel/tests/join_oracle.rs`, `aggregate_oracle.rs`

The single-table oracle above was soaked at 25,000 cases and found nothing —
which is the useful result, because it says the *generators* were the limit
rather than the case count. More of the same shapes finds nothing; different
shapes is the only way forward.

**Joins.** Four join types against hash-build-left, hash-build-right and
nested-loop, all required to agree, and all required to match a nested loop
written out over the rows. Rows are compared as multisets: a join has no
inherent order and the three algorithms genuinely produce different ones, so
demanding an order would test the implementation instead of the semantics.

It surfaced something on the first run that turned out to be correct: a nested
loop cannot serve a right or full outer join, because it streams the left side
and never sees a right row that matched nothing. The engine refuses with a
clear error rather than returning the inner-join rows and a short answer. The
refusal is now pinned by name, because a later change that "supports" the
combination by quietly falling back to a hash join would pass every other test
in the file.

**Chains.** A three-table chain must match a hand-written triple loop, and a
one-step chain must match the equivalent two-way join — the two paths exist for
different reasons and must not have drifted apart.

**Aggregates.** Grouping is checked against a fold written out over the rows,
across every aggregate including `COUNT(DISTINCT)`, and separately required to
be identical down every access path. That second property is the important one:
an index path that left a grouping column undecoded would group everything
under null and return a plausible wrong answer, which is precisely the `ORDER
BY` bug wearing different clothes. The fold's `match` is deliberately
exhaustive, so adding an `Aggregate` variant fails to compile here rather than
going untested.

## Harder schemas

`crates/slate-kernel/tests/oracle_schemas.rs`

Every oracle above uses a single-column `u64` primary key, which is the easy
case: one value, one direction, no prefix. The load-bearing keyspace code is in
the shapes that were not covered — a tenant-scoped table where every key
carries a prefix, a three-column primary key where a predicate can pin part of
it, and a composite index with mixed ascending and descending columns, where a
bound for the descending column has to be built inverted.

The same differential property, over that schema, plus one specific to it:
no access path, on any index, in either direction, may return another tenant's
row. Twelve thousand cases, nothing found.

## Writer handover

`crates/slate-slatedb/tests/handover.rs`

Fencing was tested as a sequence — commit, take over, commit again, observe the
error. The cases that decide whether a head node can be replaced safely are
either side of that: a transaction opened *before* the takeover (fenced at
commit), whether fencing is terminal across repeated attempts (it is), whether
the new writer inherits the old one's unique index entries (it does), and a
chain of four handovers where each takeover must fence every earlier generation.

**One finding, and it changes an operational recommendation.** A fenced writer
cannot read either: `begin` returns `WriterFenced` before a transaction exists.
This was written expecting the opposite, on the reasoning that a head node
stepping down would want to drain its in-flight queries. It cannot. That is
defensible — a fenced writer's view is arbitrarily stale and it has no way to
say how stale — but it means a takeover is an interruption, not a graceful
drain, and anything that must keep serving *through* a handover has to be
reading from a replica. `docs/topology.md` now says so.

## Replicas under load

`crates/slate-slatedb/tests/replica.rs`

Reads through the pool while the writer commits. The property is not that a
reader sees the newest data — a replica lags by design — but that whatever it
sees is a state the database was actually in. Row ids are written in order, so
the ids a replica returns must be a contiguous prefix from zero: any gap means
it served a later write without an earlier one. Twelve concurrent readers,
half routed by tenant affinity and half round-robin, each waiting on a commit
token, all see the full result.

## When the tests are the flaky thing

Four of the generator-quality checks above — the ones asserting that generated
predicates are not all-or-nothing — were written with thresholds at 50% against
observed rates of 51–75%. One of them duly failed on an unlucky sample.

That is worth recording rather than quietly fixing, because it is the same
mistake in a new place: a threshold picked by eye rather than measured. The
rates were measured across repeated runs, sample sizes raised to 400, and the
bars set at 30–40% — five or more standard errors clear. A check that guards
the generators must not be the flakiest thing in the suite, since a test that
fails one run in twenty teaches people to rerun rather than to look.

Then a fifth, of the same shape. The lost-update test guarded against a vacuous
run by asserting that the retry counter ended above zero — "if nothing ever
retried, the tasks never overlapped". On an unloaded machine that holds. Under
the full crate suite, with twenty test binaries competing for four cores, the
eight tasks can be scheduled one after another, nothing conflicts, and a
*correct* store fails the test. It did, one full-suite run in six.

The lesson is narrower than "avoid timing in tests" and worth stating exactly:
a vacuity guard must be as deterministic as the property it guards. Asserting
that contention *happened* is an observation about the scheduler; arranging for
contention to be *unavoidable*, with a barrier, and then asserting what the
store must do about it, is a property. The rewrite survives sixty runs at ten
binaries in parallel on four cores, and ten full-suite runs, with no failures,
where the old one failed reliably under the second condition.

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

## A crash underneath the storage engine

`crates/slate-slatedb/tests/storage_crash.rs`

`crash.rs` fails writes at the transaction boundary — the layer this project
owns. This fails the *object store*, so SlateDB is interrupted midway through
its own SSTs and manifest, which is what a killed process leaves behind. The
claim is deliberately weak because it is the only one that can hold: committed
data may be lost, but what survives must be coherent — no index entry pointing
at a row that is not there, no unique slot occupied by nothing.

Fault budgets are chosen against a **measured** figure (40 row writes cost 43
object-store writes) rather than a guess, because a budget above the real count
never fires and a case whose fault never fires tests nothing.

### SlateDB hangs when the object store refuses writes

Found because this file hung instead of failing. Driving SlateDB directly, with
no record-layer code in the path, two hundred puts and a flush do not return —
ever. A full disk, a revoked credential or a changed bucket policy therefore
produces no error at all: the writer stops while still looking alive, which is
the state an operator finds last.

It cannot be fixed from here. What this layer can do is refuse to pass an
unbounded wait to its callers, so `SlateStore::with_commit_timeout` turns it
into `CommitTimedOut` — deliberately **not** retryable, because a timed-out
commit may already have landed. The test fails if a future SlateDB returns an
error instead, which is the right moment to reconsider the timeout.

## Two writers at once

`handover.rs` gained a minimal lease — a monotonic generation, and a rule that
only the newest holder may write — so "two writers overlap" can be written down
as a test. Both push concurrently across the takeover, and the store, not the
lease, is the backstop: nothing the old writer commits after being fenced is
visible, and the index still agrees with the table afterwards.

## The cost model was calibrated against itself

`crates/slate-slatedb/examples/cost_calibration.rs`

Every performance number in this project came from multiplying the planner's
cost by 2.2 ms — a figure from a *latency fixture*, not from storage. Measuring
against an S3 server at 200,000 rows found the model wrong in both directions
at once:

| | predicted | actual GETs |
|---|---:|---:|
| full scan, 200,000 rows | 2001 | 25 |
| 400 rows via index | 30 | 1217 |

Scans were overcharged about **eighty times** (`SCAN_ROW_COST` assumed 100 rows
per request; readahead delivers ~8,000). Index lookups were undercharged about
**forty times**: a point read costs ~3 requests rather than 1, and
`pipelined_read_cost` divided by concurrency depth — which is true of latency
and false of work, and the fixture could only ever measure latency.

The errors compounded in the same direction, and the planner acted on them. For
`WHERE bucket = 7` it chose an index scan at cost 30 over a table scan at cost
2001, and the plan it chose did **58x the object-store requests and took 9x
longer** — 1,217 requests and 3.6 s against 21 and 0.4 s.

Recalibrated from the measurements, the model now predicts 26 against 21 actual
requests for the scan and 1201 against 1223 for the index, and picks the scan.
ClickBench is unchanged at 70.1 s.

### What that changes about indexes

The consequence is counter-intuitive enough to state plainly: **an index earns
its keep on absolute rows fetched, not on percentage selectivity.** Fetching
`k` rows beats scanning `n` only when `n > 24000k`, so `k = 0.005n` never
qualifies at any table size. Seven tests asserted the old rule — that a few
percent was selective enough — and each was rewritten against the measurement
rather than nudged: a nested loop now wins at a hundred million inner rows
rather than a hundred thousand, and a small table is simply cheaper to scan
whole. Covering indexes are unaffected, since they do no point reads at all.

## The one optimisation that can return a wrong answer

`crates/slate-kernel/tests/partial_indexes.rs`, `crates/slate-kernel/tests/index_in.rs`

Every other choice in the planner picks between paths over the same rows, with
the residual predicate deciding what comes back; get one wrong and a query is
slow. A **partial** index is different. It holds entries only for the rows its
own predicate admits, so choosing it does not narrow a scan — it changes which
rows exist to be found. Use one for a query that reaches outside it and the rows
outside are not filtered out, they are never seen: no error, no empty result,
just fewer rows than were asked for, on whichever queries happened to flip.

So the gate is an implication check, `plan::implies`, and it is deliberately
one-sided: an index carrying a predicate is a candidate only when the query's
predicate can be *shown* to land inside it, and "cannot show it" means the index
is not used. It answers `true` for syntactic equality, for a conjunction where
any one conjunct suffices, for a disjunction where every branch lands inside,
for a bound against a looser bound in the same direction, for an equality or an
`IN` whose every value meets the goal, and for `IS NOT NULL` from any comparison
or pattern match on the column — three-valued logic, since all of those are
*unknown* rather than true on a null. It answers `false` for everything else,
including implications a human can see: `x >= 5 AND x <= 5` does not imply
`x = 5` here, because that needs two conjuncts at once. A missed implication
costs a table scan. The asymmetry is the whole design.

Two things make that reviewable rather than asserted. Comparisons between
literals go through `Value`'s own ordering — the same total order
`Expr::evaluate` compares with — so the planner and the evaluator are not two
opinions about what `<` means. And a proptest generates pairs of expressions,
and wherever `implies` says yes, checks every row of a corpus: a row the query
admits and the index does not hold is a refutation. It cannot prove the general
claim, but a wrong rule shows up as a concrete row. A second test counts how
often the generators produce an implication at all — 200 of 2,048 pairs,
measured, against a bar of 5% — because a property test that mostly skips is
the failure mode the four flaky thresholds above were about.

The test file is lopsided on purpose: one case showing a partial index being
used, and a table of cases showing it not being used. Including one that is
easy to get wrong — an `AccessHint` naming a partial index does **not** force
it. A hint is advice about which of several correct plans to take, never
permission to read an index that does not hold the rows asked for.

`IN` over a secondary index is the same feature's easy half. It becomes one
range per value rather than one range spanning them all, which is what the
planner used to produce and which on a column worth indexing is most of the
index. Nothing there can lose a row, so the tests are about the two ways it
could still go wrong: reading the wrong entries (an oracle against a full scan,
plus a count of index operations against a `LatencyStore`) and reading them in
the wrong order (the ranges are held in ascending *key* order and concatenated,
which is what lets an `ORDER BY` on the index's own columns still stream; a
descending walk reverses the list, and a descending index column is covered
separately because sorting encoded bounds rather than literals is what makes
that fall out).

### Maintaining one, and why the tests read raw keys

The other half is the write path, and it is now there: an insert the predicate
rejects writes no entry, an update that stops matching deletes one, an update
that starts matching writes one, and a delete of a row the index does not hold
touches nothing. The predicate is declared on `IndexDef` and both halves read
the same one — the writer runs it through the `slate_schema::Predicate` seam a
`CHECK` already uses, the planner downcasts it back to an `Expr` to reason
about. Two declarations of one predicate would put a silent wrong-answer bug one
edit away.

Those tests compare **raw keys**, not query results, and the reason is worth
stating because it is not obvious. A *missing* entry shows up in a query: rows
go absent. A *spurious* one never can. An index scan still evaluates the
residual predicate on every row it fetches, and a row that should not be in a
partial index is by definition one the predicate rejects — so the residual
throws it away and the answer looks right, while the index quietly holds
entries it should not. The only way to see it is to look at the keys. A proptest
runs a generated sequence of inserts, upserts and deletes and compares the
index's key set against a `Vec<Row>` model of the table filtered by the
predicate.

Two of the bugs the write path had to avoid were only visible on a **unique**
partial index, and both are the same shape. A unique index's key omits the
primary key — that is what makes two rows collide in it — so the entry a
*rejected* row would have had is byte for byte the entry an admitted row really
has. Computing it from the row being replaced or removed, without checking the
predicate first, deletes somebody else's entry. And the bulk path's
"this row already owns its slot, skip the check" shortcut is wrong for a row
that was outside the index a moment ago: its owner never changed, so its slot
looks unchanged, and it writes straight over the row that actually holds it —
inside one transaction, where no write-write conflict can catch it.

Every one of the six places the write path consults the predicate was checked by
removing it and running the suite. Five failed immediately. The sixth — that
bulk shortcut — passed, which is how it was found; the test that now covers it
was written before the guard was believed.

### The other seam: a key the row does not contain

An **expression** index keys on `lower(body)` or `length(url)` — a value no
column holds. That needs a second seam beside `Predicate`:
`slate_schema::Computed` produces a value where `Predicate` produces a verdict,
and the kernel implements it for `Scalar` exactly as it implements `Predicate`
for `Expr`. So the same downcast gives the planner the expression back to match
against what a query computes, and the same asymmetry applies: an index whose
key is not a `Scalar` is maintained correctly and never chosen.

One thing an expression index needs that a partial one does not: the *type* the
expression produces, declared at the schema. Nothing can run the expression
without a row, and the decoder needs the type before it has one. A declaration
is a thing that can be wrong, so it is checked — every write evaluates the
expression and refuses a value whose type is not the declared one. The
alternative is an entry encoded as one type and decoded as another, which
surfaces as a corrupt index at some later scan with nothing pointing back at the
write that caused it. Nulls are exempt, because null is not a type and an index
that refused `lower(null)` would be an index missing rows.

The oracle here is a query rather than a key set, because the failure mode is
the opposite way round from a partial index: nothing can put a *spurious* entry
in an expression index, but the encode and the decode can disagree, and then a
scan silently returns the wrong rows. So the test asks the same question of the
index and of a table scan computing the same expression per row, over equality,
range and `IN`, and requires the two to answer identically. Making
`key_values`, `key_directions` or `index_key_types` ignore the expression each
breaks it.

## The head node, and a lease checked against a wrong one

`crates/slate-server/tests/`

Two of its techniques belong here; `docs/topology.md` has the rest.

The first is the query differential, run over gRPC rather than in process: every
filter × sort × limit/offset in a sweep of 448 queries is executed under the
planner's choice *and* under each index forced, and all of them must agree with
a filter and a comparator written out again in the test. It is the planner
oracle's shape, one layer further out, so a wire conversion that quietly dropped
a predicate term would be caught by the same reasoning that catches a wrong
index — with the filters separately asserted to select some but not all rows,
and every sort ending in the primary key so ties are pinned rather than
arbitrary.

The second is more unusual and worth naming. A lease taken by reading the object
and then writing it — the obvious implementation — passes every single-threaded
test anyone would write for it. So the harness runs against two implementations:
the real compare-and-set lease, and a deliberately naive `OverwritingLease`. The
naive one is asserted to **fail**, at the renewal specifically, and that
assertion is the reason to believe the harness proves anything about the real
one. A test suite that only ever runs against the implementation it was written
alongside cannot tell "this is correct" from "this is what I wrote".

The same idea shows up in the leadership tests as counting: proving that a
fenced node refuses writes *locally* is not a statement about the error the
client sees, it is a statement about the store never being called — so the test
counts calls into it, and five writes after the first fence reach it zero times.

## What is still not proven

Stated plainly, because a document like this is otherwise an advertisement.
Everything that was on this list a round ago has moved above it; what remains is
what genuinely has not been done.

- **A real fuzzer.** The untrusted-input suites are property tests with hostile
  generators, which is most of the value for a few seconds per run. They are not
  coverage-guided, so they will not find the input that needs eleven specific
  bytes in a row.
- **Correlated columns in a join between two large sides.** Measured and found
  harmless everywhere it was measured; this is the shape where the mechanism
  could still bite, and it is the first place to look if it ever does.
- **Scale beyond 200,000 rows on storage.** The cost model was calibrated
  against a real S3 server at 200,000 rows, which is what corrected it; the
  million-row runs are still in memory. Nothing has been measured at a size
  where compaction, tiering and a cold cache all matter at once.
- **Statistics for a computed value.** `analyze` samples rows and builds
  histograms per column; it does not evaluate an expression index's expression,
  so the planner's estimate for one comes from whatever stats the caller
  supplies. The decision is right, the number behind it is a guess.
- **An expression index can never be covering.** Not because the entry holds
  too little — it holds exactly the computed value — but because the executor
  evaluates scalars from a row's own columns, and a row rebuilt from an index
  entry has the source column null. Teaching it to take the value from the
  entry it is already holding is a change to the executor, not to the planner.
- **Partial indexes in the derive macro.** `#[derive(Record)]` cannot declare
  one; the schema builder can. A struct attribute for it is a small piece of
  work that has not been done.
- **Anything about the head node's performance.** Its correctness is tested;
  nothing in it has been benchmarked. The query stream's batch size and the
  lease's fifteen-second term are chosen by argument, not measurement.
