# The comparison table became the parser this morning, and the lexer kept its own copy of half of it

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-sql` (`src/sql.rs`, `tests/front_end.rs`)
- **Kind:** fix

## What changed

`lex` decides which two-character runs are one token by asking `COMPARISONS`
instead of by a hand-written `matches!("<=" | ">=" | "!=" | "<>")`. A new test,
`every_comparison_the_table_declares_parses_as_the_operator_it_names`, runs a
query for each of the eleven rows and asserts the operator it lands on.

## Why

`ledger/2026-09-29-the-parsers-vocabulary-is-the-parser.md` made
`COMPARISONS` the single list `comparison_tail` loops over and the wasm
crate's property strategy generates from. It then recorded what it had not
done:

> **Nothing checks that `lex` pairs exactly the symbols `COMPARISONS` needs
> paired.** [...] A new two-character symbol added to the table and not to
> `lex` would be caught by a parse test if one covered it, and there is no
> rule that one must.

That is not a small residual, because the pairing is the reason the same entry
could withdraw its claim that the table's *order* matters. `eat_symbol("<")`
cannot match the front of a `<=` only because `lex` already emitted `<=` as
one token. A twelfth row spelled `=~` or `!~` added to the table and not to
the `matches!` arm would lex as two symbols, `eat_symbol` would match the
first character against some other row, and the failure would be a wrong
operator rather than a parse error.

So the table was the parser's vocabulary for `comparison_tail` and for the
generator, and not for the lexer — one list with two readers and a third
that had a copy.

## Alternatives rejected

**Test the pairing instead of deriving it.** A test asserting `lex("a <= 1")`
yields three tokens would pin today's four and say nothing about the fifth,
which is the exact shape of the caveat. Both were written here — the
derivation, and a test over every row — but if only one were possible the
derivation is the one worth having: it makes the drift unrepresentable rather
than detectable.

**Give `Comparison` a `pairs` flag.** Explicit, and a third list: the flag
would have to be kept true for every two-character spelling and false for
every one-character one, which is `spelling.len()` written out by hand. The
data already says which spellings are two characters.

**Have `lex` take the table as a parameter.** Tidier for testing — a case
could hand it a table with a fifth symbol — and `lex` is a free function
called from four places that would all have to thread it. `COMPARISONS` is a
`const` in the same module; the coupling is the point rather than a
dependency to invert.

## Evidence

`cargo test -p slate-sql --no-fail-fast`: **69 passed, 0 failed**, up from 68.
`cargo test -p slate-wasm --no-fail-fast`: 154 passed across seven binaries,
unchanged — the generator reads the same table and the browser path lexes the
same way. `cargo test -p slate-serverd --no-fail-fast`: 108 passed across
eight binaries, unchanged. `cargo clippy --workspace --all-targets`: clean.

Three mutations, record
`ledger/mutations/20260929T231008-crates-slate-sql-src-sql-rs.json`:

| mutation | outcome |
| --- | --- |
| `lex` reverts to a hand-written arm missing `<>` | caught, by the new test and two others |
| `lex` pairs every two-character run, not only the table's | caught, four cases |
| the `<>` row is respelled `=~` | caught — the new spelling lexes and parses, and the tests that write `<>` fail |

The first is the caveat's own scenario and the new test is named in its
failure list, which is what says the test covers the thing rather than the
thing happening to be covered.

**A mutation survived and was not a change**, which is worth recording
because it removed a line. Its run is
`ledger/mutations/20260929T230917-crates-slate-sql-src-sql-rs.json`, against
the first draft. The draft read
`.any(|one| !one.keyword && one.spelling == two)`, and deleting the
`!one.keyword` filter left every suite green. It cannot fire: this branch of `lex` is reached
only when the character is punctuation — a word was consumed by the alphabetic
arm far above — and a keyword spelling is matched by `eat` as a whole word, so
it is alphabetic by construction and `two`, exactly two characters, can never
equal one. The filter is gone and the reasoning is in the comment, which is
the call `check_handlers.py`'s `unscanned` already made about a dead
`is_dir()` guard: a dead safety check is worse than none, because the next
reader weighs a hazard nobody is running.

## What this does not do

**Three-character operators are still impossible.** `lex` looks at two
characters, so a spelling like `!==` or `<=>` added to the table would pair
its first two and leave the third as its own symbol. Nothing refuses such a
row — the derivation makes `lex` agree with the table for the lengths it
handles, not for every length a row could have. A `debug_assert` on
`spelling.len() <= 2` for non-keywords was considered and left out: it would
fire at run time in debug and never in release, which is the wrong place for
a fact about a `const`.

**The test picks a column by operator name.** `like`, `ilike`, `matches` and
`contains` get the string column; everything else gets `year`. That mapping
is a fourth hand-written list, smaller than the one removed and with the same
weakness — a twelfth string operator added to the table would be tested
against an integer column and fail for a reason that is not the one under
test. It fails loudly rather than silently, which is why it is acceptable and
not why it is right.

**Nothing holds the renderer to the table.** That is the sibling caveat in the
same entry and it is untouched here: `crates/slate-wasm/tests/sql.rs` matches
on the operator and panics on one it has no spelling for, pinning the two at
run time rather than at compile time.
