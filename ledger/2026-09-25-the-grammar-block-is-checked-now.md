# Auditing the SQL module's "grammar, in full" against the parser found six things it was wrong about. Four of its name lists are now compared to the parser's own tables by a test, and every production has a case.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `crates/slate-sql/src/sql.rs` (module docs, and a new `#[cfg(test)] mod grammar`), `crates/slate-sql/tests/front_end.rs`
- **Kind:** docs, and the tests that keep them honest

## What changed

`sql.rs` opens with a grammar block that a reader takes as the specification.
It now says what the parser does, and four tests inside the module read the
module's own source and compare its name lists against `TIME_FUNCTIONS`,
`AGGREGATES` and the refusals. Ten cases in `tests/front_end.rs` cover the
productions, which a test cannot read as data.

## Why

`ledger/2026-09-25-the-grammar-comment-outlived-the-grammar.md` fixed one line
of that block this morning and recorded, as its own caveat, that it had not
looked at the rest. Looking at the rest found **six** things:

1. **`WHERE <cond> (AND <cond>)*`** — wrong since the `OR` work landed the
   same afternoon, so the block said a feature shipped that day did not exist.
   Same for `HAVING`.
2. **The call list was missing `month_start` and `year_start`**, added by
   `ledger/2026-09-15-a-month-is-not-a-number-of-seconds.md` and
   `ledger/2026-09-15-the-year-there-was-no-year-to-key-on.md`, both on the
   15th. Ten days.
3. **`min` was never documented at all.** Not in the block, not in the prose —
   the module named `count`, `max`, `sum` and `avg` in passing examples and a
   reader working from this file had no way to learn `min` exists.
4. **One optional `JOIN`.** Chains — several `JOIN`s, each joining to an
   earlier input — landed in
   `ledger/2026-09-15-three-tables-in-the-front-end.md` and the block still
   showed `[ JOIN <table> ON <col> = <col> ]`.
5. **`SELECT <item-list>` on a join.** The block's shape says an item list is
   allowed wherever a `SELECT` is. An *ungrouped* join refuses it — "a join
   returns whole rows; write `SELECT *`" — which I found by writing a test
   from the block and watching it fail.
6. **"Statements may be separated by `;`."** True of the module and false of
   `parse`, which answers `unexpected ';'`. [`split`] separates them. The
   sentence sat beside the grammar, where it reads as a property of parsing.

Also absent: `DISTINCT`, aliases, `CONTAINS`, windows, `IN`/`NOT IN`, and the
refused keywords. All are in the block or the prose now.

Five of the six are the same failure: a feature landed, its own entry was
written, and the specification a reader reads first was not touched. The
sixth — `min` — was never there.

## Alternatives rejected

**Fix the block and move on.** It is what this morning's change did, one line
at a time, and this is the second session to find the same file stale. A list
maintained by whoever remembers is the thing `CLAUDE.md` says never to ask a
human to keep in step, and this file has four of them.

**Check the productions too, with a grammar engine.** A parser generator fed
the block and compared against the hand-written parser would be genuine
coverage, and it is the "second implementation" trap in its purest form: the
generator would accept things the parser refuses for reasons that live in the
refusals, not in the grammar, and every one would need an exception. The block
would end up written to satisfy the generator. Ten cases that parse what each
line describes are weaker and do not lie about what they prove.

**Put the list checks in `tests/`.** They read `TIME_FUNCTIONS` and
`AGGREGATES`, which are private. Exporting a table so a test can see it makes
it part of the crate's surface for no other reason, and the unit-test module
can see it already.

**Bound the list sentences by the phrase that follows them.** The first
version did — `find("There is no date")` — and that sentence wraps across two
`//!` lines, so the search ran past the end of the doc comment and collected
**every backtick in the file**, about 800 of them, including its own source.
It failed loudly and is the reason the prose now promises each list is one
sentence ending at its first full stop, with the promise written where the
next person to reword it will read it. A delimiter that can silently
over-match is one to replace, not to patch: had the file happened to contain
the right names further down, this would have passed.

## Evidence

**Two of the six were found by a test failing rather than by reading.** Cases
5 and 6 were written *from the block* — `SELECT books.title FROM books JOIN
authors ...` and `parse("SELECT *; SELECT id")` — on the assumption the block
was right. Both failed. That is the argument for writing a case per line even
when the case looks trivial: reading the block against the parser found four
things, and executing it found two more.

**Mutations**, five run, five caught, no survivors:

| mutation | caught by |
|---|---|
| `month_start` dropped from the prose's call list | `the_grammar_block_names_every_computed_call` |
| `min` dropped from the prose's aggregate list | `the_grammar_block_names_every_aggregate` |
| the `WHERE` line reverted to `(AND <cond>)*` | `the_where_and_having_lines_admit_or` |
| the `HAVING` line reverted to `(AND <group-cond>)*` | `the_where_and_having_lines_admit_or` |
| `EXISTS` dropped from the refused-keyword sentence | `the_refused_keywords_are_all_named` |

Each is a name removed from prose, which is exactly the change that has
happened to this file in the other direction five times.

**Suites.** `cargo test -p slate-sql`: **4 + 24 passed, 0 failed** (4 unit,
24 integration; 13 integration before). `cargo test -p slate-wasm`: every
binary green, 227 tests across 17 binaries, which is the suite that would
notice if correcting the block had needed a parser change. It did not — no
parser code changed here. `cargo clippy --workspace --all-targets`: clean.
`cargo fmt -p slate-sql` applied.

**Null result worth stating:** the site's `features.html` describes computed
columns as "`hour()`, `day_of_week()`, `year()` and the rest", which is not a
list and so was not stale. I checked it because this entry is about lists
going stale, and found nothing to fix.

**The runs themselves**, as `mutate.py` recorded them:

- `ledger/mutations/20260925T224539-crates-slate-sql-src-sql-rs.json` — crates/slate-sql/src/sql.rs

## What this does not do

**The operator list is still unchecked.** `= != <> < <= > >= LIKE ILIKE ~ IN
NOT IN CONTAINS` is a case per spelling in `front_end.rs`, not a comparison
against a table, because the parser does not hold the operators in one — they
are arms of a `match`. Adding one and not documenting it stays possible. A
table would make it checkable and would exist only to be checked, which is a
worse reason than it sounds.

**The productions are checked by example, not by shape.** `[ LIMIT <int> ]`
has a case proving the clause is optional and takes an integer. Nothing
notices if it silently starts accepting a float.

**The prose outside the lists is unchecked**, which is most of it — five
screens about timezones, joined ordinals and why a chain is not a join. Those
are explanations rather than specifications, and no test here can tell whether
one has stopped being true.

**Neither `docs/` nor the site was audited the same way.** This is one file.
`docs/sql.md` and `site/docs/features.html` describe the same grammar in
prose, and whether either has the same six defects, I did not check.
