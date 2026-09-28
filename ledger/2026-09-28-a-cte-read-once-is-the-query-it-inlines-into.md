# A non-recursive CTE read once expands inside the parser, and the expansion is why it needs no security review

- **Date:** 2026-09-28
- **Author:** Claude Opus 5, in a Claude Code session
- **Touches:** `crates/slate-sql/src/sql.rs`, `crates/slate-sql/tests/front_end.rs`,
  `crates/slate-wasm/tests/ctes.rs`
- **Kind:** feature

## What changed

`WITH <name> AS ( SELECT <columns> FROM <table> [WHERE …] ) <select reading
<name>>` now parses, and lowers to the `QuerySpec` the inlined statement
compiles to — the *same* spec, asserted with `assert_eq!` rather than by
checking the two agree on an answer. The body's `WHERE` is conjoined onto the
outer query's with the body's terms first, the body's projection becomes the
outer query's namespace (so `SELECT *` over it is the body's columns and a
column it projected away is refused), and nothing else about the statement
moves. Six shapes are refused with their reasoning in the message:
`WITH RECURSIVE`, a CTE read anywhere but the outer `SELECT`'s own `FROM`, a
name that is already a table's, a body that joins or sorts or groups or pages,
a second CTE in one `WITH`, and a `WITH` nothing reads.

The blanket `WITH is not supported` refusal that named the three-way split is
gone, because two thirds of it are now two separate refusals and the third is
built. `crates/slate-wasm/tests/ctes.rs` held that message and is rewritten
around the split — see **What this does not do** for why a file outside
`slate-sql` is in this diff.

## Why

`docs/ctes.md` §3 says a single-reference non-recursive CTE is *Missing* rather
than *Refused*, and that it is the same mechanism as a view: an expansion into
the outer spec before planning. It could name the work and not do it; this does
it.

The part worth writing down is **where** the expansion had to happen, because
the note names the hazard and the hazard is what makes the obvious
implementation wrong. Grants and policies key on a `TableId`
(`Grant { table: TableId }`, `Policy { table: TableId }`) and a query resolves
its table through `Catalog::table_by_name`. The tempting fix is to register the
CTE so that lookup succeeds — and then every check keys on a synthetic id, and
a caller reads the base table's rows with the base table's policy never
consulted.

