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

### Teaching it about expression indexes

The generator produced neither computed values nor the indexes over them, so
the newest and least-travelled path in the executor — a covering scan that
takes a value out of the index entry rather than computing it from a row — was
proved only by hand-written differentials. Those are the right shape and are
still there; the point of an oracle is to cover the shapes nobody thought of.

It now generates both. The table carries two expression indexes over a
*nullable* column, one producing text and one an integer and one of them
declared **descending**, and the rows are arranged to be awkward for them: the
column is null a quarter of the time, so `lower(null)` is a live entry rather
than a missing one; it is spelled in two cases, so `lower` is not the column
back again; and it comes in two lengths, so `length` is not one value for the
whole table. A fixture where the expression is the identity would let a
covering scan hand back the source column and still agree with everything.

Queries are drawn as a compute list *first* and everything else against it. A
predicate on the third computed value of a query that computes one is not a
hard case, it is a null, and the type each position produces is carried
alongside because `Value`'s order is type-first — an integer compared against a
string answers the same way for every row and tests nothing. Seven compute
lists between them reach: the value an expression index supplies; two computed
values where only one can come out of the entry; a computed value no expression
index has, which an *ordinary* index can still cover because it holds what the
expression reads; a value computed from an **earlier computed value**; and the
index's own value sitting **second**, behind one the primary key can produce.

The weights were measured rather than chosen. A covering scan needs every leaf,
every sort key and every projected column to be something the entry holds, so
the chance of reaching one falls off as a power of the chance that any single
leaf is a table column: at equal weights the generator reached an index-only
scan of an expression index in 3% of cases, which would have left the whole
extension resting on a handful of samples. At four to one it reaches one in
19%, and a generator-quality test holds it to 10% — along with the *control*,
a query reaching for the source column with the same index available, at the
same rate, because if the planner ever called that one covering the two paths
would part company in the differential.

**What it catches that the hand-written tests do not.** Nine mutations were
reintroduced to find out, and most of them — evaluating the computed value from
the rebuilt row instead of reading the entry, letting an expression index claim
it holds the column its expression reads, crediting *every* computed value to
an expression index, filling a row rebuilt from an entry with columns outside
the projection — fail the hand-written differentials as well. That is a good
result for whoever wrote those and a poor demonstration, so it is stated
plainly.

One is not: making the planner ignore an expression index's **declared key
direction** leaves the entire rest of the kernel suite green, including all
forty-eight partial- and expression-index tests, and fails the oracle on the
first run:

```
index IndexId(15) disagreed with a table scan for
  Or([score = 0.0, kind = "kind-0"]) computing [Length(Column(Ordinal(4)))]
```

Nothing hand-written declares a descending expression index, because an
ascending one is what anybody writing a fixture reaches for. A bound for a
descending key has to be built inverted, and getting it backwards produces an
empty range rather than a wrong one — the failure that reads as "no rows
matched".

Soaked afterwards at 20,000 cases on the differential and 4,000 on the other
two, it found nothing. That is the useful result: it says the generators are
the limit rather than the case count.

### Covering a value computed from another computed value

The rule was deliberately conservative in one place, and the note said a second
pass could prove it. It now does. A scalar may read an earlier computed value,
and `SELECT id WHERE lower(title) = 'x' COMPUTING lower(title), length(#0)` is
answerable from an entry keyed on `lower(title)` even though nothing in the
entry holds `title` — the old rule stopped at the first hop, saw a computed
ordinal where it wanted a column, and read every row to recompute a value the
entry had already answered for.

It is a forward pass over the compute list: position `i` is available when the
index keys on it, or when everything it reads is. Iterative rather than
recursive, so a list where each element reads the two before it costs `n` steps
and not `2^n`. Two things make it sound, and both are properties of code
elsewhere rather than of this function:

- the executor's `extend` walks the same slice in the same order, so an earlier
  position really is in place when a later one reads it;
- `covers` demands **every** computed position, not only the projected ones,
  because every computed value is evaluated on every row whether or not
  anything references it. That is what makes the pass safe by construction: a
  chain can only be claimed when its first link can be, and the first link can
  only be claimed when the entry supplies it.

A forward reference — a scalar naming a *later* computed value — is refused
rather than reasoned about. It reads as null on every path alike, so proving it
would be correct and would mean modelling evaluation order in the planner. A
refusal costs a row read.

