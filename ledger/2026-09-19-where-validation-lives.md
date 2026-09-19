# Where validation lives

## What changed

`docs/validation.md`, the design note `orm-comparison.md` has been asking for,
and the two places in that document which asked. No code.

## Why

The comparison document refuses to build validations until somebody answers
"where does validation live when three clients in three languages share one
catalog?", and guesses the catalog. The guess is right and incomplete, and the
gap it leaves is the reason nothing could start: *the catalog already has
this*. `CHECK` is a declarative constraint, declared as a predicate string,
parsed into the same `Expr` the planner uses. Anybody starting the feature
would have spent their first day discovering that and then had to decide what
they were actually building.

So the note's job turned out to be subtraction. What is missing is not a rule
language but three properties of the one that exists: a check cannot name the
column it is about (the refusal says only which check on which table), it stops
at the first failure rather than collecting them, and it is not in
`--print-schema`, so no client can evaluate it and no generated type can
reflect it. Those are the three things a form needs, and each is small on its
own.

Writing it also produced a finding that reading would not have. The first draft
claimed the common validations were expressible and used
`title matches '^.{1,80}$'` as the example. Running it against
`slate-serverd --check` answered ``expected a comparison after the column,
found `matches` ``. `Expr::Matches` exists in the kernel — a real regular
expression, added for the last ClickBench query — and the daemon's predicate
language has no keyword for it. Format validation, the most common kind, is
reachable only through `LIKE`. That is a fourth gap, it is a keyword in one
parser over a node that already exists, and the note would have shipped
asserting the opposite.

## Alternatives rejected

**Build it instead of writing this.** The document asks for the note first and
the reason holds: three of the four gaps are in different subsystems (the error
type, the write path, `--print-schema` and codegen) and picking them off in the
wrong order gets client-side evaluation before the rules are worth publishing.

**Design a validation DSL beside `CHECK`.** What the gap table's phrasing
suggests, and it would have produced a second predicate language in the catalog
for rules the first one can mostly express. The rejected alternative in the
timestamps entry was the same shape — widening `DEFAULT` to hold an expression
— and the same answer applies: one expression language, or none.

**Recommend lifecycle hooks along with validations.** They are listed in one
gap-table row and are a different mechanism: a validation is a pure question
about a row, a hook is arbitrary code with effects, and there is nowhere to run
arbitrary code that Rust, Python, Go and TypeScript share. The note recommends
refusing them and says where their real uses went instead — managed columns,
computed columns, the request log, and `transact`.

**Leave uniqueness and cross-table rules as future work.** Rejected as
dishonest by omission: a predicate that issues reads has unbounded cost on the
write path, so "no more than ten per account" has no answer here. The note says
that rather than listing it as not-yet-done.

## Evidence

The three predicates in the note were run through `slate-serverd --check`, not
transcribed from the parser: `status in ('draft', …)`, `title like '_%'` and
`discount <= price` validate; the regex form does not, and the note quotes the
error. The single-quoting rule in the note is there because the first attempt
got it backwards and the parser said so.

The claim that clients cannot see constraints is `--print-schema` output on a
table with three checks: the per-table keys are `columns`, `id`, `indexes`,
`name`, `primary_key`, `schema_version`, `tenant_column`. No checks, no foreign
keys.

The claim that a refusal names no column is `slate-schema/src/error.rs:455`:
`"row violates check `{check}` on table `{table}`"`.

No code changed, so no mutation testing applies. The weakness of this entry's
evidence is that a design note's recommendation cannot be tested at all — what
was checked is every factual claim it makes about the current system, and not
whether the recommendation is good.

## What this does not do

It does not build any of the four things it recommends, and does not schedule
them. It does not settle whether `message` should be a sentence or a
translation key, and it does not settle how a client-side regular expression
would avoid disagreeing with the server's — deferred rather than solved, and
the note argues that ordering the parser keyword before the publishing work
keeps them apart.

It does not ask whether anyone wants this. The note says so in its last
paragraph: the gap exists because seven other ORMs have the feature, which is a
reason to have an answer and not a reason to build one.
