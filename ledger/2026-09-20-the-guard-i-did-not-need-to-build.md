# The third instance of the pattern was never an instance

- **Date:** 2026-09-20
- **Author:** Claude, about to build a guard and checking first whether it was needed
- **Touches:** `crates/slate-kernel/tests/security_probe_cascade.rs`, two ledger entries
- **Kind:** docs

## What changed

Two withdrawals and one test. No guard, because the thing it would have
guarded is enforced by the type system.

## Why

Three findings this session had fixes covering one path of several. Two got
checks. The third — `Catalog::from_tables` and `Catalog::insert` — was left
with a caveat in two entries saying a *third* constructor would be the same
story, and I sat down to write the roster for it.

Reading the type first took ten seconds and answered the question:

```rust
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    tables: Vec<TableDef>,   // private
}
```

- **One `impl Catalog` block**, in the defining module — an inherent impl
  cannot be written anywhere else.
- **One `&mut self` method**: `insert`, which is also the only `push`.
- **No `serde`**, so no deserialisation path around the constructor.
- **Nothing returns `&mut`** into the vector; `tables()` gives `&[TableDef]`
  and `table()` gives `Option<&TableDef>`.
- `Default` and `Clone` cannot add a table.

So **every table that enters a `Catalog` passes through `insert`**, and
`insert` runs the cross-tenant refusal. There is no third door. The roster
would have been a text search asserting something `rustc` already proves, and
its entries would have needed maintaining for no gain.

Both caveats are struck through with the correction beside them, which is what
`ledger/README.md` asks for when an entry claims a gap that was never there.
One of them — "or by a `Catalog` mutated after validation" — was wrong twice
over: the only mutation *is* `insert`, so "after validation" describes nothing.

**The uncomfortable part is why I wrote them.** Both sentences were produced by
the pattern rather than by the code: three findings had turned out to cover one
path of several, so I assumed the fourth would too and wrote the assumption
down as a gap. That is the same move as the exemption whose "its callers
authorise before converting" was false, except inverted — there I assumed
safety, here I assumed danger. Reading the type is what distinguishes them, and
it was cheap both times.

## Alternatives rejected

**Build the roster anyway, cheaply.** Forty lines, and it would pass forever
while checking nothing that could fail. `CLAUDE.md`'s own framing applies: a
check that never fires is a check nobody has debugged, and this one could not
fire without someone first defeating the borrow checker. It would also
misinform — a reader seeing `check_catalog_constructors.py` would reasonably
conclude the invariant is conventional.

**Make the guarantee louder in the type** — a `Validated<Catalog>` newtype, or
sealing the struct. Nothing to buy: the guarantee already holds, and a newtype
would change every signature taking a `Catalog` to say something `rustc`
enforces at the one place it matters.

**Withdraw the caveats and write no test.** The withdrawal is the substance,
and the test is nearly free. It pins the two consequences a reader would
otherwise re-derive from the type — a clone is as safe as its original, a
`Default` is empty rather than unchecked — and those are the two shapes the
withdrawn sentences reached for.

**Say nothing and delete the sentences.** The README rejects this explicitly:
deleting loses the more useful half, which is that the reasoning was made and
where it went wrong.

## Evidence

The type, quoted above, and the greps behind it:

```
$ grep -n "self\.tables\.\|&mut self" crates/slate-schema/src/catalog.rs
55:    pub fn insert(&mut self, table: TableDef) -> Result<()> {
81:        self.tables.push(table);
104:        self.tables.pop();
141,147: … .iter().find(…)          # reads
$ grep -rn "impl.*Catalog\b" crates/slate-schema/src/
crates/slate-schema/src/catalog.rs:16:impl Catalog {
$ grep -n "Deserialize\|Serialize" crates/slate-schema/src/catalog.rs
(nothing)
$ grep -n "&mut" crates/slate-schema/src/catalog.rs
55:    pub fn insert(&mut self, table: TableDef) -> Result<()> {
```

`a_catalog_cannot_be_reached_around_by_cloning_or_defaulting` asserts the two
consequences over both referential actions: a cloned catalog refuses the edge,
the original is unaffected by what its clone was refused, and a `Default`
catalog refuses it after taking the parent. `cargo test -p slate-kernel --test
security_probe_cascade`: 11 passed.

**No mutation run for this one, and the reason is the finding.** Breaking the
refusal in `insert` is already covered by
`a_catalog_assembled_by_insert_is_refused_in_either_order`, whose three
mutations were recorded when it was written. There is no separate mechanism
here to break — which is exactly the argument against the guard.

## What this does not do

**It does not make a second mutating method impossible.** If one is ever added
it will sit in the same `impl` block, directly below `insert`'s refusal and its
long comment, and neither this test nor the compiler will object. That is the
residual risk, and it is a much smaller one than "a third constructor
somewhere else" — which is the correction.

**It does not check the other `validate_foreign_keys` rules.** Width, type and
unknown-parent are still opt-in behind an explicit call, as
`2026-09-20-the-other-way-to-build-a-catalog.md` said. Only the cross-tenant
check moved into `insert`, and only that part of the caveat is withdrawn.

**It says nothing about `SecurityCatalog`**, which is a different type with its
own constructors and its own invariants. I looked at `Catalog` because that is
what the caveats named.

**Two entries were edited after the fact.** The README permits it for a claim
that was wrong when written, which this was, and asks for the strikethrough
rather than a delete. I did not sweep the other entries for similar
pattern-derived claims; there may be more, and finding them means re-reading
twenty files rather than following a link.
