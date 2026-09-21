# A factory that generates rows from the table, because the table already knows

- **Date:** 2026-09-21
- **Author:** Claude Code (agent session)
- **Touches:** `crates/slate-orm/src/factory.rs`, `crates/slate-orm/src/{lib,error}.rs`, `crates/slate-orm/tests/factory.rs`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`slate_orm::Factory` borrows a `TableDef` and produces `Row`s from it. Every
input it needs is already on the table: each column's type, nullability and
`DEFAULT`, whether the store writes it, which columns are the primary key,
which participate in a unique index, and what the `CHECK` constraints are.
`Factory::new(&table).seed(7).rows(1_000)?` is the whole call, and the rows go
straight into `insert_many` under `seeding_context()`.

`set`, `cycle` and `null_for` override a column. `starting_at` moves where the
sequence begins so a fixture can grow. `FactoryError` is a new arm on
`OrmError`, and `CheckDef`, `ForeignKeyDef` and `ReferentialAction` are now
re-exported from `slate-orm` so a caller building a table through this crate
does not need `slate-schema` as a second direct dependency for two types.

## Why

This is the half of the comparison table's seed-data row that was really open.
The entry point was never missing — the commit before this one withdrew that
claim — but nothing generated rows, so a thousand-row fixture was a thousand
lines somebody wrote. The three named tools in that row (Drizzle, Prisma's seed
scripts, FactoryBot) are known for the generator, not for the write.

Generating from the `TableDef` rather than from a Rust struct is what makes it
worth having. A generator over `R: Record` would need a trait the user
implements per type, at which point it has told the user nothing they did not
have to say themselves. The table knows the schema; the struct knows the
fields.

## Alternatives rejected

**A seeded PRNG drawn left to right, the obvious implementation.** Rejected
for three properties it cannot have. `rows(10)` would not be a prefix of
`rows(1_000)`; `row(7)` could not exist without generating rows 0 to 6; and
adding a column to the table would shift every value in every later row,
so a fixture changes wholesale on a change that has nothing to do with it.
Making each value a pure function of `(seed, ordinal, index)` costs one hash
per value and gives all three. `a_row_is_the_same_whether_it_is_generated
_alone_or_in_a_batch` pins it.

**`rand` rather than eleven lines of splitmix64.** `StdRng` documents that its
algorithm may change between minor versions, which for a *fixture* is the whole
problem: a test asserting against generated data would break on a dependency
bump with nothing changed here. Writing the mixer down fixes it forever and
adds no dependency. The cost is that it is not a good general-purpose PRNG,
which does not matter — nothing here is sampling a distribution.

**Honouring a `DEFAULT` on a key or unique column.** Rejected: a default cannot
serve two rows of a unique column, so every batch of more than one row would
collide, and the factory's entire job is batches. Those columns are sequenced
instead and the default is deliberately ignored, which
`a_default_is_used_where_there_is_one_and_ignored_on_a_key` asserts in both
directions.

**Drawing the key randomly rather than sequencing it.** Rejected by the
birthday bound: a drawn `u64` from a small range collides well before a fixture
gets large, and the failure arrives as a `DuplicatePrimaryKey` from inside a
batch a thousand rows deep. Sequencing also extends to unique *indexes*, which
is the case that would otherwise have been missed — the key looks fine and the
batch fails on a secondary index.

**Filling the soft-delete column like any other nullable column.** This is what
the ordinary rule does, and it would mean every generated row arrives already
retired, the fixture reads back empty, and the seed looks like it silently did
nothing. Ruled out by hand rather than left to the general case.

**Generating a vector with a guessed dimension.** A vector's width is not in
the schema. A table of vectors that are all the wrong width looks fine until
the first similarity search, so it is refused with a message that says why.
Same for a `bool` in the key: two rows exhaust it.

**Letting a failing `CHECK` surface from the insert.** Rejected: that error
names the table and the check but not which of a thousand generated rows
produced it, so the caller bisects a batch. The factory runs every check over
every row it makes and refuses at generation time, naming the check, the column
and the `set` call that fixes it.

**Reading the store to find foreign-key parents.** Rejected: it would make
generation `async` and tie it to a transaction, and the caller who generated
the parents already has their keys. `cycle` takes them and wraps, which has the
side benefit that every parent gets children — a fixture where nine of ten
parents have none tests very little.

## Evidence

Sixteen tests in `crates/slate-orm/tests/factory.rs`, all passing; the whole
`slate-orm` suite is green (13 binaries, plus 9 doctests) and
`sh scripts/check.sh` reports 32/32.

The load-bearing test is `a_thousand_generated_rows_insert`, which uses the
store as the oracle: nothing asserts what a plausible value looks like, only
that a real `RecordStore` accepts all thousand — which covers type,
nullability, key uniqueness and every unique index at once. A thousand rather
than ten because the failure it guards is birthday collision, and ten rows
would not find it.

Eighteen mutations run through `scripts/mutate.py`, **sixteen caught**:

