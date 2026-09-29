# Reading the crate instead of the file turns rule 10 from a sample into the whole surface

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `scripts/check_handlers.py`, `scripts/test_check_handlers.py`
- **Kind:** security

## What changed

Rule 10's `CATALOG_SOURCE` is now `crates/slate-schema/src` rather than the
one file `catalog.rs` under it. Every `.rs` in the crate is scanned for
`impl Catalog` blocks at column zero, and their public accessors are pooled
into the one set the roster is checked against. Two test cases, and the
scanning is mutation-tested in both directions.

## Why

The entry that added rule 10 four commits ago wrote this limit down rather
than closing it:

> **It reads one file by path.** A `Catalog` accessor added in another module
> of `slate-schema` […] is invisible. […] a `slate-schema` that splits
> `catalog.rs` in two would leave this rule reading half a surface with no
> complaint. The never-fires branch only fires on *nothing* found.

That was true and it cost one `rglob` to fix, which makes recording it the
wrong call. The last clause is the reason it mattered: a rule whose only
self-check fires on *nothing found* cannot notice that it found half.

The version that matters is what the widening buys, and it is more than
"one more file". **Rust's orphan rule means only `slate-schema` can add an
inherent method to `Catalog`.** So scanning the crate is not a wider sample —
it is the complete set of inherent methods, and the roster now stands over all
of them rather than over the ones that happen to live in the file somebody
named. That is a different kind of claim from the one the first version could
make.

`docs/views.md` §3a's argument rests on "one method turns a name into a
table". Before this it rested on "one method in `catalog.rs` turns a name into
a table", which is not the same sentence.

## Alternatives rejected

**Leave it and keep the caveat.** The first version's own position, and it was
defensible for about an hour. The cost of closing it is a `rglob` and two test
cases; the cost of keeping it is an open row that reads as a hazard somebody
weighed. `ledger/README.md` is explicit that a recorded limit is a claim about
where the work stops, not a place to park work that is cheaper than the note
describing it.

**Scan the whole workspace for `impl Catalog`.** Strictly wider and it buys
nothing, because of the orphan rule: an inherent `impl Catalog` outside
`slate-schema` does not compile. It would also drag in test modules and doc
examples in other crates, each needing an exemption for a thing that cannot
exist.

**Also catch `impl <Trait> for Catalog`.** This is the real remaining path — a
trait implemented for `Catalog` in any crate can offer
`fn by_name(&self) -> Option<&TableDef>` built out of `tables()`. Rejected
because it is the `tables()` hole wearing a different hat, and guarding the
trait form while the slice stays public would report a shape and miss the
substance. Catching it properly means making `tables()` unavailable, which the
previous entry costed out and declined.

**Keep the single-file read and add a never-fires that counts files.** "Fail
if `slate-schema/src` holds more than one `.rs`" would notice the split
without reading the second file. It is a tripwire rather than a check: it
fires on a refactor that moved nothing dangerous and stays silent on the case
where `catalog.rs` itself grows the second lookup, which rule 10 already
catches. A rule that reads what it is about beats a rule that notices it
might have moved.

## Evidence

`python3 scripts/check_handlers.py` on the real tree: unchanged output, 4
public `Catalog` methods handing out a table, all rostered. `slate-schema/src`
holds eight `.rs` files and exactly one carries an `impl Catalog`, so the
widening finds the same set — which is the point of the two new cases rather
than of this run.

`python3 scripts/test_check_handlers.py`: **56 passed, 0 failed**, up from 54.

Two mutations, record
`ledger/mutations/20260929T224325-scripts-check-handlers-py.json`:

| mutation | caught by |
| --- | --- |
| the scan globs `catalog.rs` again instead of `*.rs` | `a lookup in another file of the same crate is reported` |
| any `impl` at column zero counts, not just `impl Catalog` | `another file with no impl Catalog changes nothing` |

A third record, `20260929T224311-scripts-check-handlers-py.json`, is the same
run aborted: the first anchor carried eight spaces of indentation and occurs
zero times, so `mutate.py` refused before running anything and wrote
`"outcome": "interrupted", "cases": []`. It is kept because a refused run is
part of what happened, and because the record is the only thing that
distinguishes it from a run that found nothing.

One case each, which is what a mutation run should look like. The previous
entry's whole-signature mutation was caught by a handful of cases at once
because every fixture shared the subject, and that entry records the
over-catching as a weakness rather than a strength.

## What this does not do

**A trait implemented for `Catalog` is still invisible**, in this crate or any
other, and it is the one remaining way to add a public name-to-table lookup
without rule 10 seeing it. It is a form of the `tables()` hole the previous
entry recorded: the method body has to get its tables from somewhere, and the
only public source is the slice. Nothing new is unguarded by this change; the
sentence "rule 10 covers the inherent surface" is exact, and "the inherent
surface is the whole surface" is not.

**Column zero is still the test for an `impl` block.** An `impl Catalog`
indented inside a `mod` — legal, and how a `#[cfg(test)]` helper would be
written — is skipped. That is deliberate for tests and wrong for a real
submodule that indents its contents, which no file in this crate does and
nothing checks.

**The eight-file count is today's.** Nothing asserts that `slate-schema/src`
still holds a file with `impl Catalog` in it; the never-fires branch fires on
no accessor found anywhere in the crate, which covers the crate being emptied
or moved but reports it as "the accessors are no longer public" among three
guesses. It names a person rather than a cause.