**That scoping claim was checked rather than assumed, and it holds.**
`sql::parse` has exactly two callers outside its own crate:
`crates/slate-wasm/src/lib.rs:1275` (the workbench) and
`crates/slate-serverd/src/views.rs:124` (a view's SQL, at load). Both receive a
`Statement`, and what the expansion puts in `QuerySpec::table` is `books`. A
test asserts it directly, and asserts further that the string `recent` does not
appear anywhere in the serialized spec — so `Head::authorized_read_source`,
`Head::authorized_table` and `SecurityCatalog::row_filter_with` all see the
base table's `TableDef` and need no case for a CTE. **No kernel change, no
server change, no change to the security model**, and none was made. A view is
a server concept with a registry deliberately kept out of the `Catalog`; a
CTE's name lives between two clauses of one statement and has nowhere to be
kept at all, which is the stronger version of the same property.

The four questions `docs/ctes.md` left open, each a refusal, each with a test:

1. **A name that collides with a real table** — refused. SQL shadows the table.
   Here the expansion leaves `spec.table` saying `books` either way, so the one
   artifact this front end shows a reader — the compiled spec beside the
   results — could not tell the CTE's rows from the table's. A shadow nobody
   can see is not a feature.
2. **A column the body projected away** — refused. The projection *is* the
   outer query's namespace, which is what SQL says and the only reading under
   which `SELECT * FROM recent` has an answer. Resolving it against the base
   table instead would make the projection a suggestion.
3. **A body that is a join** — refused; the case the note called unexamined.
   An ungrouped join returns whole rows and takes `SELECT *` and nothing else,
   because a projection there is a projection in the joined row's ordinal
   space. So `SELECT title FROM pairs` over a joined body would name a column
   in a space the outer query cannot address — and that is the refusal the join
   path already makes, reused rather than reasoned afresh.
4. **Several CTEs in one `WITH`** — refused. The outer query reads one table,
   so at most one of several could be the one it reads: a second is either
   never read, or read by the first, which is a CTE over a CTE. That is the
   nesting `docs/views.md` §5 refuses for views, where the base table stops
   being a field and becomes a traversal with a cycle check and a depth bound
   behind it. Two predicates composed are `WHERE a AND b`, which one CTE
   already says.

## Alternatives rejected

**Splice the CTE's tokens into the outer statement and re-parse.** Genuinely
attractive: the merge would be `WHERE (<body>) AND (<outer>)`, and this
parser's existing "a bracket that nests nothing produces the unbracketed spec"
property would do the flattening for free. Rejected because it cannot see the
*namespace*. Token splicing makes `SELECT year FROM recent` resolve `year`
against `books` and succeed, which is decision 2 answered the wrong way and
answered silently — the projection would be decoration. Every other refusal
here would also have to be recovered by scanning text, which is how
`from_table` in `slate-serverd/src/views.rs` is allowed to be wrong (it can
only make a refusal more specific); a rule that decides what a query *means*
cannot be.

**A narrowed `TableDef` holding only the projected columns.** The tidy shape:
the outer query resolves against it and the namespace falls out. Rejected
because the ordinals then belong to the CTE, and every one of them —
projections, filters, sort keys, group keys, and the `columns().len() + i`
arithmetic that places computed and window columns — would need remapping onto
the base table's on the way out. `docs/views.md` refuses a view's projection
for exactly this reason and calls the remapping "a separate piece of work with
its own way to be silently wrong". Keeping base ordinals everywhere and
narrowing only *name resolution* costs one `Option<&[u32]>` parameter and has
no mapping to get wrong.

**Register the CTE in `Schema` so `table_by_name` finds it.** Rejected in
`docs/ctes.md` before this was written, and it is the reason the note exists.
Restated here only because it is the implementation a reader will think of
first, and because `Schema` is one line from being able to do it.

**Refuse decision 2 by refusing projections in CTE bodies altogether** — that
is, only allow `SELECT * FROM t WHERE …` as a body, the way a view may only be
a `WHERE`. That would have made the namespace question disappear. Rejected
because the projection is the half of a CTE that the `IN (SELECT …)` path does
not already cover, and `docs/ctes.md` §3's worked example has one; a feature
whose example it cannot run is not the feature.

**Accept several CTEs where each is read once, via a join.** `WITH a AS (…), b
AS (…) SELECT * FROM a JOIN b ON …` is real SQL and each side is read once.
Rejected for now: a `JoinSpec` splits filters by side so each scan is narrowed
before the hash join, and merging a body's `WHERE` into one side of that is a
merge nobody has designed. Refusing it costs the reader a query they can write
as one join with both predicates in the `WHERE`.

**Let `SELECT *` over a projecting CTE keep the spec's empty projection**
(which means "every column"). Rejected: it would hand back the columns the body
removed, with nothing anywhere saying so. Wrong answers are worse than
refusals, and this one is not even a refusal.

## Evidence

TDD, strictly. The first batch of cases went into
`crates/slate-sql/tests/front_end.rs` before any parser change: sixteen of them
failed against the blanket `WITH is not supported` refusal and three passed
vacuously against it, because they assert refusals that had to survive. The
file went from 33 tests to 57; all 57 pass, as do the whole `slate-sql`,
`slate-wasm`, `slate-server` and `slate-serverd` suites, and
`sh scripts/check.sh` reports 71 of 71.

The load-bearing assertion is equality of specs, not agreement of answers:

```
WITH recent AS (SELECT id, title FROM books WHERE year > 2000)
SELECT title FROM recent WHERE id > 5
```

produces a `QuerySpec` that `assert_eq!`s with the one
`SELECT title FROM books WHERE year > 2000 AND id > 5` produces — same
`filters` list, in that order, in that field.

**Mutation testing: 19 cases in the first run, 18 caught and one survivor;
after the missing test, the survivor and one companion mutation were both
caught — 20 distinct mutations, all caught.** Records in
`ledger/mutations/20260928T162510-…json` and
`ledger/mutations/20260928T162624-…json`, including each case's verdict and the
tests that named it.

The survivor is the one worth the section. `conjoin` splices a conjunction's
parts rather than wrapping them, and making it wrap instead —
`PredicateSpec::All(parts) => vec![PredicateSpec::All(parts)]` — changed
nothing any test could see. It was a real change, not an equivalent one: it
turns the merged `WHERE` from a flat `filters` list into an `All` of `All`s,
which is the one shape `flatten` cannot flatten, so the spec lands in
`predicate` instead. No test caught it because **every CTE body and every outer
`WHERE` in the suite was a single condition**, so the splice was only ever
joining two leaves and the two implementations agreed on all of them.
`a_conjunction_on_either_side_of_the_merge_stays_flat` puts two ANDed
conditions on each side in turn and asserts `filters.len() == 3` with
`predicate` still `None`; it catches the mutation, and a companion mutation
that drops `conjoin`'s right-hand side entirely.

One of the twenty also found a hole before it was mutated, while the cases were
being chosen. The single-table `aggregate` resolved its argument through
`resolve_named(raw, table, table.name(), at)` — the one path in the file that
does not go through `resolve` — so `SELECT count(year) FROM recent` would have
read a column the body projected away while the bare `year` beside it was
refused. A namespace with one hole in it is worse than none, because the hole
is where somebody stops looking. `aggregate` now takes the same scope
everything else does, and
`an_aggregate_over_a_cte_resolves_in_the_ctes_namespace_too` pins both halves.

A second hole was found the same way, by asking which test would fail if a
guard were deleted. `select`'s refusal of a CTE as a join's **left** input is
reached by nothing else: `first_input` has already consumed the name, so
`table` never sees it, and without the guard `WITH recent AS (…) SELECT * FROM
recent JOIN authors ON …` would have planned as an ordinary join **with the
CTE's `WHERE` silently dropped**. The suite had `books JOIN recent` and not
`recent JOIN books`; it has both now, and the mutation that deletes the guard
is caught.

Run through the browser binding as well, which is the only evidence that the
expansion reaches an executor rather than merely a spec:
`WITH recent AS (SELECT id, year FROM books WHERE year > 2000) SELECT id FROM
recent WHERE id > 5` answers, and its plan is `Table Scan on books` with
residual `Compare { column: Ordinal(3), op: Gt, value: I64(2000) }`. The plan
names `books` and nothing in it names `recent`, which is the security claim
above where a caller can see it.

## What this does not do

**It touches a file outside `crates/slate-sql`, and that was not the plan.**
`crates/slate-wasm/tests/ctes.rs` asserted `message.contains("WITH is not
supported")` for two statements that now parse, so the alternative to editing
it was a knowingly red build. It is rewritten around the split rather than
deleted: the two refusals still have to refuse with their own reasons, and one
new case runs the accepted shape through `Playground` — which is the only test
anywhere that the expansion reaches the executor.

**It updates no design note or comparison row.** `docs/ctes.md` still says the
single-reference case is Missing, `docs/orm-comparison.md`'s CTE row still says
the parser "refuses `WITH` by name with the split", and `docs/views.md` still
says `ctes.md`'s half is undecided. All three are now wrong, and
`site/docs/roadmap.html` describes the refusal too. They were left alone
deliberately because another session is handling them centrally; by this
repository's own standard — stale documentation is worse than none — that is a
debt with a name on it, not a gap that can be left.

**A CTE body may not compute, window, group, sort or page**, and may not be a
join. Each is refused with the reason rather than merely absent. `LIMIT` is the
one that would be *wrong* rather than unsupported: the merge puts the outer
`WHERE` inside the body, so the filter would apply before the limit instead of
after it, and the query would quietly mean something else.

**A CTE may not be read from a join input or a subquery**, even once. The
refusal says so and says what it would take.

**Nothing measures the expansion's cost**, because there is nothing to measure:
the spec that comes out is one an unaided reader could have typed, so the plan,
the scan and the row count are identical by construction. The claim that
nothing new reaches the kernel is tested by comparing the two lowered
`Expr`s, not by timing them.

**A repeated column in a CTE's projection** — `SELECT id, id FROM books` — is
accepted and left as two entries in the namespace, which makes `SELECT *` over
it project the same ordinal twice. Nothing tests it and nothing downstream was
checked for it; it is the one shape here I would not defend, and the honest
reason it is not refused is that I did not decide.

**`EXPLAIN` over an expanded CTE is not considered.** `docs/views.md` leaves
the same question open for a view — whether a plan naming the base table says
anything a caller with `Explain` should not see — and a CTE lands in exactly
the same place, since the plan above names `books`.
