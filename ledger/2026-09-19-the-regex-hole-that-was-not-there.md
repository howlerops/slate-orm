# The regex hole that was not there

## What changed

`docs/validation.md`'s claim that the kernel's regex node is unreachable from a
`CHECK` is **withdrawn**. It is reachable, and was all along. Two tests in
`schema.rs` prove it, a third pins the four operators apart, and the note and
`docs/orm-comparison.md` now say so.

## Why

The note, written earlier today, called this "one hole in the rule language"
and "a prerequisite for any of this being useful". It was the first item on its
own recommendation list. Picking that item up as work is how it was found to be
wrong.

The draft tested `title matches '^.{1,80}$'`, got

```
expected a comparison after the column, found `matches`
```

and read it as "the parser has no regex". The error was about a *word*. The
daemon spells a regular expression the way Postgres does — `~`, `~*`, `!~`,
`!~*` — and `lang/pred.rs` has parsed all four since regex was added for the
last ClickBench query.

Two things in that file would have caught it. Its grammar comment lists
`column ( '~' | '~*' | '!~' | '!~*' ) string` on line 12, four lines above the
sentence *"Every `Expr` variant is reachable, which is the property that makes
this a surface syntax for the kernel's predicates rather than a subset of
them"* — which is the exact claim the note contradicted. And line 841 is a
parser test for `kind ~ '^a'`.

The note was careful about the thing it got wrong — it said, correctly, that
the draft had asserted the opposite and been corrected by running the example.
It ran one spelling, got an error, and stopped. Running a second spelling, or
reading one screen up, was the whole distance to the right answer.

## Alternatives rejected

**Quietly fix the note.** The repository's convention is the opposite —
`docs/correctness.md` and `docs/performance.md` both carry withdrawn claims
with the reasoning intact — and a note whose headline finding silently changed
is worth less than one that shows its own correction. The section is retitled
"The rule language is not the gap — a correction" and says what it said before.

**Add a `matches` keyword as an alias for `~`.** Tempting, because the failing
spelling was a reasonable guess. Rejected: two spellings of one operator is a
grammar with a synonym in it, every future reader has to learn both, and the
one that exists is the one Postgres uses — which is the convention the rest of
this parser follows (`ILIKE`, `IS NULL`, `!~*`). The error message is the thing
worth improving, and it already names what it found.

**Leave the recommendation list at four items.** The prerequisite is gone, so
the list is three, and all three are about reporting and publishing rather than
the rule language. Saying so is most of the value of the correction: it makes
the remaining work narrower and more coherent than the note described.

## Evidence

Three tests in `crates/slate-serverd/src/schema.rs`:

- `a_check_can_be_a_regular_expression` — `kind ~ '^.{1,8}$'` accepts `ok`,
  rejects a 22-character string and rejects the empty one. The length bound is
  what the note said was unreachable, and `LIKE` provably cannot express it.
- `a_check_regular_expression_that_does_not_compile_is_refused` — `kind ~ '('`
  fails at startup. At query time an uncompilable pattern matches nothing,
  which in a configuration file is a check that passes everything.
- `the_four_regex_operators_differ_in_case_and_sense` — `~` rejects `ABC` where
  `~*` accepts it, and the negated pair inverts both.

Four mutations of `lang/pred.rs`, all caught:

| mutation | caught by |
| --- | --- |
| `~` parses as case-insensitive | `the_four_regex_operators_…` |
| `~*` parses as case-sensitive | the same |
| `!~` parses as not negated | the same |
| the pattern is never compiled at startup | `…does_not_compile_is_refused` |

The third test exists **because of a surviving mutation**. The first version
asserted only on `^.{1,8}$`, which contains no letter, so making `~` mean `~*`
changed nothing and every assertion still passed. A test that cannot tell `~`
from `~*` is not testing the operator, only that some regex ran.

## What this does not do

It does not build any of the three remaining recommendations — a check still
cannot name its column, still stops at the first failure, and is still absent
from `--print-schema`. Those are the next three tasks and none of them is
started here.

It does not improve the error message that caused the mistake. `expected a
comparison after the column, found \`matches\`` is accurate and unhelpful; a
suggestion ("did you mean `~`?") would have saved the whole detour, and is not
written. The keyword-alias reasoning above argues against the alias, not
against the hint.

It does not audit the rest of the note for the same class of error. The three
remaining findings were each checked against the code when written, but the
regex one was too, and was wrong — so "checked when written" is now known to be
a weaker guarantee here than it sounded.
