# Relationships in the derive, checked by the compiler

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/orm-essentials`
- **Touches:** `crates/slate-derive/src/lib.rs`, `crates/slate-orm/src/lib.rs`, `crates/slate-orm/tests/derive_relations.rs` (new)
- **Kind:** feature

## What changed

`#[record(has_many(Book, foreign = author_id))]` and
`#[record(belongs_to(Author, local = author_id))]` emit the `Related` impl that
the previous commit's `load_related` consumes. Columns are named by **field
ident**, not by string, so both sides are checked at compile time.

## Why

The previous commit shipped the contract — `Related`, `load_related`,
`related_filter` — and left the declaration hand-written: four lines of `impl`
returning two integers, per direction, per pair of tables. That is a trait
anybody can implement wrongly in a way nothing catches, because `Ordinal(1)` is
a valid answer to every question.

The attribute is sugar over the trait and changes nothing about the loader. What
it adds is that the ordinals stop being written by hand.

**The design decision is that columns are idents, not strings.** A string is
what every ORM uses and it is resolved at runtime against a table the macro
cannot see, so `foreign = "auther_id"` becomes a panic on first use — or, if the
typo happens to name a real column, a relationship over the wrong one that
returns plausible rows forever. An ident is resolved by the compiler:

- the **local** side against this struct's fields, inside the macro, with a span
  on the attribute, and
- the **foreign** side by emitting `Other::COLUMNS.field`, which does not
  resolve unless `Other` derives `Record` and has a field of that name.

`COLUMNS` already existed for exactly this reason (task #8: "writing a filter is
the most common thing a caller does, and `table().ordinal_of("x").unwrap()` is a
panic waiting in otherwise ordinary code"). It turns out to be the mechanism
that makes a cross-type reference checkable, which is more than it was built
for.

## Alternatives rejected

**Strings, as everywhere else.** Rejected above: it moves both errors to
runtime, and one of them is not an error at all but a silently wrong answer.
The cost of the ident form is that a renamed column (`#[record(rename = ...)]`)
is named by its *field*, not its stored name — the same inconsistency
`only_where` and `COLUMNS` already have, and the same justification: the reader
of the attribute is looking at the struct.

**Defaulting both sides from the names.** `has_many(Book)` could guess
`author_id` from `Author` + `_id`. Rejected: a convention that is right most of
the time produces a relationship that is wrong occasionally and silently, and
the failure is a query that returns rows. Two of the four sides are therefore
required outright, and the derive says so with the fix in the message.

**Defaulting a `has_many`'s local column on a composite key** by taking the
first. Rejected, and it is the sharpest case in this change: under
`#[record(tenant = "...")]` the first key column *is* the tenant, so the
"convenient" default is a relationship that matches rows belonging to every
other tenant. It is a compile error naming the fix.

**Defaulting a `belongs_to`'s foreign column** is the one default kept, because
a foreign key points at a primary key and there is nothing else it could mean.
It cannot be resolved in the macro — that table belongs to a type the macro
cannot see — so it is a lookup at first use, and on a composite key it panics
rather than take the first column. A panic and not a `None`, on the same grounds
as the schema build ten lines above it: the answer is the same every run, so it
is a mistake in source code, and widening `Related::foreign` to a `Result` would
put a fallible call in every caller of `load_related` to report something that
cannot vary at runtime.

**Emitting the impl inside the `Record` impl block.** Rejected because a
relationship to a type that is not a `Record` should fail on the relationship,
not take the whole table definition down with it and bury the real error under
twenty others.

## Evidence

Six tests in `crates/slate-orm/tests/derive_relations.rs` and five
`compile_fail` doctests on the `Record` re-export.

The fixture is deliberately misaligned: `Book::author_id` is the **third**
field while `Author::id` is the first, and `Series` has a single primary key at
ordinal **1**. Both exist because a macro that emitted the constant `0`, or that
resolved a foreign name against the struct the attribute is written on, passes a
fixture where the ordinals happen to line up. The mutation table below shows
both of those mutations being caught by exactly those two tests.

Each `compile_fail` doctest was also run as a real compilation and its error
read, because a `compile_fail` passes when the snippet fails for *any* reason —
a typo in the snippet is a green test that checks nothing. All five produce one
error, and it is the intended one:

| snippet | error |
| --- | --- |
| `foreign = auther_id` | `no field 'auther_id' on type 'BookColumns'` |
| `local = auther_id` | `` `auther_id` is not a field of this struct `` |
| `has_many(Book)` | `` `has_many` needs `foreign = <field>` … `` |
| `belongs_to(Author)` | `` `belongs_to` needs `local = <field>` … `` |
| `has_many` on a composite key | `primary key of 2 columns, so has_many cannot default …` |

### The mutation pass

Eight mutations, one survivor, now caught:

| mutation | result |
| --- | --- |
| a named `local` ignores the name and uses ordinal 0 | caught (3 unit + doctests) |
| the `local` default is the first field, not the key | caught: `the_local_default_is_the_primary_key_wherever_it_sits` |
| a named `foreign` resolves against `Self`, not `Other` | caught (does not compile) |
| the composite-key default takes the first column | caught: `defaulting_the_foreign_side_of_a_composite_key_refuses_rather_than_guesses` |
| `has_many` stops requiring `foreign` | caught (doctest) |
| `belongs_to` stops requiring `local` | **survived** → new `compile_fail` doctest |
| the local field is no longer checked against this struct | caught (doctest) |
| a composite key may default its local column | caught (doctest) |

The survivor is worth naming: `belongs_to(Author)` with no `local` would have
silently defaulted to this struct's *primary key*, so `Book` would have been
related to `Author` on `Book::id`. Every other refusal had a doctest and that
one did not, which is precisely the gap a mutation pass exists to find. It was
re-applied after the new doctest to confirm the failure lands there.

The composite-key panic test is derived rather than hand-written, on the second
attempt. The first version copied what the macro emits into the test file, which
tests the copy: the macro could stop emitting it — or emit `[only, ..]` instead
of `[only]`, which is one of the mutations above — and the copy would go on
passing. Declaring the relationship with the attribute and calling `foreign()`
under `#[should_panic]` tests the thing.

## What this does not do

**Still nothing in the SDKs.** Python, Go and TypeScript have no relationship
surface. Unchanged from the previous entry, and still a schema question rather
than a client one.

**No `through` / many-to-many.** A join table is two `belongs_to`s and two
`load_related` calls today, which works and reads honestly about the two reads
it costs. A `has_many_through` would hide a third.

**No cascade and no constraint tie-in.** Declaring `has_many(Book, ...)` does
not create the foreign key constraint that task #61 added, and does not delete
children. The two are independent on purpose: a relationship is a way to fetch,
a constraint is a rule about what may be stored, and a caller may reasonably
want either without the other.

**A renamed column is still named by its field.** Consistent with `COLUMNS` and
`only_where`, and not separately tested here, because it is those features'
behaviour rather than this one's.
