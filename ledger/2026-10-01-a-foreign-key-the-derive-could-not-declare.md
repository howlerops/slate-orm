# A foreign key the derive could not declare, and the lock it would have deadlocked on

- **Date:** 2026-10-01
- **Author:** Claude Code (session: finish the backlog, the docs and the examples)
- **Touches:** `crates/slate-derive`, `crates/slate-orm`,
  `crates/slate-serverd/src/lang/pred.rs`
- **Kind:** feature

## What changed

`#[derive(Record)]` takes a `foreign_key(...)` attribute, on the struct or on
a field:

```rust
#[derive(Record)]
#[record(table = "invoices", id = 2)]
#[record(tenant = "tenant_id")]
#[record(foreign_key(
    name = "invoices_account",
    parent = Account,
    column = "tenant_id",
    column = "account",
    on_delete = cascade,
))]
struct Invoice { /* … */ }
```

The parent is a **type**, not a table name. A key written on a field
references that field, resolved after any `rename`. `on_delete` is `restrict`
unless it says `cascade`, and any other word is a compile error naming the two
that work. So are a `column` the struct does not have, a column named twice in
one key, and two keys sharing a name.

`Record` gains a provided `table_id()`, which the derive overrides with the
literal from `#[record(id = N)]`. That is not a convenience — see below.

Also here, because CI was red on it: a trailing blank line at the end of
`crates/slate-serverd/src/lang/pred.rs`. The `formatting` job failed on runs
541, 543 and 544 for that one character.

## Why

`ledger/2026-10-01-a-check-the-derive-could-not-declare.md` closed the same
gap for `check` and recorded this one in its own caveat:

> **It does not declare a foreign key.** `TableBuilder::foreign_key` is the
> other constraint the derive cannot express, and it has the same shape of gap
> and no caveat recording it.

The shape is the same and so is the cost. `Record::table()` is what every
write hands the store, so a table that needs a constraint the derive cannot
express has to be hand-written through `TableDef::builder` — and then the
struct and the schema are two things that can drift, which is the entire
reason the macro exists.
`ledger/2026-09-30-an-application-written-against-the-rust-surface.md` priced
that workaround in `examples/helpdesk`: a newtype, plus a test holding the two
shapes together.

A foreign key is also the constraint most likely to be wanted. `check` is a
rule somebody chooses to write; a reference between two tables is in almost
every schema, and the one surface that reads as *how you declare a table in
Rust* could not say it.

## The lock this would have deadlocked on

The parent has to become a `TableId`, and the obvious way to turn a type into
one is `<Parent as Record>::table().id()`.

That code runs **inside `Self::table()`'s own `OnceLock` initialiser**. For
`employee.manager_id -> employee` — the ordinary org chart, not an exotic
shape — `Parent` is `Self`, so `get_or_init` is re-entered from inside itself.
`std`'s `OnceLock` blocks rather than panicking, so the symptom is a hang with
no output at all.

That is why `Record::table_id()` exists: the id is a literal in the attribute,
so the derive can write it out and nothing has to be built to read it. It is
a **provided** method defaulting to `Self::table().id()`, so the hand-written
impls this trait already supports keep compiling; the one shape that would
still recurse is a hand-written impl referencing itself through a derived
foreign key, and that cannot arise because the attribute is the macro's.

This was demonstrated rather than reasoned about — see Evidence.

## Alternatives rejected

**`parent_id = 2`, a raw table id.** No new trait method, no recursion, four
characters. Rejected because the table id is already written down in the
parent's own `#[record(id = N)]`, and a second copy is a second place for it
to be wrong — with no compiler anywhere in the loop. The failure is a key
pointing at a table that exists and is not the one meant, which
`Catalog::validate_foreign_keys` catches only if the widths or types happen to
differ. `has_many` settled this argument for relationships and it does not
reopen here.

**A `const TABLE_ID: TableId` instead of a method.** Cleaner at the use site
and strictly better for the compiler. Rejected because an associated const
with no possible default is a required item, and adding a required item to a
public trait breaks every hand-written `impl Record` — which this trait's own
doc comment says are supported and are "the same amount of work the macro
does". A provided method costs a function call that is inlined away and
breaks nothing.

