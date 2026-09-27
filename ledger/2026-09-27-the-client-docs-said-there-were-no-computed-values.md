# Five features were reachable from all three clients and documented in none of them, and two "what is not here" bullets had stopped being true

- **Date:** 2026-09-27
- **Author:** Claude Code, closing an open caveat from
  `ledger/2026-09-23-five-built-features-the-docs-never-mentioned.md`
- **Touches:** `site/docs/clients.html`, `clients/python/README.md`,
  `clients/go/README.md`, `clients/typescript/README.md`
- **Kind:** documenting what already shipped, and withdrawing two claims

## What changed

`site/docs/clients.html` gained a table of the five features that shipped after
the clients did — windows, array columns, full-text, views, soft delete —
against the spelling each of Python, Go and TypeScript gives them. Each SDK
README gained the same five in its own idiom, as running code rather than a
table, because that is the register those files are written in.

And two bullets under **What is not here** were withdrawn, in the Go and
TypeScript READMEs:

> No vector similarity search surface, and no computed values in a `Query` or a
> join input.

Both halves are false. `Query.Compute`, `JoinInput.Compute` and
`JoinQuery.Compute` are all `[]Scalar` in Go; `compute?: Scalar[]` appears in
all three of the equivalent TypeScript types. `slate.Distance(a, b, metric)`
and `distance(a, b, metric)` both exist, with a `Metric` enum. What is
genuinely absent is an index-backed nearest-neighbour *operator* — a distance
is a scalar, so a similarity search is computed per row and sorted, which is a
full scan — and that is what those bullets now say.

## Why

The caveat asked for one place a client author can look. The answer was spread
across five ledger entries, and the entries are organised by the change that
made each feature work rather than by the language somebody is writing in.

The two withdrawn bullets are the reason this was worth doing rather than
merely tidy. `CLAUDE.md` calls stale documentation worse than none because it
is read as current, and a "what is not here" list is the most load-bearing
paragraph in a client README: it is what somebody reads to decide whether to
use this at all. Both bullets told a Go or TypeScript author that a feature
they have was missing. Neither was found by a guard; both were found by reading
the file against the code in order to write the section above it.

## Alternatives rejected

**One table on the site and a link from each README.** Half the work and it
keeps one copy. Rejected because the READMEs are what npm, pkg.go.dev and PyPI
render, and a reader there has not got the site open. The three copies are
short and each is in its own language's idiom, which a shared table cannot be:
`Query{IncludeDeleted: true}` and `query.include_deleted()` are the same
feature and not the same sentence.

**A page per feature per language.** Fifteen sections. Rejected for the reason
`2026-09-23`'s entry already gave for the site: the structure is one page per
area, and five features are a paragraph each, not a chapter each.

**Say "see the ledger" and link the five entries.** What the caveat complained
about, restated as a feature.

**Leave the two stale bullets and note them in the caveat.** Cheaper and
dishonest: they are load-bearing in a way a caveat in a ledger entry nobody
reads is not.

## Evidence

Every spelling in the three READMEs and the site table was read out of the
client source or a test in the same suite, not recalled:

| | Python | Go | TypeScript |
|---|---|---|---|
| window | `Query.window`, `Window.row_number().over(…)`, `Query.windowed(i)` | `Window []Window`, `RowNumberOver().Over(p, o)`, `RowStream.Windowed()` | `over`, `rowNumber`, `windowed` |
| array | `ValueType.ARRAY` + `element=`, `Array([…])` | `TypeArray` + `Element:`, `slate.Array{…}` | `{type:"array", element}`, `array([…])` |
| full text | `Expr.contains` | `slate.Contains(ord, text)` | `contains(ord, text)` |
| view | `Table.as_view` | `TableDef.AsView` | `asView(table, name)` |
| soft delete | `Query.include_deleted`, `purge_deleted` | `Query.IncludeDeleted`, `Session.PurgeDeleted` | `includeDeleted`, `purgeDeleted` |

Four first drafts were wrong and each was corrected against real usage rather
than guessed again: Python values are plain `str` and `u64(…)` rather than an
`str_()` wrapper (`clients/python/tests/test_array.py:41`); Go's string
constructor is `slate.String` and not `slate.Str`
(`clients/go/slate/array_test.go:143`); TypeScript's `over` takes an options
object rather than two positional lists (`clients/typescript/src/join.ts:487`);
and the Python window example needed `Query(docs)` bound to a name, because
`q.windowed(0)` is a method on the query.

- `python3 site/check/docs.py` — the site holds together, every element closes
  and every relative link resolves.
- `python3 scripts/check_cited_docs.py` — 640 citations, all openable.
- `sh scripts/check.sh` — 71 of 71.
- The restore claim was checked in the kernel rather than assumed:
  `crates/slate-kernel/src/record.rs:1146` says an update naming a retired row
  restores it by sending null in the soft-delete column, and `check_row`
  refuses any other value. Grepping the three clients and the proto for
  `restore` finds nothing, which is consistent: there is no such call because
  none is needed.

## What this does not do

**None of the twenty-odd lines of example code is executed.**
`site/check/quickstarts.py` runs the code on the site's quickstart page and
nothing runs a README's fences. Four of these were wrong when written, which
is a rate that should be read as a warning rather than as reassurance that
careful reading is enough: the fifth mistake is the one nobody caught. Making
them runnable means a harness per language that builds a server and imports
the package, which is what each client's own test suite already is — the
honest cheap version is to move each fence into that suite and quote it, and
that is not done here.

**It documents the five, not the surface.** The same enumeration problem the
2026-09-23 entry recorded is untouched: nothing compares what the clients can
reach against what the wire offers, so a sixth feature added to the proto and
three clients will be absent from these four files in exactly the way these
five were. Finding these took a grep per feature name.

**The two withdrawn bullets were found by hand, and nothing looks for the
third.** A "what is not here" line becoming false is a documentation defect
with no guard anywhere in this repository, and the Python README's own **Not
covered** section was read in the same pass and left alone because its claims
still hold — which is a sample of three, checked once, by the person who had
just been reading the same code.
