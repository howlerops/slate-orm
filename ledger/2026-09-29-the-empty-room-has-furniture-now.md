# The guard declined because there was no view to leak, and views shipped a week later

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `scripts/check_handlers.py`, `scripts/test_check_handlers.py`,
  `scripts/check_closed_caveats.py`, `docs/caveat-status.json`
- **Kind:** security

## What changed

`check_handlers.py` grew a tenth rule: every public `Catalog` method whose
**return type** mentions `TableDef` must be listed in `HANDS_OUT_A_TABLE` with
a reason a caller's name cannot reach it. Four are, today —
`table_by_name`, `table`, `tables`, `referencing` — and the roster's shape is
its argument: one of them turns a name into a table.

It reads `crates/slate-schema/src/catalog.rs` by path rather than joining
`SOURCES`, because `slate-schema` holds the primitive rule 1 exempts by name
and walking it would make every `self.table(` in it a rule 1 failure.

Eleven test cases, and the never-fires halves both directions: no catalog
file, no public accessor, and `table_by_name` missing from an otherwise
healthy file each fail rather than pass.

## Why

`ledger/2026-09-21-refusing-a-view-everywhere-else-is-free.md` is the entry
that argued a view kept *out* of the `Catalog` is refused by every read path
in the system without a line being written — because `Catalog::table_by_name`
is the only way a name becomes a `TableDef`. It then said outright that it had
not checked the thing it rested on:

> **It does not check the property it relies on.** Nothing fails if a future
> `Catalog` gains a second lookup that a view could satisfy […] A guard is
> writable — assert `table_by_name` is the only public name-to-`TableDef`
> lookup — and was not written, because today there is no view to leak and
> writing it before the feature would be guarding an empty room.

That was a good reason on 2026-09-21 and it expired. Views shipped across F5b
and F5c: `crates/slate-serverd/src/views.rs`, a `[[views]]` block in the
config, `docs/views.md`, one opted-in read path. The room has furniture in it,
and the one sentence holding the whole design up was the one sentence nothing
tested.

Rule 8 already guards the same property in `slate-server` and `slate-serverd`,
and `FINDS_BY_NAME`'s own comment says where it stops: "`Catalog::table_by_name`
is deliberately absent: it lives in `slate-schema`, outside the directories
this reads, and it is the one this roster exists to keep singular." Nine rules
were pointed at the callers of a primitive nothing was pointed at.

## Alternatives rejected

**Add `crates/slate-schema/src` to `SOURCES`.** One line, and it turns the
other nine rules loose on the crate. Rule 1 fires on every bare `self.table(`
— and `Catalog::table` *is* the bare resolver; that is what it is for. The
`MARKERS` comment already measured this exact collision when `self.table(` was
proposed as a fourth marker and reported `slate-schema/src/catalog.rs` among
three false positives: "a crate holding a primitive is not a crate that
discloses". A separate path with one rule on it keeps that true.

**Key the rule on taking a `&str`.** The obvious version, and it passes the
dangerous shape. `pub fn by_name(&self) -> HashMap<&str, &TableDef>` takes no
name at all and is a name lookup for every caller holding the map. There is a
test case for exactly that, and a mutation that adds such a getter to the real
catalog — both would sail past a parameter-keyed rule.

**Key it on the whole signature rather than the return type.** Simpler to
write and it rosters `insert` and `from_tables`, which take a `TableDef` and
hand back a `Result`. Neither can leak a table to a caller. A roster carrying
entries nobody can act on is a roster people stop reading, which is how
`EXPECTED_REFUSALS`-style lists fail. Mutation-tested: the whole-signature
version is caught.

**Assert it in Rust instead — a compile-time or test-time reflection over
`Catalog`'s surface.** Rust has no stable way to enumerate an inherent impl's
methods. The alternatives are a proc macro over the impl block, which means
`slate-schema` grows a build dependency to hold a repository convention, or a
doc-test that lists the methods by hand, which is the roster again with worse
tooling. Text over the one file is honest about what it is.

**Ban `tables()` outright, so the slice cannot be walked.** That is the real
hole — `catalog.tables().iter().find(|t| t.name() == n)` is a name lookup in
any crate rule 8 does not read — and closing it means removing a method the
migration runner, the fingerprint and `--print-schema` all need. Rostered with
the hole written into its reason instead; see below.

## Evidence