**Defaulting `on_delete` to `cascade`.** Would make the common parent-child
case one word shorter. Rejected on the direction of the mistake: forgetting
`restrict` means a delete is refused and somebody notices immediately;
forgetting `cascade` means a delete quietly takes rows with it. The builder
already defaults to `Restrict` and this does not second-guess it.

**Inferring the referencing columns from the parent's primary key.** The macro
can see the parent's type, so it could in principle match column names. It
cannot: it sees a `syn::Type`, not the parent's `TableDef`, which does not
exist until that type is expanded and built. Even given it, name matching is
the inference `ledger/2026-09-19-a-check-that-names-its-field.md` rejected for
`column` — right until two tables share a column name, then wrong with no way
for the author to tell. The field-level default is the version of this that is
not a guess: it reads the attribute's *position*, which the author chose.

**Accepting a column list in any order and sorting it.** The order is the
parent's primary key order and a reversed pair is a reference to the wrong
row. Sorting would need the parent's key, which is the same thing the
inference above cannot have. Left as written order, with the order asserted by
ordinal in a test rather than by length — a reversed pair has the right
length.

**Refusing a self-reference.** It would have made `table_id()` unnecessary.
Rejected on sight: `employee.manager_id`, `comment.reply_to`,
`category.parent_id`. `crates/slate-kernel/tests/constraints.rs` has had a
self-referencing cascade since the constraint work, so refusing it in the
derive would have made the macro weaker than the layer under it.

## Evidence

**Seven tests**, in `crates/slate-orm/tests/derive_foreign_keys.rs`, and four
new `compile_fail` doctests on the derive in `crates/slate-orm/src/lib.rs`.

**Thirteen mutations of the macro, across five runs.** Listed rather than
totalled, because three runs record a survivor and the survivor is the point:
each was a missing test, written, and caught by the run after it. The records
are
`ledger/mutations/20261001T145706-crates-slate-derive-src-lib-rs.json`,
`ledger/mutations/20261001T145741-crates-slate-derive-src-lib-rs.json`,
`ledger/mutations/20261001T145757-crates-slate-derive-src-lib-rs.json`,
`ledger/mutations/20261001T145834-crates-slate-derive-src-lib-rs.json` and
`ledger/mutations/20261001T145857-crates-slate-derive-src-lib-rs.json`.

| run | mutation | outcome |
|---|---|---|
| `…T145706…` | no foreign key reaches the builder | caught, 4 tests |
| `…T145706…` | `on_delete` is never emitted | caught, 3 tests |
| `…T145706…` | the columns are emitted in reverse | **survived — not a change**, see below |
| `…T145741…` | the referencing columns are reversed | caught, 4 tests |
| `…T145741…` | a field-level key resolves to the field ident | `NOTHING RAN` — the compiler, see below |
| `…T145741…` | the column-exists rule never fires | **survived** — the doctest was in a suite this run did not list |
| `…T145757…` | the column-exists rule never fires | caught, the `column = "authr_id"` doctest |
| `…T145757…` | the `on_delete` word roster accepts anything | `NOTHING RAN` — the mutation did not compile |
| `…T145757…` | two foreign keys may share a name | **survived — no test existed** |
| `…T145834…` | the `on_delete` word roster accepts anything | caught, the `on_delete = set_null` doctest |
| `…T145834…` | two foreign keys may share a name | caught, the new duplicate-name doctest |
| `…T145834…` | one key may name a column twice | **survived — no test existed** |
| `…T145857…` | one key may name a column twice | caught, the new duplicate-column doctest |

**Three of those were findings rather than confirmations.**

