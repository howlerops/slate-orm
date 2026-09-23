# An additive schema change is applied instead of refused, because the stored schema can tell an append from a retype

- **Date:** 2026-09-21
- **Author:** Claude Code, on `claude/rust-orm-record-layer-gswxlu`
- **Touches:** `slate-kernel/src/migrate.rs`, `slate-kernel/tests/migrations.rs`,
  `slate-serverd/src/main.rs`, `docs/persisting-the-schema.md`,
  `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

`evolution()` classifies a layout difference as `Compatible` or `Incompatible`
and `plan` acts on the answer: an appended column is now a `Step::WidenSchema`
that writes one key, reads no row and rewrites none, where before it was a
refusal. Three things are still refused, each by name — a retype or any other
reinterpretation of an existing column or of the key; a *narrowing*, because
the trailing bytes of every stored row would have no column claiming them; and
an append whose `added_in` is not strictly after the version rows were written
at, which looks additive and is not. `layout_changes` now compares the common
prefix even when the counts differ, which is what makes the first two
distinguishable at all. `--plan` renders the new step with both lists.

## Why

The test pinning the old behaviour was called
`adding_a_nullable_column_is_not_a_migration` and its comment was the reason
this item existed:

> the runner cannot tell an appended column from a retyped one and the safe
> answer to "I cannot tell" is no… **This is the sharpest limitation of the
> fingerprint and it is recorded rather than papered over: a genuinely additive
> change needs a hand.**

The fingerprint is one number and moves identically for both. Storing the
schema (the previous commit) removed the cannot-tell, so the refusal stopped
being justified by anything but inertia. Nothing about the row format had to
change: `decode_row_columns` already reads a row's own written version and
`present_at(column, written_version)` already hands back a default for a column
that did not exist yet. That mechanism was built and then never reachable,
because the only way to get a row into that state was a migration the runner
refused.

## Alternatives rejected

**Leave it refused and document a manual override.** The shape most migration
tools have: a flag saying "I checked, let it through". Cheap, and it moves the
decision to the person least able to make it — the operator has the fingerprint
and the operator cannot tell an append from a retype either. The whole value of
the stored schema is that the *runner* can now answer, and an override would
have been the answer to a question nobody still has.

**Allow any change whose prefix is unchanged, including a narrowing.** One term
shorter, and wrong: `present_at` is what makes an append safe and it has no
counterpart for a removal. Every row written under the wider list still carries
the trailing value and nothing would declare its type. A mutation that made
exactly this change survived the whole suite, and the test that now catches it
exists because of that survival rather than because the case was foreseen.

**Rewrite rows on a widening, the way a `VACUUM FULL`-shaped tool would.**
Correct, straightforward, and it turns a constant-time metadata write into a
full table scan and rewrite. The versioned row format exists precisely so this
is not necessary; paying for it would be admitting it does not work.

**Report the column count and stop, as `layout_changes` originally did.** That
was the existing behaviour and the argument for it is sound *in general* — a
count difference does make per-ordinal comparisons meaningless for a diff
tool. It is wrong here because "two columns appended" and "two columns appended
and column 1 retyped" are precisely the two answers this feature has to
separate, and `zip` stops at the shorter side so every comparison it does make
is between columns genuinely at the same ordinal.

## Evidence

The kernel suite is 26 migration tests, all passing; the whole `slate-kernel`,
`slate-serverd`, `slate-schema` and `slate-orm` suites pass unfiltered.
`sh scripts/check.sh` reports 32 of 32.

Ten mutations, via `scripts/mutate.py`, all now caught:

```
ok   a narrowing counts as compatible          ->  a_table_that_loses_a_column_is_refused
ok   everything is refused again               ->  adding_a_nullable_column_is_not_a_migration_but_it_is_a_new_layout
ok   no stored schema is widened anyway        ->  a_record_with_no_stored_schema_falls_back_to_the_hash
ok   the prefix is not compared when counts differ -> a_retype_is_still_refused_after_an_append_is_allowed
ok   added_in is not checked                   ->  a_column_appended_at_an_already_written_version_is_refused
ok   a non-count change is compatible          ->  a_retype_is_still_refused_after_an_append_is_allowed
ok   an empty retired list is printed anyway   ->  the_plan_names_which_columns_a_widening_adds_and_retires
ok   the added list is never printed           ->  (same)
ok   the two lists are swapped                 ->  (same)
```

**Two survivors, and the second was the more interesting.**

The first was the narrowing, above: every other case in the file either changes
the prefix or adds a column, so nothing pinned the direction. Test written,
mutation re-run, caught.

The second was not in this change at all — it was in the previous commit's
encoder, and four separate mutations survived together:

```
!!  droppedness is not stored:       SURVIVED
!!  the tenant column is never stored: SURVIVED
!!  the scale is not stored:         SURVIVED
!!  the element type is not stored:  SURVIVED
```

The round-trip test that was supposed to cover the format round-trips `users`,
whose two columns are non-null, plain `u64` and `str` with no scale, no element
type, no droppedness and no tenant column — so four of `StoredColumn`'s six
fields and the tenant tag were being written, read and compared against zero on
both sides. What that would have cost is not an unreadable record: it is a
table with a decimal, an array, a dropped column or a tenant column that
migrates once and is **refused on every startup after**, because the decoded
schema disagrees with the one computed from the catalog in a field that never
reached the keyspace. `every_stored_column_field_survives_the_round_trip` uses
a table with one of each, compares against `StoredSchema::of` as an oracle
rather than a list of remembered field names, and asserts the second plan is
empty. All four are caught.

Three of my own mutations in the first run were invalid rather than findings —
adding an `if false` guard inside a `match` arm makes the match non-exhaustive,
so nothing compiled and `mutate.py` reported `NOTHING RAN` rather than a
survival. That is the script's second failure mode working as designed; a
hand-rolled grep for `FAILED` would have scored all three as survivors and
bought three tests nobody needed.

## What this does not do

**It does not widen a column's type**, even where the widening is lossless
(`i64` to `f64`, a decimal gaining scale). Those are reinterpretations of bytes
already written and would need a rewrite or a per-column read-time conversion,
neither of which exists.

**It does not reorder or insert.** A column added anywhere but the end shifts
every ordinal after it, which is a `Column` change at each one and is refused.
That is the right answer and it means "add a column" here really means "append
a column".

**It does not check that the stored schema and the stored fingerprint agree.**
They are written together in one transaction so they should not diverge, and
`persisting-the-schema.md` still lists this as an open suggestion rather than a
design — recomputing the fingerprint from the stored schema at read time would
turn an impossible state into a `Corrupt` refusal. Unchanged by this commit.

**`Step::WidenSchema` has no integration coverage in `tests/plan.rs`**, for the
structural reason the neighbouring `RecordSchema` test already records: every
store that harness builds is written by the same binary, so reaching a widening
would need a store whose stored schema is narrower than the catalog, which one
run cannot produce. Both are covered by unit tests over `render_plan` instead,
and that is weaker — it tests the rendering, not the binary.

**The roadmap table on the site was stale in seven rows, not one**, and fixing
the migrations row meant fixing the rest: codegen, timestamps, arrays, set
operations, seeding and validations had all moved and the table still listed
them as flat gaps. Each row now carries where it stands, checked against
`orm-comparison.md` rather than from memory. Full-text search and window
functions are the two that are genuinely still open.
