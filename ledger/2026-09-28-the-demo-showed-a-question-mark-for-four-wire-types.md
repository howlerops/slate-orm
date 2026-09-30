# The demo showed `?` for four wire types, and arrays reached nothing a reader could see

- **Date:** 2026-09-28
- **Author:** Claude, continuing the pass over what the caveat tracker still listed
- **Touches:** `examples/explorer/web/src/api.ts`, `examples/explorer/backends/go/seed.go`, `examples/explorer/head.toml`, `site/docs/features.html`
- **Kind:** fix

## What changed

`render` handles `uuid`, `decimal`, `vector` and `array`, and a `never`
parameter makes the **compiler** refuse a build in which any `Tagged` arm is
unhandled. `posts` is seeded with three rows across two array columns of
different element types, is in the `reader` grant, and is no longer in
`NOT_IN_THE_UI` — which is now empty and kept. The features page points at it.

## Why

The residual of `ledger/2026-09-20-an-array-on-the-wire-and-in-three-clients.md`
was the demo half: `posts` existed so the generated array decoders had
something to decode, nothing seeded it, and the UI had no list cell.

Closing it found something larger that nobody had recorded. `render` ended in
`return "?"`, and the adapters can send eleven tags while it handled seven.
Two of the missing four are on `books`, a table the demo has always shown: its
`embedding` is a `vector` and its `price` is a `decimal`. **The demo has been
rendering `?` in two columns of its most-browsed table**, and the test that
would have caught it is called *"every tagged type renders"* while listing
seven of eleven.

That is the shape worth naming: a fallback returning a plausible string is
indistinguishable from working. Nothing errored, nothing logged, and the
assertion that claimed completeness was the thing making it invisible.

## Alternatives rejected

**Add the four arms and leave the `?` fallback.** Fixes today and preserves
exactly the mechanism that hid it — the next wire tag renders as `?` and the
test named "every tagged type" still lists the ones somebody remembered.
Rejected for the reason this repository rejects conventions over structure.

**A runtime registry of tag → renderer, checked against the catalog.** Would
catch a missing tag at startup rather than at compile time, and costs a second
list of the wire types that has to stay in step with `Tagged`. The union is
already that list; making the compiler read it is free.

**Render a decimal at its column's scale.** `1250` at scale 2 is 12.50, which
is what a reader wants. Not taken here: `render` is given a value and no
column, and the scale lives in the catalog beside the column name. Plumbing it
means a second prop on `ValueTable` parallel to `columns`, and the dynamic
query view has no table to look one up in. Left as units, with the header
saying `decimal` beside it, and pinned by a test so a later change has to come
here and say it is doing it.

**Seed `posts` from the Node or Python adapter instead.** The Go adapter is
the one `run.sh` runs once before the others serve, precisely so three
adapters do not race to write the same rows. Putting a second seeder anywhere
else reopens that.

## Evidence

Nine new assertions in `examples/explorer/web/test/api.test.ts`, red before the
change: four tags added to the "every tagged type" table, an array test
covering elements-by-kind, the empty list and nesting, and a decimal test
recording the units decision. 16 tests pass.

**Four mutations** —
`ledger/mutations/20260928T185213-examples-explorer-web-src-api-ts.json`:

| mutation | outcome |
|---|---|
| an array renders its elements without recursing | caught by 2 tests |
| a vector renders as the bare list | caught by 1 |
| a decimal falls through to the unhandled arm | **nothing ran** |
| a uuid falls through to the unhandled arm | **nothing ran** |

The last two are the finding, not a gap. Deleting an arm makes the value
reaching `unhandled` no longer `never`, so `tsc` refuses and no test runs —
`mutate.py` reports `NOTHING RAN`, which is its second documented lie and is
here the truth. Run by hand to see which:

```
src/api.ts(301,20): error TS2345: Argument of type '{ decimal: string; }'
  is not assignable to parameter of type 'never'.
```

The error names the unhandled arm. That is a stronger defence than a failing
test and is invisible to a mutation runner that reads test output, which is
worth knowing before the next session reads those two rows as survivors.

`gofmt -l` clean, `GOTOOLCHAIN=local go vet ./...` clean,
`python3 site/check/docs.py` and `scripts/check_site_claims.py` green,
`sh scripts/check.sh` 71 of 71.

## What this does not do

**A decimal still renders as stored units.** `price` shows `1250`, not
`12.50`. The header says `decimal`, so it is not passed off as an integer, but
a reader who does not know the scale cannot get the amount from the cell. The
fix is the column's scale at the cell, which is a change to `ValueTable`'s
contract and has no answer for the dynamic query view.

**Nothing ran the demo end to end here.** The seeded rows, the grant and the
list cell are checked by the unit suite, `gofmt`, `go vet` and the schema
guards; `./run.sh --e2e` needs a built server and a browser, which this
container does not have. CI runs it. If the seed is wrong in a way the type
system permits — three rows with the wrong ids, say — nothing above would say
so.

**The workbench still has no array.** The features page now points at the
demo, but the page's own invitation elsewhere is "type it into the workbench",
and the workbench's fixture is taxi data with no array column. Two places to
try things, and arrays are in one of them.

**`NOT_IN_THE_UI` is now empty**, which makes it a mechanism with nothing in
it. Kept deliberately — it is what forces the next table added to the catalog
to be shown or explained — but an empty roster is one deletion away from being
tidied up by somebody who does not know that.