The honest note on what this is worth: the pass **cannot** produce a wrong
answer that reading the entry wrongly does not already produce, for the reason
above, and the oracle confirms as much — the mutation that removes the pass
leaves the differential green and fails only the reads-counted test that says
the scan is index-only. So the second pass is a performance change with a
correctness argument, not a correctness change, and it is tested as one: a
chained compute list is asserted to plan as `Index Only`, to read **zero** rows,
and to return the same whole rows as a forced table scan, with a control one
link along where the chain reaches `length(body)` and must stop.

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

`analyze` is checked as a *read*, and passes: run under a policy it describes
only the slice that policy admits. That is not what a deployed node does. The
guidance in `record.rs` — analyse as a superuser, or the planner optimises for
a table nobody is querying — means a real node's statistics describe every
tenant, and a histogram bound is a value rather than a count. So the read path
being clean does not make `EXPLAIN` clean, which is why explaining a plan is
its own `Action::Explain` and not something a `read` grant carries. The
demonstration, and what granting it back costs, are in
`tests/security_probe_explain.rs`.

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

**And what it did not generate, which cost a wrong answer.** The file produced
a filter per side and no cross-side condition — no `Join::having` — for as long
as it existed. That single omission hid a real bug, and the reason is worth
stating because it is the whole argument for oracles: without a condition over
the formed pair, *every* probe that finds a bucket produces a pair, so "this
bucket was probed" and "this built row paired with something" are the same
statement. The hash join recorded the first and owed the caller the second.

With a condition they come apart. `having` is evaluated per pair, so it can
admit the pair with one row of a bucket and reject it with the next — and the
rejected row was swallowed by its neighbour's success, never drained, never
returned. A right outer join over the fixture came back with 30 rows building
the left side and 29 building the right: same query, same data, one book
missing, decided by a cost estimate.

It needed three things at once, which is why nothing hand-written had it:
building the *right* side (with the left built, right rows stream and each one
that finds no admitted pair is emitted immediately), a right or full outer join
(nothing else drains at all), and a condition that splits a bucket. The fix
flags the built row that paired rather than the bucket it lives in. It is pinned
by name as well as generated, and the generator now draws a cross-side condition
over two columns the join is not keyed on, one of which is null a third of the
time so the rejection also arrives by three-valued logic rather than only by
comparison.

It was found by the *grouped-join* oracle, not by this one — a grouped join runs
the same three algorithms and requires them to agree, and one of them counted
one book fewer. Reproducing it without grouping in the way took four lines.

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

## Grouping over a join, and ordering over groups

`crates/slate-kernel/tests/grouped_join_oracle.rs`

Two gaps closed at once. They fail in opposite ways, so they get different
oracles.

**A grouped join** consumes the joined row stream rather than a single table's
cursor, in the ordinal space `JoinSchema` defines — the space `Join::having` was
already written in. Everything that can go wrong there produces a *plausible*
group rather than an error: a key resolved to the wrong side reads as null and
collapses every row into one group; a projection narrowed too far reads as null
and does the same; an outer join's missing side is null and has to group *as*
null rather than vanish. A table of numbers comes back either way.

So the property is a fold written out over a join materialised with **no
projection at all**, which is what makes a narrowing that dropped a needed
column visible. The fold is deliberately different in two ways from the thing it
checks: it buckets by comparing `Value`s where the implementation hashes an
encoded key, and it orders by `Value`'s own ordering where the implementation
sorts the bytes. That the two agree is the codec's central property, so using it
here is a second statement of the answer rather than the same one. Its `match`
over `Aggregate` is exhaustive, so a new variant fails to compile rather than
going untested. Separately, all three join algorithms and the planner's own
choice must produce the same groups, which assumes nothing about what a group
means — a hash build flags its buckets and drains the unprobed ones at the end,
a nested loop streams, and the two see the rows in genuinely different orders.