| mutation | caught by |
| --- | --- |
| the soft-delete column is drawn | `a_seeded_table_with_a_soft_delete_column_reads_back_full` |
| only the key is sequenced, not unique indexes | four tests, incl. `a_thousand_generated_rows_insert` |
| a `DEFAULT` is honoured on a key | `a_default_is_used_where_there_is_one_and_ignored_on_a_key` |
| `starting_at` is ignored | `starting_at_continues_the_sequence_rather_than_restarting_it` |
| rows are never checked | `a_check_the_generated_rows_fail_names_the_check_and_the_column` |
| a `bool` key is sequenced rather than refused | `a_bool_primary_key_is_refused_rather_than_silently_colliding` |
| a managed column is drawn | `a_managed_column_is_written_by_the_store_not_the_factory` |
| a sequenced `u64` key is constant | four tests |
| `cycle` always takes the first choice | `children_generated_with_cycle_reference_parents_that_exist` |
| `null_for` accepts a non-nullable column | `null_for_is_asked_for_and_is_refused_where_it_cannot_apply` |
| an empty `cycle` is treated as no override | `a_misspelled_column_and_an_empty_cycle_are_both_refused` |
| the vector arm falls through to the wildcard | `a_vector_column_is_refused_and_says_so`, `every_value_type_is_generated_or_refused` |
| the vector refusal borrows the wildcard's reason | the same two |
| only the *first* key column of a composite key is sequenced | `a_composite_key_is_distinct_as_a_whole` |
| only the *last* key column of a composite key is sequenced | the same |

**The last two of those are a defect this found in the code as first written,
and fixing it is why they are catchable.** `ValueType` is `#[non_exhaustive]`
and `slate-orm` is a downstream crate, so the match in `draw` needs a wildcard.
The wildcard returned the same `CannotGenerate` the explicit vector arm did —
so disabling the vector arm changed nothing observable and the mutation
survived. The two refusals are not the same thing: one is a decision about the
schema that `set` answers, the other means `ValueType` grew a variant and this
module did not. They now carry different reasons (`NO_DIMENSION` versus
`NO_GENERATOR`), which is both a better message and the thing that makes the
mutation catchable.

**The two composite-key rows are a second thing mutation testing found, in the
test rather than in the code.** Sequencing *every* column of a composite key is
more than distinctness needs — one injective column makes the tuple distinct —
so both narrowings survived at first. The redundancy is deliberate: a caller
pinning one key column with `cycle` needs the key to stay distinct through the
other, and which column that is cannot be known in advance. Catching it took
two passes:

1. Adding the pinned-`region` case caught the *first*-only narrowing but not
   the *last*-only one.
2. The *last*-only narrowing escaped because the table's other key column was a
   `Uuid`, and a drawn uuid is unique by accident — 200 of them never collide,
   so the tuple was distinct whether or not the rule held. Changing that column
   to a `Str`, whose generator draws from a vocabulary of 256 and therefore
   collides freely at 200 rows, makes the sequencing rule the only thing
   holding the key apart. Both narrowings are now caught.

That is the more useful of the two findings here, because the test looked
correct at every step and was passing for a reason that had nothing to do with
what it claimed to check.

**One survivor, recorded rather than chased:** `NO_ELEMENT_TYPE` swapped for
`NO_GENERATOR`, with `expect_survivor`. `TableBuilder::build` refuses an array
column with no element type — verified by building one and watching it refuse,
not assumed — so the branch is unreachable through any `TableDef` a caller can
construct. It is kept as a refusal rather than an `expect` because the schema
layer's guarantee is one edit away from weakening, and the failure then would
be a fixture full of wrongly-typed arrays rather than a readable panic.

`every_value_type_is_generated_or_refused` is the roster the compiler cannot
be: it drives all ten `ValueType`s through a real column and requires each to
produce rows or a refusal that names the type, with `REFUSED` holding the one
deliberate exception and its reason. A new variant fails that named test rather
than surfacing from inside somebody's seed.

## What this does not do

**No client gets a factory**, and that stays refused rather than open. Seeding
is a superuser write and `seed.rs` designs a superuser write path out of the
wire; a generator on a client would either need one or would be generating rows
it then writes as itself, which is not seeding.

**No `slate-serverd --seed` integration.** The TOML path still takes rows
written out by hand. Generating them there would need a way to say "a thousand
of these" in the file, which is a configuration-language decision this does not
make.

**Plausible is a low bar.** A generated string is two words from a
sixteen-by-sixteen vocabulary, not a name; a generated `f64` is a number under
a thousand. Nothing here knows that a column called `email` should look like an
email. That is the next thing a user will want and it is not built.

**Nothing derives a factory from `#[derive(Record)]`.** The factory works from
the `TableDef`, which a `Record` type has, so `Factory::new(Note::table())`
already works — but there is no `Note::factory()` and no way to say that a
particular field should be generated a particular way from the struct
definition.

**The `CHECK` refusal is not a solver.** It reports the first failing check for
the first failing row and stops. It does not say which columns the check
covers, because a `CheckDef` carries at most one column name and most carry
none, and it cannot suggest a value that would pass.

**It does not know what a column means.** Nothing infers that `email` should
look like an email or that `country` should be a country. The generator reads
the *type*, and the type is all it reads.

**The seed is not part of the fixture's identity.** Two factories over the same
table with the same seed produce the same rows, but nothing records which seed
a stored fixture came from, so a test that finds a bad row cannot recover the
call that made it except by reading the test.