`python3 scripts/check_handlers.py` on the real tree:

```
ok    31 files, … 3 lookups by name all rostered, 4 public `Catalog` methods
      handing out a table all rostered, 13 workspace crates …
```

`python3 scripts/test_check_handlers.py`: **54 passed, 0 failed**, up from 43.
The eleven new cases are rule 10's; the other eleven of the difference is the
catalog fixture every existing case now needs, for the same reason every case
needs a `Cargo.toml` — twelve failed on the first run naming a temporary
directory, the same shape as the nine that failed when rule 9 arrived.

Three mutations of the **real catalog**, judged by the guard through
`mutate_guard.py` — record
`ledger/mutations/20260929T223610-crates-slate-schema-src-catalog-rs.json`:

| mutation | outcome |
| --- | --- |
| a second `pub fn find_named(&self, name: &str) -> Option<&TableDef>` | caught |
| `pub fn by_name(&self) -> BTreeMap<&str, &TableDef>`, taking no name | caught |
| `table_by_name` renamed to `lookup_named` | caught, by the anchor branch |

Six mutations of the **guard**, judged by its fixture tests — record
`ledger/mutations/20260929T223641-scripts-check-handlers-py.json`:

| mutation | caught by |
| --- | --- |
| key on the whole signature, not the return type | four cases, not the one written for it |
| stop the scan at the first closing brace | `a lookup in a second impl Catalog block is reported` |
| the anchor branch settles for any method called `table` | `a renamed table_by_name fails rather than passing` |
| the never-fires branch tests `is None` rather than empty | both never-fires cases |
| the unrostered set difference taken the other way round | four cases |
| a private method counts as public surface | `a private lookup is not the public surface` |

The first of those is worth reporting honestly: the case written for it — "a
method that takes a `TableDef` and returns none is not rostered" — is not the
first name in the failure list, because the fixture catalog carries `insert`
and `from_tables` in *every* case, so the whole-signature version turns all of
them red at once. It is caught, and it is over-caught; a fixture without those
two methods would have made it a one-case mutation and would also have made
the passing case vacuous.

**One defect found by writing a case rather than by reading the code.** The
first version of the scan `break`s at the first `}` at column zero. Rust
allows several inherent `impl` blocks, and a second `impl Catalog` further
down a 271-line file — precisely where somebody adds an accessor — was
unreachable. The case is `a lookup in a second impl Catalog block is
reported`; it failed, the loop now clears state and keeps scanning, and the
mutation above holds it there.

## What this does not do

**`tables()` is the hole, and rostering it does not close it.** Any crate that
is neither `slate-server` nor `slate-serverd` can call `catalog.tables()` and
walk the slice comparing `name()`, and nothing anywhere fails. Rule 8 catches
that line in two crates; rule 10 catches a method on `Catalog`; the gap
between them is every other crate in the workspace. The roster entry for
`tables` says so, which is not the same as fixing it.

~~**It reads one file by path.** A `Catalog` accessor added in another module
of `slate-schema` — an `impl Catalog` in `lib.rs`, a trait implementation, an
extension trait in a third crate — is invisible.~~ **Withdrawn the same day.**
Rule 10 reads the crate's whole `src/`, and Rust's orphan rule makes that
complete for inherent methods: only `slate-schema` may add one to `Catalog`.
See `ledger/2026-09-29-the-orphan-rule-makes-the-tree-the-whole-surface.md`.
What remains is a trait implemented for `Catalog` elsewhere, which is the
`tables()` hole above and not a second thing.

**It is text, not types.** A method whose return type is an alias for
something containing `TableDef` — `type Tables = Vec<TableDef>` used as
`-> Tables` — is not matched. So is one returning `impl Iterator` over an
associated type. Both are catchable by a person reading the diff and by
nothing here.

**Splitting on the first `->` is a guess.** A parameter that is itself a
function type puts an arrow before the real one; a return type that is a boxed
closure puts one after. Neither exists in `catalog.rs` today. The first arrow
errs toward reporting a method that hands out nothing, which costs a roster
entry; the last would err toward missing one, which costs the rule.

**Nothing checks that the roster's *reasons* are true.** `referencing` says it
selects by a foreign key's parent and never by a caller's name; that is a
claim about the body, read once, and a rewrite could falsify it while the name
stayed on the list. Every roster in this file has the same weakness and it is
the reason each entry carries prose a reader can check rather than a bare name.