**Ordering over groups** could not use the row path's bounded top-N heap, and
the reason is worth stating because it is not "it did not fit". That heap exists
for memory: sorting a million rows to return ten holds a million decoded rows.
Groups are all in memory before the first one can be returned — nothing can be
known about the last group until the last row has been read — so a heap there
would save nothing and would be a second sorting implementation to keep in step
with the first. What *is* shared is the comparator, so direction and null
placement cannot drift between rows and groups. The oracle is that comparator
written out again, over the group set filtered, sorted and sliced in the test:
the order of those three is SQL's and is a statement this test makes rather than
one it inherits, because taking a window before ordering returns a different set
of groups rather than the same set differently arranged.

Ties are pinned rather than left arbitrary. The groups arrive in encoded-key
order and the sort is stable, so `ORDER BY count(*) DESC LIMIT 3` over groups
that tie has one answer and not six — the same rule every oracle here follows by
appending the primary key to a generated sort.

**What it found.** The algorithm-agreement property failed on its first run
carrying a cross-side condition, and the bug was not in the grouping: a hash
join building the right side lost a book from a right outer join. It is written
up under "Joins, chains and aggregates" above, where the generator that should
have caught it now lives.

One more thing is measured rather than asserted, because it is the reason
grouping belongs in the kernel at all: a grouped join reads only the columns the
grouping needs. `count(*)` per author needs nothing off a book but the join key,
which the index on it holds, so **no** book row is read; asking for `sum(pages)`
in the same shape reads all thirty. The zero is next to the number in the same
run, so it is a measurement and not a constant.

### The one mutation that survived

Replacing the stable sort of the groups with `sort_unstable_by` fails nothing —
including a test written specifically to pin the tie order, over thirty groups
at seven distinct levels. Measured rather than assumed: on this toolchain
`sort_unstable_by` reorders that pattern for a `Vec<usize>` and does **not**
reorder it for a struct the size of a `Group`. So the mutation is not observable
at any size this fixture reaches, which makes it a fact about the standard
library's sort rather than a missing test. The stable sort stays, because the
guarantee wanted is that the tie order is a property of this code and not of
whichever sort `std` ships.

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

### Statistics for a value no column holds

`analyze` sampled rows and described columns, and an expression index keys on
something that is not a column — so its estimate came from the default a column
nobody measured gets: a hundred distinct values, a tenth of them null. That is
not a small error in one place. For an expression index it is the *only* number
behind every decision about it.

It now evaluates the expression over the rows it samples and puts the result
through the same distinct count, null count and reservoir as everything else.
The statistics are recorded against the **index**, because the value has no
ordinal of its own; a query that computes the same expression does give it one,
and the planner copies them onto that ordinal for the length of the plan, which
is what lets `predicate_selectivity`, `match_key` and `bounded_selectivity` read
them without any of them learning what an expression is. The alternative — a
resolver threaded through six signatures — is six places to forget.

Measured on two thousand rows over fifty tag buckets, five of them null and
each of the rest spelled three ways, so forty-five distinct computed values:

| predicate | rows | estimate before | after |
|---|---:|---:|---:|
| `lower(tag) = 'tag-07'` | 40 | 18 (2.22x out) | 40 (1.00x) |
| `lower(tag) < 'tag-10'` | 360 | 594 (1.65x out) | 352 (1.02x) |

The equality was wrong because a hundred distinct values is the default and
there are forty-five; the range because without a histogram a one-sided bound is
a flat third of the table whatever it asks for. And it changes plans, not only
numbers: a non-covering scan of an expression index costs a point read per row,
so on a million rows the unmeasured estimate of ten thousand matching rows loses
to a table scan and the measured one of a single row wins.

The tests are oracles rather than expectations. What `analyze` records is
checked against the same expression computed by hand over the same rows, and
what the histogram says against counting them — a statistic asserted equal to a
number someone wrote down only agrees with whoever wrote it. Two expression
indexes on one table are checked separately, and chosen so they cannot be
confused: every tag is the same length, so `length(tag)` has one distinct value
and no histogram at all where `lower(tag)` has forty-five and does. Swapping the
two slots fails both halves.

### Covering: taking the value out of the entry

An expression index could never satisfy an index-only scan, and the reason was
never that the entry held too little — it holds exactly the computed value the
query asked about. The executor was what could not use it: every scalar was
evaluated from the row's own columns, and a row rebuilt from an entry has the
source column null, so the scan would have computed `lower(null)` and answered
differently from every other access path.

