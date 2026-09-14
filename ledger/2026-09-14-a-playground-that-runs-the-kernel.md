# A query playground that runs the database, not a picture of one

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `site/{index.html,playground.js,style.css,build-wasm.sh,README.md}`,
  `site/check/playground.py` (new), `.github/workflows/{ci,pages}.yml`,
  `.gitignore`
- **Kind:** feature

## What changed

A panel on the landing page that answers queries against the record layer
compiled to WebAssembly. Pick a table, a filter, a sort, a limit, and which
columns to read; get the rows and the real `EXPLAIN` plan beside them.

The wasm is built by `site/build-wasm.sh`, checked in a browser by
`site/check/playground.py`, run by CI on every push, and rebuilt by the Pages
deploy before publishing. It is not committed.

## Why

The site could describe the planner and not demonstrate it. "An index-only scan
does not read rows" is the kind of claim a reader has to take on trust, clone
the repository to check, or quietly disbelieve.

More specifically, this project's most surprising property is one nobody
believes from prose: **a secondary index usually loses**. `SCAN_ROW_COST` is
0.000125 and `POINT_READ_COST` is 3.0, so one point read costs as much as
scanning twenty-four thousand rows. An index that still fetches rows is worse
than a table scan at any size a browser tab holds. An index that avoids them —
a covering scan — wins.

The panel makes that a thing a reader does rather than a paragraph they skim:
filter on `author_id`, see `Table Scan · cost 1.603`; untick the other columns,
see `Index Only Scan using by_author · cost 1.001`.

## Alternatives rejected

**Showing only the projected columns in the results table.** The obvious
design, and wrong: a projection changes what the plan *decodes*, not the shape
of the answer, and hiding columns would suggest the projection filters output.
The table shows every column and the `decodes [1]` badge is where the narrowing
appears — which is also what the plan itself reports.

**Committing the built wasm.** Removes a Rust toolchain from the deploy path,
and reintroduces exactly the failure the Python client's protobuf stubs are
guarded against: a generated artifact that is correct until somebody forgets to
regenerate it. Built twice instead — in CI and on deploy — so the published
binary is always the current kernel. The cost is real: a Pages deploy now
compiles Rust.

**Leaving the panel visible while the module loads.** It starts `hidden` and is
revealed only after `init()` resolves, so a reader with wasm disabled, or one
arriving before a deploy carried the binary, sees prose rather than controls
that do nothing. The visibility doubles as the browser check's "did it load"
assertion.

**A free-form query language.** A text box and a parser would be a second
surface to keep honest, and the panel's job is to show the *planner*, not to
relitigate SQL. Dropdowns cannot express a query the kernel will refuse to
parse, which keeps every error a real one.

**Checking the panel with unit tests over the binding.** Those exist, in
`crates/slate-wasm`, and they cannot catch a control wired to nothing or a
renamed JSON field. The browser check drives the actual page, which is the same
reason the quickstart checker executes the page's snippets instead of reading
them.

## Evidence

`site/check/playground.py`, in headless Chromium: the module loads and the
panel answers; a filter on the indexed column plans as a table scan; narrowing
to that column alone makes it index-only; the covering plan is badged as the
good one; `decodes` narrows with the projection; a bad literal is refused in
the panel rather than swallowed; no page or console errors.

Three mutations, each killed: dropping `decodes` from the badges, swallowing
the error instead of showing it, and never sending the projection so
index-only never happens.

Size: 1,715 KiB of wasm, **596 KiB gzipped**. `build-wasm.sh` fails over 900
KiB gzipped, so a dependency that doubles the bundle breaks the build rather
than a reader's connection.

`site/check/quickstarts.py` still passes, so the page's existing code blocks
were not disturbed.

## What this does not do

Filters, sort, projection, limit, offset. No joins, aggregates or grouped
reads, though the kernel underneath does all of them — the binding's surface is
narrower than the kernel's on purpose, and widening it is mostly UI.

**No writes.** The store is seeded and then read, so a reader cannot insert a
row and watch an index be maintained — which is half of what makes this a
record layer rather than a query engine, and the more interesting half.

One filter, not a conjunction. `WHERE a = 1 AND b > 2` is expressible in the
kernel and not in this panel.

Nothing lazy-loads the bundle: 596 KiB is paid by every visitor to the landing
page, including one who never touches the panel. Deferring it until the panel
scrolls into view is a real improvement and is not done.

The size budget is a number in a shell script, not a measurement of what a
reader can tolerate. 900 KiB was chosen as "meaningfully above today's 596 with
room for a dependency", which is a judgement rather than a finding.
