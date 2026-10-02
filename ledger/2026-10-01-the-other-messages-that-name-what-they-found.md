# The other refusals that named what they found

- **Date:** 2026-10-01
- **Author:** Claude Code (session: work down the open caveat backlog)
- **Touches:** `crates/slate-serverd/src/lang/pred.rs`,
  `crates/slate-serverd/src/lang/lex.rs`
- **Kind:** fix

## What changed

The predicate parser's refusals were read for the weakness
`ledger/2026-09-29-the-same-wrong-sentence-twice-in-one-file.md` recorded in
the regex one — *a message that names what it found, not what the reader
wanted*. **Three more had it**, and each now says what is wrong:

| was | is |
|---|---|
| ``size BETWEEN 1 AND 5`` → *the word-spelled tests are `LIKE`, `ILIKE`, `IN` and `IS NULL`* | *A range is two comparisons here, joined with `AND`: `size >= 1 AND size <= 5`.* |
| ``count(id) > 1`` → *there is no column `count` here; this table has `id`, `kind` and `size`* | ``count(…)` is a function call, and a predicate has no functions…* |
| ``docs.kind = 'a'`` → `` `.` cannot appear in an expression `` | *…A predicate is scoped to one table, so column names here are unqualified: write `kind`, not `docs.kind`* |

## Why

The caveat was one sentence:

> **No other error message in the parser was reviewed for the same weakness.**
> This one was fixed because an entry recorded what it cost. `expected a
> column name, found …` and the operand-type refusals may have the same shape
> and were not examined.

So: examine them. Not by reading the format strings — the first version of
this was going to be exactly that, and the format strings look fine — but by
**running sixteen inputs through the parser and reading the sentences a
person gets.** The two are not the same exercise. `count(id) > 1` produces
``there is no column `count` here``, and nothing in the source of
`resolve_column` suggests that; the message is assembled from a name the
caller supplied and reads wrong only when you see the name.

Of the sixteen, four were clean and had already named the alternative
(`IS NOT 'a'`, `NOT` without `LIKE`, `kind IN 'a'`, `kind = NULL`), four were
exotic enough that the input is the problem rather than the message, five
were already right, and three had the weakness.

**`count(id) > 1` is the sharpest and is worse than the one that started
this.** The regex message was *accurate and unhelpful*: it named what it
found and the reader had to know the answer already. This one is accurate
and **misleading**: it names a missing column, so the obvious next move is
to go and add a column called `count`. A reader who takes the message at its
word ends up somewhere further from working than where they started.

The function-call check sits in `column()` before `resolve_column`, not
after it fails, so `size(id)` — a function sharing a real column's name — is
refused as a call rather than parsed as the column with a stray `(` after
it. That case has its own row in the test.

## Alternatives rejected

**Accepting `BETWEEN`.** It is three tokens of grammar and every dialect has
it. Rejected on the same argument the `matches` alias was rejected on and
which this entry does not reopen: a second way to write `a >= x AND a <= y`
is a synonym in the grammar, and the cost of learning the one spelling is a
sentence in an error message. The difference between that and `matches` is
only that `BETWEEN` is sugar rather than an alias, and sugar in a predicate
language that a form generates is harder to justify, not easier.

**Accepting a qualified name and checking the table part.** `docs.kind` would
parse and refuse if `docs` is not the scoped table. Rejected because the
lexer refuses `.` outright today, and teaching it a qualified name means
deciding what `a.b.c` is, what a quoted `"a.b"` is, and whether an alias is
in scope — a grammar decision for one error message. The hint costs two
lines and says the true thing: there is one table here.

**A roster of function names.** `count`, `sum`, `upper`, `lower`, `now` — the
message could say *"`count` is an aggregate, which belongs on the query"*
rather than the generic sentence. Rejected: the roster is the
`REACHED_FOR`-shaped guess again, and here the syntax alone settles it —
anything followed by `(` in column position is a call, whatever it is called,
so the general rule is both simpler and complete. `REACHED_FOR` needs a
roster because `matches` is a bare word and indistinguishable from a column
name; `count(` is not.

**Fixing the messages without the probe.** Faster, and it would have found
`BETWEEN` and missed the other two. The format strings for both read
correctly in isolation; what is wrong is the sentence a particular input
produces, which only running it shows. That is the method this entry is
really about.

## Evidence

**The probe, verbatim, before the change** — sixteen inputs through
`parse(source, &TableScope::constant(&table))` against a three-column table:

```
REFUSED size BETWEEN 1 AND 5
    ^ expected a comparison after the column, found `BETWEEN`. The
      word-spelled tests are `LIKE`, `ILIKE`, `IN` and `IS NULL`; every
      other operator is punctuation.
REFUSED count(id) > 1
    ^ there is no column `count` here; this table has `id`, `kind` and `size`
REFUSED upper(kind) = 'A'
    ^ there is no column `upper` here; this table has `id`, `kind` and `size`
REFUSED docs.kind = 'a'
    ^ `.` cannot appear in an expression
```

and the ones that were already right, which is the half worth recording
because it bounds the finding:

```
REFUSED kind IS NOT 'a'
    ^ expected `NULL` after `IS`; the only `IS` test is `IS NULL` and `IS NOT NULL`
REFUSED kind = NULL
    ^ comparing to NULL is always unknown and so admits no row; write `IS NULL`
      or `IS NOT NULL`
REFUSED size > '3'
    ^ a quoted string cannot be compared with a i64 column
REFUSED kind IN 'a'
    ^ expected `(`, found a string literal
REFUSED kind LIKE 'a%' ESCAPE '!'
    ^ `ESCAPE` is left over; the expression already ended
```

**Five mutations, all caught**
(`ledger/mutations/20261001T011315-crates-slate-serverd-src-lang-pred-rs.json`,
`ledger/mutations/20261001T011355-crates-slate-serverd-src-lang-lex-rs.json`):

| mutation | caught by |
|---|---|
| `RANGE_WORDS` emptied, so `BETWEEN` gets the generic hint | 2 tests |
| the range hint replaced with the regex hint | 2 tests |
| the function-call check disabled | `the_three_refusals_that_named_what_they_found` |
| the function-call check fires on any next token | 4 tests, including `a_grouping_paren_is_not_a_function_call` |
| the `.` hint dropped | `the_three_refusals_that_named_what_they_found` |

The fourth is the control that matters: the grammar's one legitimate `(` is
a grouping paren, and a check written as "is there a next token" eats it.

**And an existing test went red, correctly.**
`a_word_that_is_not_a_regex_guess_names_the_word_operators` used `between`
as its example of a word with no roster, which is exactly what `between`
stopped being. Moved to `resembles`, with a note in the test saying the
generic arm's example has to be a word no roster claims and that the next
roster will take the test with it again.

**Not measured.** Three string comparisons on a refusal path. Nothing here is
near a budget and nothing was timed.

## What this does not do

**Sixteen inputs is not the grammar.** The probe was written by listing what
a person coming from Postgres, MySQL or SQLite would type, which is the same
kind of guess `REACHED_FOR` is and has the same ceiling: a seventeenth input
with the same weakness is entirely possible and nothing here would find it.
A generative version — every `Kind` in every position — would find the shapes
nobody thought of and says nothing about whether a *sentence* is helpful,
which is the whole question.

**`kind <=> 'a'` is still confusing and was left alone.** MySQL's null-safe
equality lexes as `<=` then `>`, so the refusal is *expected a value, found
`>`* — a message about the second half of an operator the reader thinks is
one token. Fixing it means the lexer knowing about operators it does not
have, which is a bigger change than the three here and for a rarer input.
Recorded rather than done.

**`SIMILAR TO` gets the regular-expression hint**, because `similar` is in
`REACHED_FOR`. SQL's `SIMILAR TO` is a pattern language closer to `LIKE`
than to a regular expression, so the hint points at `~` where `LIKE` might
serve better. Left as it is: the roster was always "a list of guesses" and
this one is defensible either way, but it is a guess that is now recorded as
one.

**No guard stops the next message having the shape.** The review was a
reading, the fixes are three literals, and the thing that would catch a
fourth is somebody running the probe again. The probe itself was deleted
after it had answered — it is sixteen hand-chosen strings, which is a
fixture pretending to be a test — and the four cases worth keeping are in
`the_three_refusals_that_named_what_they_found` instead.