The executor now takes that one value out of the entry it is already holding.
Which value that is comes from one function, called by the planner to decide the
scan covers the query and by the executor to decide what to substitute — two
answers to that question would be a wrong answer one edit away.

The change that made it possible is elsewhere, and is worth stating on its own
because it changes what a **table scan** returns: **a computed value's input
columns are read and are not part of the answer.** `SELECT id, lower(title)`
decodes `title`, computes, and puts `title` back to null. Before, it came back
populated. It had to change: an entry keyed on `lower(title)` cannot produce
`title`, so if a table scan produced it the two paths would differ on a column
nobody projected — which is the covering-scan bug the planner oracle found,
wearing different clothes. A column the *predicate* reads still comes back, as
the projection has always documented.

A pleasant by-product: an *ordinary* index now covers a query that computes
something, when it holds what the expression reads. `by_kind_size` can answer
`SELECT id WHERE kind = 'x'` computing `upper(kind)` with no row read at all.
That was previously impossible for a reason with nothing to do with expression
indexes — the planner expanded a computed value into its inputs before it knew
which index it was asking about, and then also demanded the computed ordinal
itself, which no index has.

The tests are differentials, because every way this can go wrong produces a
*plausible* row rather than an error: the same query is run as an index-only
scan and as a forced table scan and the **whole rows** are compared, not the
ids — comparing ids cannot see a column that leaked. The covering decision is
asserted per projection in the same loop, including the control that reaches for
the source column and must not be covered, and the reads are counted rather than
timed: zero for the covered query, one per row for the uncovered one measured in
the same run, so the zero is a measurement and not a constant.

The planner's guard saying an expression index is never covering was load-bearing
before this — forcing it true failed three tests — and each of the three now
asserts the new behaviour instead of being deleted.

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

## The wire says the same thing the kernel does, or it is wrong

A second query surface is a second place for an answer to come from, and the
only way to know it agrees is to ask both. `slate-server/tests/multi.rs` builds
a kernel `Join`, `Chain` or grouped query, runs it in process, converts the
same value to its wire form, asks it over a real socket, and requires the two
to return identical rows. 96 two-table cases — four join types by four
algorithm choices by six request shapes, of which 84 return rows, measured
rather than assumed — plus a three-table chain, a self-join whose second step
joins back to its first input, every aggregate function, grouping by a computed
value, `HAVING` over both a key and an aggregate, and a case per scalar shape a
string or integer column can reach.

The oracle is what makes the protocol's refusals meaningful rather than
decorative, because a refusal that should have been an answer shows up here as
a disagreement. It also catches the failure that motivated the reference model:
a client that computed flat ordinals from table widths would keep passing every
test until a column was added to an early table, at which point every later
reference would point one column left and the wire would quietly answer a
different question. Naming a producer and an index inside it makes that
unrepresentable rather than unlikely.

`tests/rls_join.rs` is the access-path matrix again, for the shape that has
more than one access path per row: 14 cells over three policies of three
different shapes, failures collected before asserting so one run names all of
them, and a control that runs the same join in process as a superuser and
requires the forbidden rows to actually appear — without it the matrix would
pass just as well against a join that returned nothing. Making the join handler
read as a superuser fails three of its six tests.

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

## A second pair of eyes on the newest four

Four bodies of code landed in a day and had been read once each, by whoever
wrote them. This is what a review that was told to hunt for a *wrong answer*
rather than a panic found, and — as valuable — where it came up empty, stated
precisely enough that nobody re-treads it.

Two findings. Both are the same shape, and it is the shape this document keeps
returning to: **the answer depended on something that is not part of the
question.** Neither is a crash, neither returns an error, and both return a
table of numbers that looks exactly like an answer.

### A sum that depended on the direction of the scan

`crates/slate-kernel/tests/review_aggregates.rs`

`Total::add` kept a running sum on an integer side until something forced a
float, then kept it on the real side. Integers arriving *after* the first float
were added to the integer side and, two lines later, discarded — the same
statement that moves the total to the real side zeroed the integer accumulator
unconditionally.

Nothing caught it for as long as a column had one declared type, because an
aggregate then only ever saw one domain. `Scalar` ended that: `SUM(coalesce(price,
amount))` over a nullable `f64` and an `i64` is an ordinary thing to write and
produces a float on some rows and an integer on others.

