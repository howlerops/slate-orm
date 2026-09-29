# Rule 8 stopped at a crate boundary, and the thing it could not see was the hole rule 10 kept recording

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `scripts/check_handlers.py`, `scripts/test_check_handlers.py`
- **Kind:** security

## What changed

Rule 8 — every `.find(|x| x.name() == ..)` is rostered — now reads **every
workspace member's `src/`**, not only the two directories in `SOURCES`. The
crates outside `SOURCES` use a second roster, `FINDS_BY_NAME_OUTSIDE`, keyed
on path *and* function rather than on a bare name, and it is checked in both
directions. Each half of rule 8 has its own never-fires branch, counted
separately.

Three lookups are rostered there: `Catalog::table_by_name` itself, and the
two duplicate-name refusals in `TableBuilder::build`.

## Why

Three entries in a row today recorded the same limit, each one honestly and
none of them closing it. The first:

> **`tables()` is the hole, and rostering it does not close it.** Any crate
> that is neither `slate-server` nor `slate-serverd` can call
> `catalog.tables()` and walk the slice comparing `name()`, and nothing
> anywhere fails. Rule 8 catches that line in two crates; rule 10 catches a
> method on `Catalog`; the gap between them is every other crate in the
> workspace.

That is a complete description of a hole and a complete description of the
fix, and the fix is rule 8 reading more directories. Rule 10 could never close
it: `Catalog::tables()` has to stay — the migration runner, the seeder and
`--print-schema` all walk it — so rule 10's job ends at rostering it, and the
line that turns the slice into a name lookup is a line in some other file.
Rule 8 reads exactly that line and was pointed at two crates out of thirteen.

The reason the boundary held so long is written in `FINDS_BY_NAME`'s own
comment, which said `Catalog::table_by_name` is "deliberately absent: it lives
in `slate-schema`, outside the directories this reads". A roster whose subject
is excluded for living in the wrong directory is describing a boundary, not
making a decision, and the sentence was doing the work of both.

## Alternatives rejected

**Add `crates/slate-schema/src` to `SOURCES`.** The one-line version, and it
was already rejected once today for rule 10: rule 1 fires on every bare
`self.table(` and `Catalog::table` *is* the bare resolver. Nothing has changed
about that. Rule 8 needs a wider tree and rules 1-7 need the narrow one, which
is what a second walk buys.

**One roster, keyed on the bare function name, for both halves.** Simpler, and
wrong in a way that is measurable: `build` is the enclosing function of *both*
matches in `slate-schema`, and an entry spelled `build` would exempt every
`build` in thirteen crates — including ones written later, which is the whole
population a roster is for. Mutation-tested: keying the widened half on the
bare name is caught.

**Sum the two halves into one count.** It reads better in the summary line and
it destroys a never-fires. A live lookup in `SOURCES` would hold the total
above zero while the widened half had stopped reading anything, which is
precisely the failure the never-fires branches exist to catch, reintroduced by
arithmetic. They are counted and reported separately.

**Scan test files too, not just `src/`.** Every `tests/` directory in the
workspace, plus benches and examples. It would have found nothing today — the
six matches are all in `src/` — and it would roster a large number of
fixtures that build catalogs by hand. `unscanned` makes the same choice for
the same reason, so the two halves of this file agree about what a crate is.

**Leave it and keep the caveat, a third time.** Each of the three entries that
recorded it was right that the entry in front of it had not fixed it. A limit
recorded three times in one day is not a limit; it is a task nobody has
started.

## Evidence

Measured before writing anything, over every `.rs` under `crates/`:

| where | matches |
| --- | --- |
| `slate-schema/src/catalog.rs::table_by_name` | 1 |
| `slate-schema/src/table.rs::build` | 2 |
| `slate-server/src/service.rs::resolve_relation` | 1 |
| `slate-serverd/src/views.rs::views`, `::lowered` | 2 |
| **total** | **6** |

Three were already rostered, and the other three are the new roster. **No
match anywhere walks `Catalog::tables()` comparing names**, which is what
makes this a guard that starts green rather than a cleanup: a separate sweep
for `.tables()` within 200 characters of a `find`/`any`/`position` closure
returned one hit, in `migrate.rs`, and it compares `step` against
`table.id()` inside a `for table in catalog.tables()` loop — not a name
lookup.

`python3 scripts/check_handlers.py`: `3 lookups by name here and 3 in the
other crates, all rostered`.

`python3 scripts/test_check_handlers.py`: **62 passed, 0 failed**, up from 58.

Four mutations, record
`ledger/mutations/20260929T225745-scripts-check-handlers-py.json`:

| mutation | caught by |
| --- | --- |
| the widened half is keyed on the bare function name | most of the file's cases |
| the widened half skips the crates it should read (test inverted) | most of the file's cases |
| the widened half's never-fires branch is unreachable | `no name lookup outside SOURCES at all fails, not passes` |
| the widened roster is never checked for stale keys | `a roster key whose lookup is gone is reported` |

The first two over-catch because the default fixture now carries the widened
half's subject in every case; the last two are one case each. Reported as
observed rather than as four clean single-case catches.

## What this does not do

**The pattern is still a spelling.** `BY_NAME` matches
`.find(|x| x.name() == ..)` and `.any(..)` and `.position(..)`. A lookup
written as a `HashMap<&str, &TableDef>` built once and indexed, a `match` on
`name()`, a comparison with the operands the other way round, or a closure
whose parameter is destructured all pass unseen. That is the residual of the
`tables()` hole rather than its closure, and it is narrower than what was
there this morning rather than gone.

**`Catalog::insert`'s own lookup is invisible to it**, and to the rule before
this change too: it is spelled
`.find(|t| t.id() == table.id() || t.name() == table.name())`, and the
pattern wants `name()` immediately after the closure's parameter. It is not a
resolver — it is the duplicate refusal — so nothing is unguarded, but the
roster does not carry it and a reader counting matches will come up one short
of what a human sweep finds.

**Tests, benches and examples are not read.** A helper in a `tests/` directory
that resolves a table by name is outside both halves. Nothing reaches a tenant
from there, which is the argument, and it is an argument rather than a check.

**It says nothing about what the rostered lookups do.** `build`'s entry claims
both its comparisons are inside the type being built and touch no catalog;
that was read once and a rewrite could falsify it while the key stayed. Every
roster in this file has that weakness, and it is why each entry carries prose
rather than a bare name.