*Two rules had no test at all* — the duplicate name and the duplicate
column — and both survived, one run apart. The duplicate name is cheap to
argue for: it is the same rule `validate_checks` already has. The duplicate
column is the one worth the paragraph. The right response to its survival
was not to delete it: a key resolved as `(tenant_id, tenant_id)` against a
two-column parent key of matching types passes
`Catalog::validate_foreign_keys` — the widths match and the types match — and
then references a row that is not the one meant. Nothing downstream catches
it, so the rule stays and the doctest is new.

*The first attempt at "the columns are reversed" was not a change.* It was
written as `#(key = key.column(#columns);)* let mut key = key;`, which emits
the same calls in the same order and shadows a binding. It survived, and the
survival meant nothing — exactly the equivalent-mutation trap CLAUDE.md warns
about, met once more in the position it is always met in: the first
formulation. The real one reverses the column list in `parse_foreign_key`, and
four tests caught it.

**The deadlock, demonstrated.** With the `table_id()` override deleted from
the macro, so that the parent's id resolves through `table()`:

```
$ timeout 45 cargo test -p slate-orm --test derive_foreign_keys \
      an_employee_can_reference_its_own_table
     Running tests/derive_foreign_keys.rs (…/derive_foreign_keys-2de6fac)

running 1 test
```

and then nothing — no `test result:` line, killed at 45 seconds. That is the
whole argument for `table_id()`, and it is worth noting that it is **not a
failure mutate.py can score**: a hang is the same empty output as a build that
died, which is the tool's second documented lie and the class
`ledger/2026-09-21-a-hang-is-now-a-named-failure.md` is about. It was run by hand,
bounded, with the restore outside the timeout.

**The sixth mutation scored `NOTHING RAN` for the right reason**, as the
equivalent one did for `check`: resolving a field-level key to the field ident
makes `employees_manager` reference `manager_id`, which the renamed table does
not have, so `validate_foreign_keys` refuses at expansion with its own message
quoted in the run's output. `mutate.py` cannot tell a compile-time guard
firing from ENOSPC.

**Four mutations of the `compile_fail` doctests**
(`ledger/mutations/20261001T145909-crates-slate-orm-src-lib-rs.json`), every
one of which the doctests caught. A `compile_fail` block passes when it fails
to compile for *any* reason, so each
was mutated to remove only the mistake it is about — spell the column right,
make the duplicate name differ, make the duplicated column the second one,
replace `set_null` with `cascade` — and each then compiled, so each doctest
failed.

**Not measured.** Nothing here is about speed. The key is evaluated per write
by the record store exactly as a builder-declared one is, and the macro adds
no evaluation; `table_id()` replaces a `OnceLock` read with a constant, which
is faster and was not timed because nothing is near a budget.

## What this does not do

**No client surface, and no conformance case.** The same gap the `check`
attribute has, stated again because it did not close: a key declared this way
is published by `--print-schema` and reaches Python, Go and TypeScript exactly
as a TOML-declared one does, because both end in the same `ForeignKeyDef` —
but nothing in those clients is tested against a *derived* key, and the daemon
declares its tables in TOML, so no deployed path reaches this attribute.

**The parent's shape is still checked at `Catalog`, not at compile time.**
Column count against the parent's primary key, and the type of each column,
are `Catalog::validate_foreign_keys`'s job and cannot move here: the macro
sees a `syn::Type` and the parent's `TableDef` does not exist yet. So
`column = "tenant_id"` alone, against a two-column parent key, compiles and
fails when the catalog is built. That is the same moment the TOML loader
fails, which is the floor rather than a regression.

**`#[record(tenant = ...)]` and a non-tenant parent is refused by the
catalog, and the message says so from the wrong layer.** `Catalog` returns
`CrossTenantForeignKey`, which is correct and arrives at `Catalog::from_tables`
rather than on the attribute. The macro could check it — it knows this
struct's tenant column — but not the parent's, which is the same blindness as
above. Met while writing the tests, which is how it is recorded here rather
than guessed at.

**One parent shape, two key shapes.** The tests cover a composite key into a
tenant-scoped parent and a single-column self-reference. Nothing exercises a
key into a parent whose primary key is three columns, a key on a decimal or a
string column, or two keys from one table into two different parents.