The property that catches it needs no oracle at all, which is what makes it
worth copying: **an aggregate is a fold over a set, so nothing about the answer
may depend on the order of the fold.** Scan direction varies that order and
nothing else. Over two rows, `SUM` came back as `1.5` read forwards and `11.5`
read backwards; `AVG` as `0.75` and `5.75`.

The test is kept as the order-independence property rather than as the two
numbers, because the numbers are a fact about a fixture and the property is a
fact about aggregation.

### A grouped join that depended on which algorithm won

`crates/slate-kernel/tests/review_grouped_join.rs`

`read::narrowed` drops a query's `limit` and `offset` before grouping it, and
says why: "aggregating a windowed subset of an unordered result is not a
meaningful request". `read::narrowed_join` cloned the join and replaced only the
two projections, so a join's own `limit` survived and windowed the row stream
before the grouper saw it.

A join has no order. Which rows the window kept was therefore whichever ones the
chosen algorithm happened to produce first — so the same grouped join over the
same rows came back as counts of 3/3/3/3 with the left side built and 4/2/4/2
with the right, decided by a cost estimate.

`grouped_join_oracle.rs` could not have found it: it builds every join it tests
through one helper, and that helper never sets `limit` or `offset`. The gap was
not in the property, which is the right one, but in the corpus — the same reason
the cross-side condition had to be added to `join_oracle.rs` before the hash
join's per-bucket flag became visible.

The fix drops the window in `narrowed_join` too, so the two grouped paths make
the same decision for the same reason rather than one of them making it by
accident.

### Two things the kernel accepts that a joined ordinal cannot mean

Pinned in the same file, as statements of what happens today rather than as
failures, because the fix is a refusal and that is a decision:

- A `Grouping` ordinal **past the joined width** groups every row under null and
  returns one row. `Join::validate` refuses exactly this for `having` — "would
  silently read as null, that is, as a condition nobody wrote" — and nothing
  applies the same rule to a `Grouping`, which arrives through the same call.
- A **left-side computed value's ordinal** is worse, because it is in range.
  `JoinSchema` packs by declared table width, so the ordinal a left-side query
  gives its first computed value is the right table's column 0 in the joined
  space, and `JoinedRow::flatten` truncates the left row before the grouper sees
  it. Grouping by it silently groups by the right table's first column.
  `records.proto` refuses this reference on `having` in as many words; the
  kernel's `Grouping` accepts it.

### Where it came up empty

Stated with the shapes, because "we looked at the covering path" is not a result
anybody can act on.

**The expression-index covering path** — `crates/slate-kernel/tests/review_covering.rs`.
Nine compute lists the planner oracle's generator cannot draw, against five
projections, four filters and all six access paths, plus the same nine sorted on
a computed value in both directions with and without a window. The lists are the
ones the second pass has to reason about: the same expression at two positions
(so the planner's "this one comes from the entry" and the executor's must be the
same position, and the *other* copy must be refused); a chain three long; a chain
whose last link also reaches for a table column, which must stop it; a forward
reference; an ordinal naming no computed value at all; both expression indexes'
values at once; and the descending index's value with a chain on it. No
disagreement anywhere. 24 of 360 hinted plans are index-only scans of an
expression index, asserted, so the differential is about the covering path and
not about a table scan agreeing with itself.

Reasoned through and found sound rather than tested: `covers` and
`QueryCursor::open` derive `from_entry` from the same function, so they cannot
disagree about which position the entry supplies; `output_columns` is computed
before any candidate is considered, so `transient` — and therefore which columns
a row comes back holding — is a property of the query and not of the plan; and
`Scalar::collect_columns` is exhaustive over the enum including the `Expr`
conditions inside a `Case`, which is what would otherwise let `covers` credit a
value whose real inputs the entry does not carry.

**The schema fingerprint** — `crates/slate-server/tests/review_fingerprint.rs`.
A collision sweep over ~20,000 declarations, with names chosen to attack the
*framing* rather than the hash: `:` and `;`, which are the canonical form's own
delimiters; `key` and `columns`, which are its two literal field markers; and
bare digits, which sit next to both the decimal length prefix and the decimal
ordinal. Every declared width the check would accept is in the sweep, which is
what folds the prefix rule into it. No two different declarations share a
fingerprint. The accepted-spellings cross product is sound by construction as
well: a declaration is accepted exactly when every ordinal's name is a current or
previous name *of that ordinal*, so no rename can make one column's name
acceptable at another's position.

