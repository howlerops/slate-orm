# The SQL comparison list is the parser now, and mutating it caught a sentence I had just written

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `crates/slate-sql/src/sql.rs`, `crates/slate-wasm/tests/sql.rs`,
  `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** fix

## What changed

`COMPARISONS` in `crates/slate-sql/src/sql.rs` is the parser's comparison
vocabulary — eleven spellings, ten operators — and `comparison_tail` is a loop
over it where it was an eleven-branch `else if` chain.

`filter_strategy` in `crates/slate-wasm/tests/sql.rs` derives its operators from
that constant instead of listing them, with two rosters for what it cannot
generate (`NOT_GENERATED`, `STRING_ONLY`) and a test holding both to the parser.

## Why

`ledger/2026-09-21-contains-in-the-sql-front-end.md` left it `open`:

> **The SQL operator list has the weakness the wire's had.** `filter_strategy`
> in `tests/sql.rs` names its operators by hand, and nothing holds that list to
> the parser. … Unlike `Expr`, there is no enum to pin it to; closing it
> properly means giving the parser a vocabulary constant, which is a change to
> production code for a test's benefit and was not made here.

The entry named the fix and declined it on that cost. The cost is not what it
looked like: **driving the parser from the constant removes a list rather than
adding one.** There were two and a half — the `else if` chain, the strategy's
hand-written vector, and the renderer's spellings — and an operator added to the
parser and to neither of the others was invisible. Now the parser has no second
list and the strategy is computed from the first.

## Alternatives rejected

**Keep the `else if` chain and add a constant beside it.** That is the shape the
original entry priced, and it is the worse one: two lists that must agree, with
a test to check they do. A loop over the table cannot disagree with itself.

**A `filter` over spellings instead of `NOT_GENERATED`.** Cheaper and it drops
operators silently. `matches` is excluded because `value_for` makes plain
strings and a regex literal would test the regex parser's refusal rather than
this round trip — a reason worth writing down, and a roster is where a reason
fits. Both rosters are held to `COMPARISONS`, so a name that stops being an
operator is reported rather than quietly excusing nothing: the rot
`ledger/2026-09-29-the-skip-list-that-excused-nothing.md` is about, met for the
second time today.

**An enum rather than `&'static str` operators.** It would be better typed and
it is a much larger change: `FilterSpec.op` is a `String` all the way to the
wire, so an enum means a conversion at every boundary. The string is what the
protocol carries; the table's job is to stop the *list* drifting.

## Evidence

`cargo test -p slate-sql -p slate-wasm --no-fail-fast`: **134 passed**, 0
failed, across eight binaries — 72 in `slate-sql`, 62 in `slate-wasm`.

Three mutations, record
[`ledger/mutations/20260929T215358-crates-slate-sql-src-sql-rs.json`](mutations/20260929T215358-crates-slate-sql-src-sql-rs.json):

| mutation | outcome |
| --- | --- |
| the parser loses an operator the vocabulary still names | caught, `a_spec_rendered_as_sql_parses_back_to_itself` and `every_excluded_operator_is_one_the_parser_has` |
| a keyword is matched as punctuation, so `likelihood` starts a `LIKE` | caught, four tests |
| the longer symbol is tried after the shorter one | **survived, and correctly** |

### The survivor corrected a sentence I had written minutes earlier

The new constant's doc comment said:

> The order is load-bearing, which is why this is a slice and not a map: `<=`
> must be tried before `<`, or `a <= 1` matches `<` and then fails on `= 1`.

I mutated the order to demonstrate it. Every suite stayed green. The reason is
one function away: `lex` emits `<=`, `>=`, `!=` and `<>` as **single
two-character tokens**, so `eat_symbol("<")` can never match the front of a
`<=`. Prefix ambiguity is the lexer's problem and it was solved before this
table existed.

The comment now says that, and the case carries `expect_survivor` with the
reason, so the mutation is a live assertion that the lexer still pairs them: if
`lex` ever stops, the case fails for being *caught*. The order is kept anyway,
and the comment says why — it is the lexer's property, not this table's, and a
symbol added here that `lex` does not pair up would make the order load-bearing
again without anything saying so.

**This is the second false claim of mine that a mutation caught today**, after
a witness needle that occurred twice. Both were written with confidence in the
same hour as the code they described.

### And an operational finding about `mutate.py`

Editing the file *while a run holds it* loses the edit. The run saves a copy at
the start and restores it in a `finally`; my correction went in during the run
and was silently reverted by the restore. I found it by grepping for my own
sentence rather than assuming, which is the only reason it is not in this
commit as the wrong text. The tool is behaving exactly as documented — the
`finally` is what
`ledger/2026-09-29-...` and `mutate.py --help` both advertise — and the rule
that follows is: **do not edit a file a mutation run is holding**, and check
after one that your own edits are still there. It cost two restores here
before the corrected comment stayed.

## What this does not do

**`IN` is not in the table.** `in_filter_strategy` is separate because `IN`
takes a list rather than one literal, and `comparison_tail` returns a single
value. An `IN` added to `COMPARISONS` would not parse; nothing says so except
this paragraph.

**The renderer's spellings are still its own.** `filter_strategy` now agrees
with the parser about *operators*; the SQL text a test renders comes from the
renderer's own match, which panics on an operator it has no spelling for. That
panic is what pins renderer to strategy, and it is a panic rather than a
compile error.

**Nothing checks that `lex` pairs exactly the symbols `COMPARISONS` needs
paired.** The `expect_survivor` above asserts the pairing holds for `<=`
specifically, by way of the order being irrelevant. A new two-character symbol
added to the table and not to `lex` would be caught by a parse test if one
covered it, and there is no rule that one must.

**The two rosters are held to the parser, not to `value_for`.** `STRING_ONLY`
says which operators need column 2; nothing checks that column 2 is still the
string column. Renaming the fixture's columns would leave it silently wrong.
