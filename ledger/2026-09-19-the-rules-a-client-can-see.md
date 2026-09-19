# The rules a client can see

## What changed

`--print-schema` publishes `checks` and `foreign_keys`, which it never has.
`scripts/codegen.py` generates from them: every table's constraints as data in
all three languages, and — where a check says `status in ('draft', 'live')`
over a string column — a narrowed field type, `Literal["draft", "live"]` in
Python and `"draft" | "live"` in TypeScript.

The demo schema gained its first `CHECK`, so the generated files carry a real
one rather than an empty list nobody has looked at.

## Why

The third of the validation note's recommendations, and the one it called the
largest. Checks and foreign keys are deliberately outside the schema
fingerprint — the migration refusal says so — and that exclusion is exactly why
they had to be published: a client cannot restate what it cannot see, so a rule
that existed only in the catalog could not be shown beside a field, generated
from, or mapped back from a refusal without parsing prose.

## The part that is not built, and why

The note's headline for this item was "a client that holds the predicates can
refuse a bad row without a round trip". **That is not built and should not be,
yet.**

Evaluating a predicate client-side means an expression evaluator in Python, Go
and TypeScript: three more implementations of `lang/pred.rs`. This session has
now twice paid for the cost of keeping two statements of one rule in step — the
withdrawn regex claim was a note disagreeing with a parser, and the note's own
open question is how three regex engines would ever agree on one pattern. Three
evaluators would be that failure by construction, and the failure mode is the
worst available: a client that accepts a row the server refuses is an annoyance,
and a client that *refuses a row the server would accept* is a bug nobody can
diagnose from the server's logs, because the request never arrives.

So what ships is the rule as **data**. Enough to render it, generate the easy
shape as a type, and map a refusal to a field. The server stays the only thing
that evaluates anything, which is what the note said it wanted anyway.

## Alternatives rejected

**Parse the predicate properly in `codegen.py`.** The enum matcher is a regular
expression over `column in ('a', 'b')` and recognises nothing else — not
conjunctions, not nested parentheses, not a bare column in the list. A real
parser would narrow more fields and would be the second implementation argued
against above. All-or-nothing is the point: a narrowing that is sometimes right
and sometimes silently absent is worse than one obviously limited to a shape
you can see. A test walks four predicates that *contain* an `in` and asserts
none of them narrows.

**Narrow numeric `in` checks too.** `priority in (1, 2, 3)` is a range someone
wrote as a set, and a union of numeric literals says less about arithmetic than
`int` does.

**A named Go type with a const block.** `type PostsStatus string` plus
constants looks like the other two languages and is not: Go converts any string
into it, so it would read as a check and enforce nothing. A `[]string` of the
allowed values is honest about being data.

**Publish the parsed expression as JSON instead of the source text.** More
precise and it commits the output to the shape of an internal type, which then
cannot change without breaking every generator. The source string is what a
human wrote and what a human reads.

**Give `CheckDef` a real `Display`.** The predicate is a Rust function and
cannot be printed, so the text has to be carried alongside. `with_source` is
explicitly *not* authoritative — the predicate is what runs — and says so.

## Evidence

Nine mutations of `codegen.py`, all caught:

| mutation | caught by |
| --- | --- |
| an `in` over a number narrows too | `test_an_in_check_over_a_number_narrows_nothing` |
| a partially-understood predicate narrows anyway | `…does_not_fully_understand_narrows_nothing` |
| the enum never reaches the Python field | `…becomes_a_type_in_the_two_languages…` |
| the enum never reaches the TypeScript field | the same |
| the Go allowed-values slice is never emitted | the same |
| TypeScript publishes `""` for a missing column | `…publishes_the_absence` |
| an empty check list still emits a block | `…emits_no_check_block` |
| `Literal` is always imported | `…becomes_a_type…` / `…no_check_block` |
| `Literal` is never imported | the same pair |

Two of these are worth the space. The numeric one **survived its first test**:
the assertion used `priority in (1, 2, 3)`, whose bare numbers fail the
*literal* pattern long before the column's type is looked at, so deleting the
type guard changed nothing. The test now also uses `priority in ('1', '2')`,
which parses cleanly and is refused only because the column is an `i64` —
the case the guard exists for.

The `Literal` import is the third time this session that a conditionally-needed
import has been the bug. `Optional` did it, then `Literal`: emitted
unconditionally, unused on a catalog with no enum, stripped by `ruff --fix`,
and drift on the next `--check`. It is now emitted only when some table
narrows, with a mutation in each direction.

Verified as CI verifies it: `ruff check` and `ty check` against a clean
virtualenv, `gofmt -l`, `go build`, `go vet`, `tsc --noEmit` on the client and
the backend, `codegen.py --check` reporting no drift, and
`cargo test -p slate-serverd --bins` at 169 passing.

## What this does not do

No client-side evaluation, as above.

Foreign keys are published and nothing generates from them. A `parent` table id
and the referencing columns are in the output; turning that into a typed
relation in three languages is the obvious next step and is not started.

The demo's one check is weak on purpose — `year > 0`, which every seeded row
satisfies. A demo constraint that rejected the demo's own data would be a trap
for whoever adds the next row, so the generated files demonstrate the
*plumbing* and not an interesting rule. The enum narrowing is therefore proved
only against synthetic catalogs in `scripts/test_codegen.py`; no real schema in
this repository has an enumerated column.

`message` is still a plain string with no translation story, `#[derive(Record)]`
still cannot declare `column` or `message`, and a generated `CHECKS` map is
keyed by check name with no compile-time guarantee that the name matches one
the server will actually send.