What the prefix rule does leave open, pinned in the same file: the check compares
the declaration against the table's prefix and never against the ordinals the
request goes on to use, so a request may declare one column, pass, and then
filter on ordinal 2. The server has the information to refuse it — `convert`
resolves every `ColumnRef` after `fingerprint::check` runs — so closing it is a
policy change rather than a fix.

**`slate-serverd`'s configuration language** —
`crates/slate-serverd/tests/review_lang.rs`. The parser's own tests assert the
tree it produces, which is the right shape for a parser and has one blind spot:
whoever decided where the `And` goes also wrote the test that says where it goes.
So this asks from the other end — twenty-three predicates go into
`[[security.policies]]` entries, seven rows go into the store, and the ids that
come back are compared against a set written out by hand from reading the text as
SQL. Each case names the reading it rules out, so a case both readings agree on
cannot creep in. Covered: `AND` over `OR`, `NOT` over both, `NOT NOT`, unary minus
bare and inside an `IN` list and inside a conjunction, `<>` against `!=`, the
doubled-quote escape, `~*`, `NOT LIKE` against `NOT (… LIKE …)` — which lower to
different `Expr` shapes and must mean the same thing about a null — `NOT IN` with
and without a conjunction, and `IS NULL`. Nothing disagreed.

The claim in `Pred::lower` that a `:tenant` with no tenant behind it "fails closed
in every position a placeholder can occupy" is checked as its own block, read by a
caller with no tenant header: `=`, `<>`, `NOT (… = …)` and `NOT IN` all admit
nothing, with `IN ('a', :tenant)` as the control that a null in the list does not
swallow a real match. `Expr::In`'s three-valued rule is what makes the `NOT IN`
case hold, and it is the one that would have been easy to get wrong.

Read and found clean without a test: the lexer's operator table is longest-first
for every prefix pair (`<>`/`<=` before `<`, `!~*`/`!~` before `!~`… and no bare
`!`, so `!` alone is a refusal); `literal_from_number` refuses an out-of-range
`i64`, a negative against a `u64` and a fractional against either, so the type
boundaries are closed; and a literal's type comes from the column on the other
side of *this* comparison at both call sites of `operand`, so there is no nesting
in the predicate grammar for it to take the wrong column's type from.

## An index that returns nothing, and two the property suite found

Three defects from one round of work, kept together because the way each was
found is the point rather than the defect.

### Adding an index made queries return no rows

Not slower — *empty*. Index entries are written in the same transaction as the
row, so an index created after the rows exist has no entries for them. The
planner then costs it as cheap, scans a key range no write ever wrote into, and
answers with no error while the rows sit on disk where they always were.

It is intermittent, which is worse than if it always happened: whether a query
goes wrong depends on whether the planner picks the index, which depends on the
statistics and on whether the index covers the projection. A test that reads a
column the index does not hold passes.

`crates/slate-kernel/tests/migrations.rs` asserts the bad answer first — a
freshly declared index over a populated table, queried, returning nothing — and
then that the migration fixes it. The fix is a third keyspace (`0x03 <table
id>`) holding, per table, the schema version, a fingerprint of the layout, and
which indexes are actually built; `migrate::{plan, apply, verify}` back-fill an
index with no entries, reclaim a dropped one's, and refuse a layout change that
would reinterpret stored rows.

The fingerprint covers what decides how bytes are read — column count, types,
nullability, drops, the primary key, the tenant column, a decimal's scale — and
deliberately not names. A rename moves no bytes and must not read as a
migration.

`slate-serverd` reconciles **before** it binds its listener, because a node that
binds and then migrates accepts a connection and answers it wrongly. Turning it
off (`[schema] migrate_on_start = false`) makes it verify and refuse to start,
which is not the same as skipping.

### Every decimal compared equal to every other

`Value::Decimal` was added to the existing tuple *property* suite rather than
given one of its own, and it failed on the first run: `Decimal(i64::MAX)`
against `Decimal(i64::MIN)`. The new variant had a class rank in the ordering
but no match arm, so every decimal-against-decimal comparison fell through to a
branch whose comment read "unreachable: equal class ranks are exhausted above".
The comment had been true when it was written.

