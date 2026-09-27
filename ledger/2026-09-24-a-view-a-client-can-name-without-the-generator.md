# A view's declaration is its base table's, renamed. The generated file knew that; the three client libraries did not, so only generated code could name a view.

- **Date:** 2026-09-24
- **Author:** Claude Code, working #302 (F7c)
- **Touches:** `clients/python/src/slate/schema.py`, `clients/go/slate/schema.go`, `clients/typescript/src/schema.ts`, `clients/typescript/src/index.ts`, and a test in each
- **Kind:** closing a recorded caveat by building the thing it names

## What changed

- Python: `Table.as_view(name) -> Table`
- Go: `func (t TableDef) AsView(name string) TableDef`
- TypeScript: `asView(table, name)`, a free function, exported from the index

Each returns the receiver's columns and primary key under a new name.

## Why

`scripts/codegen.py` has emitted `VIEWS` and `VIEWS_BY_NAME` since #258, and it
builds each one as `Table(name, BASE.columns, BASE.primary_key)` — inline, in
three languages. The construction was known and duplicated, and the libraries
did not have it, so a hand-written declaration could not name a view at all.

The fact being encoded is not the rename. It is that **a view's ordinals are
its base table's**. `docs/views.md` refuses a projection precisely so that
stays true, and a caller who declared a view by hand with its columns in a
different order would not get an error — they would get a row decoded into the
wrong fields. That is the failure this removes the opportunity for.

Closes the caveat "No client library gained a `Table.as_view(name)`" from
`ledger/2026-09-21-a-generated-view-declaration.md`.

## Alternatives rejected

**A distinct `View` type.** Type-safe, and it would let the client refuse a
write through a view locally. Rejected: the server already refuses that by
construction, and a second type means every function taking a `TableDef`
either grows an overload or stops accepting views. `docs/views.md` says a view
is its base table's declaration under another name; a separate type would say
something the design does not.

**A method on the TypeScript interface.** `TableDef` is an interface, and a
generated declaration is an object literal. A method would have to be attached
by every hand-written declaration too. The file already exports `ordinalOf` and
`fingerprint` as free functions over `TableDef`, so `asView` follows.

**Having codegen call it.** Tempting — one construction, used everywhere. Not
done: the generated file must stand alone without importing helpers that could
change under it, which is why it writes literals today. The duplication is
deliberate and now documented at both ends.

## Evidence

Python: 4 tests, including that every ordinal and type survives the rename.
Go: `TestAsViewCopiesWhatItShares`. TypeScript: one case in `schema.test.ts`.

**Go and TypeScript copy the slices; Python does not need to.** Python's
`Table` holds tuples, so sharing is safe. Go's `TableDef` holds `[]ColumnDef`
and TypeScript's holds a mutable array behind a `readonly` field — a view built
by reference would let an append to either declaration be seen by the other,
which is a wrong ordinal and a mis-decoded row. Both tests mutate the view and
assert the base table is untouched; both fail without the copy.

`gofmt` clean, `go build ./...` clean, `tsc --noEmit` clean, Python 4 passed.

## What this does not do

**No mutation run.** The container is at 1.2 GB free and `target/` is 19 GB;
`mutate.py` needs a build per case. The two copy-tests are the ones that would
matter, and each was checked by hand — writing through the view and watching
the base table change before the copy was added — which is weaker than a
recorded run and is why this says so.

**The three clients are not compared against each other.** Each suite tests its
own language. The conformance runner compares answers from a live server, and
`as_view` never reaches the wire: it produces a declaration, and the claim that
declaration makes is checked server-side under the view's name. So there is
nothing for the runner to compare, and nothing asserts that the three
implementations agree beyond three separately-written tests.

**The web app's `VIEWS` is still hand-written.** That is the sibling caveat
from the same entry and it is untouched here.
