# The cross-tenant refusal guarded one constructor, and there were two

- **Date:** 2026-09-20
- **Author:** Claude, checking a caveat written an hour earlier in the previous entry
- **Touches:** `crates/slate-schema/src/catalog.rs`, `crates/slate-kernel/tests/security_probe_cascade.rs`, `docs/security-review.md`
- **Kind:** security

## What changed

`Catalog::insert` now refuses a referential action from a shared parent to a
tenant-scoped child, which until now only `Catalog::from_tables` did. The
schema that security finding 1 declared unrepresentable was representable, and
building it that way brought the cross-tenant cascade back in full.

## Why

The previous entry closed with a caveat: *"The two `RESTRICT`/`CASCADE` names
were merged into one test, and I did not verify the merged test covers
everything the two separate ones did."* Checking it found something larger than
the question asked.

The original probe asserted **runtime** behaviour — tenant A deletes the shared
org, tenant B's rows are destroyed. The replacement asserts the **catalog
refuses the schema**. Collapsing the first into the second is only sound if the
refusal is unavoidable, so: is it?

It was not. `Catalog::from_tables` inserts every table and then calls
`validate_foreign_keys`, and the refusal lived in that second call. Both halves
are `pub`, and `validate_foreign_keys`'s own doc comment said *"call it
yourself after building a catalog with `Catalog::insert`"* — which is advice.
A caller who did not take it got a catalog holding the exact edge finding 1
refuses, with nothing stopping the delete.

Measured, with the old probe's fixture pointed at a catalog built by `insert`:

```
tenant A's delete of a shared parent left []; expected ["b's doc"]
```

Empty. Tenant B's row destroyed by tenant A, which is finding 1 verbatim —
rated *high, cross-tenant data destruction, no read needed* — reached around
the fix that closed it.

**The blast radius is narrower than that sounds, and worth stating precisely.**
In-repo, `Catalog::insert` has exactly one caller: `from_tables`. `slate-serverd`
builds its catalog through `from_tables` (`schema.rs:102`), as do the
clickbench loader, the headbench fixture, the wasm fixture and the seed
command. So no deployment of this repository was exposed. The exposure is the
published surface of `slate-schema`: a library consumer assembling a catalog
incrementally, which the API not only permits but documents.

That is still worth fixing at the severity the finding carries, because the
whole argument for refusing the edge rather than confining the scan was that
the edge *"is not expressible safely by either action"* — and it was
expressible.

## Alternatives rejected

**Make `insert` private.** The complete fix, no runtime cost, no ordering
subtleties, and it has exactly one in-repo caller so nothing here would notice.
Rejected because incremental construction is a reasonable thing for a library
to offer and the hole is in the *checking*, not the capability; removing a
useful constructor to avoid checking it is the kind of trade that looks clean
in the diff and shows up later as somebody reimplementing `Catalog` outside the
crate, where no check reaches at all.

**Move all of `validate_foreign_keys` into `insert`.** Cannot be done, and the
reason is already written in that function's doc comment: a child may be
inserted before its parent, so an unresolved parent is a forward reference, not
an error. Erroring on it would make a self-reference or a cycle unexpressible.
Only the *tenant* check survives the restriction, because it needs both
endpoints visible and can simply skip an edge that is not yet resolvable.

**Check only the table being inserted.** Half the cases. Inserting the child
after the parent catches it; inserting the parent after the child does not,
because the new table is the innocent one. The check runs over every edge in
the catalog for that reason, and the test asserts both orders — a test of one
order would pass for this weaker implementation, which is why there are two.

**Validate at `RecordStore::new` instead.** The point of use, and it would
catch a catalog built any way at all. `RecordStore::new` is a `const fn`
returning `Self`; making it validate means returning `Result` and changing
every construction site in the workspace and in three clients' harnesses. Worth
doing if a second bypass ever appears; not worth it for one.

**Confine the cascade walk at runtime rather than refuse the schema.** The
option the original review weighed and rejected, for the reason quoted above:
it stops the destruction and leaves other tenants' children pointing at a
parent that is gone. Nothing about this second constructor changes that
reasoning.

## Evidence

**The hole, demonstrated before the fix** — `seeded_unvalidated` builds the
catalog with `Catalog::new()` and two `insert`s, seeds one shared org and one
doc in each of two tenants, and has tenant A delete the org:

```
assertion `left == right` failed: tenant A's delete of a shared parent left [];
  left: []
 right: ["b's doc"]
```

The same fixture through `from_tables` cannot be built at all, which is the
contrast that makes it a finding rather than a restatement of finding 1.

**After the fix**, `a_catalog_assembled_by_insert_is_refused_in_either_order`
asserts `CrossTenantForeignKey` naming `docs` and `orgs`, for `Cascade` and
`Restrict`, with the tables inserted parent-first and child-first — four
combinations. It also asserts the catalog is **not** left holding both tables
after a refused insert, because a caller that handles the error would otherwise
go on using a catalog containing the table it was just told was rejected.

Mutation-tested through `scripts/mutate.py`, three mutations, all caught by
that test:

```
ok  insert no longer runs the cross-tenant check
ok  the refused table is left in the catalog
ok  the incremental check skips a resolvable parent instead
```

The third is the one that matters: it is the difference between "the check
runs" and "the check looks at anything".

`cargo test -p slate-schema -p slate-kernel --no-fail-fast` green;
`cargo clippy --workspace --all-targets` clean; `scripts/check.sh` 23/23;
`cargo fmt --all -- --check` clean.

`docs/security-review.md` §1 now says both constructors refuse it, and says
which one did not and for how long. The new test name it cites resolves —
checked by `scripts/check_cited_tests.py`, added in the previous commit for
exactly this.

## What this does not do

**It does not audit the other refusals for the same shape.** This one lived in
`validate_foreign_keys`; the width, type and unknown-parent checks live there
too, and they are still opt-in behind `insert`. They are consistency checks
rather than security ones — a bad width refuses every write rather than
permitting a bad one — so I left them, but I did not enumerate the whole list
and confirm that reasoning holds for each.

**`RecordStore` still accepts any `Catalog`.** A catalog built by some future
third constructor, or by a `Catalog` mutated after validation, reaches the
store unchecked. The fix closes the one bypass that exists today rather than
making the invariant structural.

**No test asserts the daemon's path is the safe one.** I read the five in-repo
`from_tables` call sites and none of them uses `insert`; that is a grep, not a
check, and a sixth call site added tomorrow could use the other constructor
without anything complaining. The refusal now catches it, which is the point —
but the *claim* that the daemon was never exposed rests on reading, not on a
test.

**The performance cost was not measured.** `insert` now walks every edge in the
catalog on every insertion, so building an *n*-table catalog is O(n·e) rather
than O(n+e). Catalogs here have tens of tables and this happens once at
startup, so I judged it irrelevant and did not time it. That is a judgement,
not a measurement.

**It does not re-examine findings 2 through 8 for the same class of bypass.**
Finding 1's fix was guarded in one constructor of two. Whether any other
finding's fix sits behind an opt-in call I have not checked, and the sweep that
found this one was aimed at a documentation caveat rather than at that
question.