Hand-written cases would very likely have compared two decimals of different
magnitudes and different signs and never noticed, because the class-rank
comparison gets those right. The pair that exposes it is two decimals that
differ only in their value.

The second run found the other half: `skipping_and_decoding_can_be_mixed`
failed with "unknown type code 0x1e". Encode and decode knew the new code;
`skip` did not, so any tuple with a decimal before the element being read
decoded the wrong bytes.

Neither was found by a test written for decimals. Both were found by existing
tests that a new value type was dropped into — which is the argument for
putting a new variant into the oracle rather than beside it.

### A lost update the store cannot see

The store detects two writers overlapping in time. The read-modify-write across
two transactions does not overlap: read a row, decide something, write it
later, while somebody else wrote it in between. Nothing conflicts, and the
second write lands with a value computed before the first existed.

`update_if_unchanged` compares the whole stored row against what the caller
read. A version column was rejected for a reason worth keeping: a version only
detects changes made by writers who remembered to bump it, and the writer who
forgets is exactly the writer whose edit is silently lost. Comparing the row
needs no convention and no extra round trip — `update` already reads the row to
enforce the row policy.

## What is still not proven

Stated plainly, because a document like this is otherwise an advertisement.

That sentiment is not self-enforcing, and this list proved it: five entries sat
here after the work was done or the claim withdrawn — partial indexes in the
derive macro, grouped joins on the wire, grouping over a chain, a grouped join's
cost, and the head node's performance. A list of open problems that quietly
keeps closed ones is read as current and is worse than no list, which is the
same failure mode this document warns about everywhere else. They have been
removed; what follows is what genuinely has not been done.

One of the five was not *done* but withdrawn, and that distinction is worth
keeping: "a grouped join is not costed" was based on a plausible reading of the
code that the arithmetic does not support — the per-joined-row term is added to
both candidates, and a term common to both sides of a comparison cannot decide
it. `the_per_row_term_is_symmetric_so_grouping_cannot_flip_the_algorithm`
asserts the symmetry the withdrawal rests on, so the day someone makes that term
asymmetric the item comes back by failing test rather than by memory.

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
- **Expression statistics at scale, or on storage.** The improvement is measured
  on an in-memory fixture of two thousand rows. Nothing says how a reservoir
  sample of a computed value behaves on a table where `analyze` is itself a long
  read, and the cost of evaluating an expression per sampled row is not measured
  against the read it rides along with.
- **A partial *and* expression index's statistics.** They describe every row the
  caller can see, not only the rows the index holds, because the planner
  multiplies the key's selectivity by the predicate's separately and describing
  the subset would count the predicate twice. That matches how a partial index's
  column statistics already work; neither has been measured against a partial
  index selective enough for the independence assumption to hurt.
- **A read-only node that starts before the leader has migrated.** It verifies,
  finds the schema state behind, warns and serves. During that window a query
  through an unbuilt index returns no rows — the defect above, on a follower.
  Closing it means the follower waiting, which needs a way to tell "the leader
  has not migrated yet" from "there is no leader", and that distinction does
  not exist yet.
- **Decimals anywhere but the kernel.** `Value::Decimal` has no case in the
  gRPC `Value`, so one reaching a client becomes a visible
  `<unrepresentable decimal>` marker. That is pinned by a test whose own doc
  comment says it is a pin and not an endorsement — but nothing establishes
  that the marker is the *right* answer rather than the one that was cheap.
- **Decimal arithmetic.** `price * quantity` is not expressible: the product of
  two scale-2 values is scale-4 and nothing in the expression layer tracks
  scale. `AVG` over a decimal returns a float for the same reason, since the
  mean of exact decimals is generally not representable at their scale.
- **A decimal literal in the SQL front end.** `WHERE total > 19.99` parses as a
  float and will not match a decimal column. It does not refuse, which is the
  part that is wrong.
- **An interrupted back-fill.** Every migration test runs against
  `MemoryStore`, and `a_backfill_larger_than_one_batch_writes_every_row`
  establishes that batching works — but nothing kills a migration partway and
  restarts it. Resumability is an argument about where the state record is
  written, not a measured property, and it has never been tried against a real
  object store where a batch can fail on its own.
