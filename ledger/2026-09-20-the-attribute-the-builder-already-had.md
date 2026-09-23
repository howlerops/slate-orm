# The attribute the builder already had

## What changed

`#[record(soft_delete)]` on a field of a `#[derive(Record)]` struct now makes
that column the table's retirement stamp, the way `#[record(created_at)]`
already makes one a managed timestamp. The macro collects the column name,
refuses a second one, and passes the name to `TableBuilder::soft_delete`.

`crates/slate-orm/tests/soft_delete_derive.rs` — five cases. Two doctests on
the derive, one of them `compile_fail`.

## Why

`ledger/2026-09-19-a-row-that-is-gone-but-still-there.md` shipped soft delete
as a first-class convention and closed by admitting the one surface it had not
reached: *"`#[derive(Record)]` has no attribute for it, so the Rust library
declares it through the builder only."*

That is the surface most likely to be read as *how you declare a table in
Rust*. Everything else about the feature was reachable from it — a partial
index over `deleted_at`, the nullable column itself — except the declaration
that makes any of it mean anything. A reader deriving a table and looking for
soft delete would have found the attribute list, not found it, and concluded
the feature did not exist.

The work is small because the schema layer already does all the checking. What
was missing was six lines of macro and the decision about where the rules live.

## Alternatives rejected

**Infer it from a column named `deleted_at`.** Zero attributes, reads
beautifully in an example, and wrong: it would retire rows in any table that
happened to have a column of that name for its own reasons — an audit log
recording *when someone else's* record was deleted is the obvious one. A
convention that fires on a name cannot be opted out of. The test file pins the
negative half with a `Plain` struct whose `deleted_at` is an ordinary column.

**A struct-level `#[record(soft_delete = "deleted_at")]`,** matching how
`tenant_column` is spelled. Rejected because it writes the column name a second
time, and a name written twice can be misspelled — whereas marking the field
means the compiler has already agreed the column exists. The builder needs a
name, so the macro derives one from the marked field instead of asking for it.

**Re-check the schema rules in the macro** — that the column is a nullable
`i64` — so the error arrives at the attribute rather than from a panic inside
`table()`. Tempting, because the macro has the `Type` and could say something
precise about the span. Rejected because those two rules already live in
`TableBuilder::build` with a paragraph each explaining *why*, and a second copy
in a second crate is two things to keep in step. The two `should_panic` tests
assert the refusal arrives; where it is raised is the schema layer's business.

**Let a second `soft_delete` win silently.** That is what the builder would do
— it takes one name and keeps the last. Which field wins would then be decided
by field order, which is not a decision anybody made. Refused in the macro,
with the span on the second field, because this is the one rule the schema
layer *cannot* see: by the time it has a name, the other one is gone.

## Evidence

**Three mutations of the macro, each caught by a named test**, restored and
re-verified:

| mutation | caught by |
| --- | --- |
| the attribute is parsed and the builder call never emitted | `the_attribute_reaches_the_table`, `a_delete_retires_the_row_rather_than_erasing_it` |
| the builder call names the *first* field's column instead | all three behavioural tests |
| the duplicate check removed | the `compile_fail` doctest |

The third is the one worth spelling out. A `compile_fail` doctest passes when
the code fails to compile *for any reason*, so on its own it proves nothing
about the check it claims to test. Removing the duplicate check turned it red —
`test crates/slate-orm/src/lib.rs - Record (line 89) - compile fail ... FAILED`
— which is what makes it a test of that check rather than of a typo.

**The retirement is observed, not assumed.** `a_delete_retires_the_row_rather_
than_erasing_it` deletes a row, confirms an ordinary `get` no longer returns it,
and then reads it back through `include_deleted` — so "gone" and "retired" are
distinguished. `a_table_without_the_attribute_still_deletes_for_real` is the
control: without it, "the row is gone" says nothing, because a hard delete looks
identical from an ordinary read.

**Suites:** `cargo test -p slate-orm -p slate-derive --no-fail-fast` — 18 test
binaries, all ok. Doctests 13 of 13. `sh scripts/check.sh` — 20 of 20.

**One thing removed rather than kept.** The first version carried a
`soft_delete: bool` on `FieldSpec` as well as the collected name, and the
compiler said the field was never read. Deleted rather than `#[allow]`ed: two
representations of one fact is how they disagree later.

## What this does not do

- **No `restore`.** Un-deleting is still an ordinary update that writes null
  back into the column, and nothing in the ORM names that operation. Unchanged
  from the entry this closes, and the larger of the two items it left.
- **No reaper.** Nothing removes retired rows on a schedule; `purge_deleted`
  exists and nothing calls it periodically. Also unchanged.
- **Nothing measures the cost.** Every read of a soft-deleting table carries an
  extra `IsNull` conjunct. The original entry said this was unmeasured and it
  still is — adding a way to declare the column does not change what the column
  costs, and I did not take the opportunity to measure it.
- **The attribute is Rust-only, which is correct but worth saying.** A client in
  Go, Python or TypeScript learns about soft delete from the catalog the server
  publishes, not from a derive; this changes nothing for them.
- **No compile-fail case for `soft_delete` on a field that is also `pk`.** The
  combination is already impossible — a key column may not be nullable and a
  soft-delete column must be — so the refusal comes from `NullablePrimaryKey`,
  which the schema layer's own suite covers. Named because I looked for it, not
  because it is missing.
