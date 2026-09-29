# The fifth table list is derived now, and deepEqual compares more than I said

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `examples/explorer/web/test/api.test.ts`
- **Kind:** test

## What changed

One case: the UI's tab order is the catalog's declaration order. And a
correction to the caveat it closes, which was wrong about half of what it said.

## Why

> The constant is still hand-maintained, and the next table to be added will
> break it again the same way. …
>
> Nothing checks that the *order* of tables matches, only the set and each
> table's column order — though `deepEqual` over an object compares neither, so
> the name of the test slightly overstates what it holds.
> — `ledger/2026-09-19-the-fifth-table-list.md`

**The first half is no longer true.** `TABLES` is
`shown(CATALOG_TABLES)` — derived from `src/catalog.ts`, which
`scripts/codegen.py` writes from `slate-serverd --print-schema` and CI
re-checks with `--check`. The next table added to the catalog appears in the UI
without anybody editing a list. That happened after the entry was written and
nobody connected it back; it is the third such find this session.

**The second half is right about tables and wrong about columns**, and the way
to know is to run it rather than reason about it:

```
throws   : column order differs   {a:[1,2]} vs {a:[2,1]}
no throw : table key order differs {a:1,b:2} vs {b:2,a:1}
throws   : a table missing        {a:1,b:2} vs {a:1}
```

`assert.deepEqual` compares arrays in order, so each table's column order *is*
pinned by the existing case. Object keys it compares as a set, so the order of
the tables is not. "compares neither" is half a sentence too strong.

That remaining half is what the new case is for. It holds today by
construction — `Object.fromEntries(Object.entries(…).filter(…))` preserves
insertion order — and "by construction" is exactly the shape this session has
spent the day closing. The tab order is what a visitor sees.

## Alternatives rejected

**Close it on the derivation alone.** The first half deserves that and the
second does not: the order of the tables genuinely was unchecked, and a
refactor that sorted the keys or rebuilt the object some other way would change
the demo's appearance with nothing to object. One case is cheaper than the
argument for not writing it.

**Assert a literal tab order.** `["books", "authors", …]` in the test. It would
catch the same mutation and it is a sixth copy of the table list — which is
what the entry being closed is *about*. Deriving the expectation from
`CATALOG_TABLES` keeps the count at five.

**Widen the existing `deepEqual` case instead of adding one.** `deepEqual` on
objects cannot be made order-sensitive without comparing `Object.keys` anyway,
so it would be the same assertion inside a case whose name is about membership.
Two properties, two cases, two failure messages.

## Evidence

- `examples/explorer/web`: `npm test` — **17 pass, 0 fail** (16 before);
  `npm run typecheck` clean.
- The three `deepEqual` probes above, run rather than reasoned about. They are
  what withdraws "compares neither".
- **Two mutations, both caught**
  (`ledger/mutations/20260929T053326-examples-explorer-web-src-api-ts.json`,
  outcome `clean`). Sorting the tabs breaks only the new case. Reversing every
  table's columns breaks only the old one — which is the demonstration that the
  column-order half was already defended and the new case is not a duplicate.

## What this does not do

**It does not reduce the count.** Five places still name the demo's tables;
four are derived and this one is derived now too, so what changed since the
2026-09-19 entry is that *none* is hand-maintained, not that there are fewer.
The entry's own argument — that the count is the problem — is untouched.

**Nothing checks the order a visitor actually sees.** This pins `TABLES`, and
the tab strip is rendered from it by a component this test does not run. The
browser e2e opens tabs by name, not by position. A renderer that sorted on the
way out would pass everything here.

**The `--check` that keeps `catalog.ts` honest is CI's, not this suite's.** The
derivation is only as good as the generated file being current, and that is
checked in a different job. Running `npm test` alone against a stale
`catalog.ts` proves the UI agrees with a stale catalog.
