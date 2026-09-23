# `WITH` is three constructs with three answers, and the row called them all Missing

- **Date:** 2026-09-20
- **Author:** Claude, answering the question the CTE gap row said nobody had answered
- **Touches:** `docs/ctes.md`, `docs/orm-comparison.md`, `crates/slate-wasm/src/sql.rs`, `crates/slate-wasm/tests/ctes.rs`
- **Kind:** docs

## What changed

`docs/ctes.md`, splitting CTEs into recursive (Refused), referenced more than
once (Refused) and non-recursive-referenced-once (Missing, and the same
mechanism as views). A named refusal for `WITH` in the SQL front end carrying
that split, in place of `expected SELECT, INSERT, UPDATE or DELETE, found
`WITH``. Four tests. The comparison's CTE and views rows now say the same thing
as each other.

No behaviour changes: `WITH` was refused before and is refused now. What
changed is that the refusal is true about which part.

## Why

The gap row asked the question itself:

> Whether that makes this Refused (like `UNION`) or Missing is a question
> nobody has answered in writing, and the row has been asserting the second for
> as long as it stood.

Answering it needed the constructs separated, and separating them produced the
finding: **a single-reference, non-recursive CTE and a view are the same piece
of work.** The views row already says the safe shape is *"a view expands to its
underlying spec before planning, so the base table's id is what reaches the
policy, and that has to be structural rather than a convention somebody
remembers."* A single-reference CTE expands to its underlying spec before
planning. Same sentence. The two rows had been listed as separate gaps.

The security argument transfers intact and a CTE meets it *sooner*. Grants and
policies key on `TableId`; a CTE has no catalog entry, so
`Catalog::table_by_name("recent")` finds nothing, which is why the parser fails
where it does. The obvious fix — register the CTE under a synthetic `TableId`
so the lookup succeeds — is the exact hazard the views row names, and it is the
fix somebody reaching for the feature would reach for first.

## Alternatives rejected

**Answer "Refused" and move on.** Defensible for two of the three shapes and
wrong for the third, and the third is the one somebody could build. A row
saying "Refused" would close the question by making it un-askable, which is
worse than leaving it open.

**Answer "Missing" and move on** — what the row already said. Wrong about the
recursive case in a way that would waste whoever picked it up: they would
discover the fixpoint problem after writing a parser for `WITH RECURSIVE`.

**Leave the parse error alone and put the answer only in the note.** The
`UNION` refusal's own comment is the argument against: a bare "unexpected
`UNION`" *"reads as a parser that has not heard of it, when the real answer is
that there is nowhere for it to go."* `WITH` had precisely that error. A note
nobody is pointed at is a note nobody reads.

**Say "CTEs are not supported" in the message and link the note.** Shorter, and
it asserts the thing that is wrong about one of the three. The message is long
because the split is the content; it names each shape, gives each reason, and
ends with what *does* work today — an uncorrelated subquery in `IN (SELECT …)`.

**Build the single-reference inlining now.** It is genuinely the smallest of
the three and it is *not* small: where the expansion happens, what it does
about a CTE name colliding with a real table, whether `EXPLAIN` shows the name,
and what an outer `ORDER BY` does with a column the CTE projected away are all
undecided — and all of them are decisions the views design has to make anyway.
Building the CTE half first would decide them by accident, in the place with
less at stake, and views would inherit them.

## Evidence

**What it did before, measured rather than assumed**, through the workbench's
own parser:

```
WITH recent AS (SELECT id FROM books WHERE year > 2000) SELECT id FROM recent
  -> expected SELECT, INSERT, UPDATE or DELETE, found `WITH`

SELECT id FROM books WHERE id IN (SELECT id FROM books WHERE year > 2000)
  -> accepted, planned as an ordinary IN
```

Four tests: every spelling reaches the named refusal (including a bare `WITH`
with nothing after it, and the lower-case form); the message carries all three
answers; the subquery it recommends is actually accepted; and `with` is special
only as the first word, so a schema with a `with` column is unaffected.

Five mutations. Four caught:

```
ok  WITH falls through to the generic message again  -> two tests
ok  the read-twice clause is dropped                 -> the_refusal_separates_the_three_shapes
ok  the recursive clause is dropped                  -> the same (after the fix below)
ok  the buildable case is described as refused too   -> the same
```

**One mutation was invalid and I wrote it**: replacing the message's first line
with "Ignore the rest of this:" *inserts* text without removing any asserted
content, so of course every assertion still held. Not a missing test — a
mutation that does not change the property under test. Recorded rather than
chased.

**One survivor was a real vacuous assertion, in a test written minutes
earlier.** `assert!(message.contains("recursive"))` is satisfied by the phrase
"a non-recursive CTE referenced once" further down the same message, so the
entire recursive clause could be deleted and the test would pass. It now
asserts `"fixpoint"` — the reason rather than the word — and the deletion is
caught. A substring assertion over prose is only as sharp as the substring's
uniqueness, and "recursive" is a substring of "non-recursive".

`cargo test -p slate-wasm --test ctes`: 4 passed. `python3 site/check/docs.py`:
the docs site holds together, with the new relative links resolving.

## What this does not do

**It builds nothing.** Two thirds of the row is now Refused with a reason and
one third is Missing with its work named. No CTE runs.

**The three-way split is a claim about SQL, not a proof about this codebase.**
That a recursive CTE needs a fixpoint and that reading a CTE twice needs
materialisation are properties of the constructs; that neither fits a
`QuerySpec` is read off the type. I did not try to build either and fail.

**The inlining equivalence in §3 was reasoned, not executed.** The note says
`WITH recent AS (SELECT id, title FROM books WHERE year > 2000) SELECT title
FROM recent WHERE id > 5` is `SELECT title FROM books WHERE year > 2000 AND id
> 5`. There is nothing to run the first through, so the claim rests on reading
the grammar. It is stated as such in the note.

**A CTE over a join is unexamined.** The inlining discussed is single-table on
both sides. A CTE whose body is a join, referenced from a query that also
joins, is a chain, and whether that composes or needs a fourth answer is not
decided.

**The refusal's message is prose with no guard on its accuracy.** Four
assertions check that four phrases are present; nothing checks the message is
coherent, and nothing would notice if `docs/ctes.md` were deleted while the
message kept pointing at it. `scripts/check_cited_tests.py` checks cited *test*
names, not cited paths.
