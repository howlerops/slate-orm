# The site

Published at **<https://howlerops.github.io/slate-orm/>**, from `main`, by
[`.github/workflows/pages.yml`](../.github/workflows/pages.yml) on every push.

Two pages and a stylesheet. `index.html` is the **workbench** — an application
that runs slate's kernel in the browser. `docs.html` is everything written
down: what the project is, the quickstart, and the concepts the clients assume.
No build step for the pages themselves, but the workbench needs the wasm built
first:

```sh
sh site/build-wasm.sh
python3 -m http.server --directory site 8000
```

## Why the home page is an application

It used to be a landing page with a query panel two thirds of the way down.
Nobody found the panel — the first report about it was somebody asking whether
it had deployed at all — and a panel of dropdowns can only ask the questions
its author thought of.

So the workbench is the home page: a schema tree, an editor, results, and the
plan beside them. The claim this project makes that is most worth checking is
"the planner picks an access path, and the choice is not the obvious one". You
cannot check that by reading; you check it by writing a query and looking at
what it did. The prose moved to `docs.html`, one click away in the header.

The cost is real and is not hidden: every visitor now downloads roughly 690 KB
of gzipped WebAssembly on arrival, where before it was fetched only for readers
who scrolled to the panel. That is the price of the page being the thing rather
than describing it. `docs.html` loads none of it.

## Why there is no build step for the pages

Two pages. A static-site generator would add a toolchain that has to be
installed, upgraded and eventually migrated, in exchange for templating two
files that share one `<header>`. The trade flips as soon as there are ten
pages or the content wants to live in Markdown.

## The data is real

100,000 New York yellow-taxi trips from January 2024, sampled from the TLC's
published month, joined to their own 265-zone lookup table. The same corpus
ClickHouse and DuckDB benchmark on. Fares, tips, trip distances and the 4.7% of
rows with **no passenger count** are as published — which is why
`count(*)` and `count(passengers)` are different numbers here, a distinction
the generated books fixture could not draw.

```sh
python3 site/data/make-trips.py <yellow_tripdata_2024-01.parquet>
```

`site/data/trips.bin.gz` (1.26 MB) **is committed**, which is a deliberate
exception to the rule that build outputs stay out of git. The wasm is rebuilt
every deploy because it must not drift from the kernel; this file cannot drift
from anything, and rebuilding it in CI would mean a 50 MB download, a pyarrow
dependency and a sampling step that has to be deterministic to the byte.

**Why 100,000 and not the month.** The whole month is 2,964,619 trips and loads
into this same store in 17.8 s — that is measured, in
`docs/performance.md` §7b. The limit is the tab, not the record layer: the
in-memory store costs about 1.3 KB a row, so the month wants ~3.8 GB, and a
`wasm32` tab has 4 GB of address space in theory and around 2 GB in practice.
100,000 rows is ~130 MB and first paint — wasm fetched and compiled, 1.26 MB of
data fetched, decoded, seeded, analysed, first query answered — measured
**1.4 s** in headless Chromium.

The books and authors fixture is still there, as the small-table contrast: at
4,824 rows the planner makes different choices than at 100,000, and having both
on one page shows a cost model responding to size rather than to a rule.

## The workbench runs the real kernel

Not a mock and not a reimplementation: `slate-kernel` and `slate-schema` are
compiled to WebAssembly (`crates/slate-wasm`), and the plan shown beside the
rows is the same `Explanation` the head node returns for `EXPLAIN`.

```sh
sh site/build-wasm.sh            # builds slate_wasm{.js,_bg.wasm} into site/
python3 site/check/workbench.py  # drives it in a real browser
```

The two built files are **not committed** and are in `.gitignore`. A checked-in
binary drifts from the kernel it claims to be and nothing notices; CI builds it
before checking the site, and the Pages deploy builds it before publishing, so
what ships is always the current kernel. `build-wasm.sh` enforces a gzipped
size budget, because the cost of this lands on a reader's connection.

### The SQL is a front end, and says so

slate has no SQL. The kernel takes a `Query`; the clients and the wire take a
structured spec. The editor accepts a small `SELECT`/`INSERT`/`UPDATE`/`DELETE`
subset (`crates/slate-wasm/src/sql.rs`) and **parses it into that spec** — the
same `QuerySpec` the dropdowns used to build and the same one an SDK sends.
The **Spec** tab shows what your statement compiled to, so the translation is
visible rather than claimed.

That is the whole design of it. A parser producing a `Query` directly would be
a second route into the executor, and the two would drift. Going through the
spec means SQL adds no execution path at all, which is what makes the round-
trip property in `crates/slate-wasm/tests/sql.rs` worth having: generate a
spec, render it as SQL, parse it back, require the same spec.

Anything outside the grammar is refused with the offset that caused it, and
the editor puts the caret there. `OR` is rejected rather than quietly ANDed:
the spec has no disjunction to lower it onto, and a wrong answer is worse than
a refusal.

### What the panel is for

The most useful thing it shows is counter-intuitive: filtering on the indexed
`author_id` still plans as a *table scan*. On object storage a point read costs
about as much as scanning twenty-four thousand rows, so an index that still has
to fetch rows loses. Narrow the projection to the indexed column and the plan
becomes an index-only scan at two-thirds the cost. Two statements, one
observation, on the reader's own query.

A row from an index-only scan comes back with its unread columns as `null` —
late materialization, not missing data — so the grid renders those cells as a
muted `·` rather than the word "null". Printing "null" there would be a claim
about the data that is false.

Writes work, and are the other half of a record layer: insert a book and the
index answers for it in the same breath, which the browser check asserts by
requiring the plan to still be index-only afterwards. Writes live in the tab
only; **Reset data** puts the fixture back.

## The quickstarts are checked by running them

```sh
cargo build -p slate-serverd --bin slate-serverd
python3 site/check/quickstarts.py
```

That extracts the four `<pre><code>` panels out of `docs.html`, validates the
TOML panel with `slate-serverd --check`, starts a node from it, and runs the
Python, Go and TypeScript snippets against that node. Each must insert a row
and read it back — asserting on the row rather than on an exit status, because
a snippet whose query silently returned nothing would still exit 0.

Two concessions, both asserted rather than assumed:

* The TOML names an S3 bucket, so what *runs* is a copy with the `[storage]`
  stanza swapped for `memory`. The swap is done by splitting the file, so
  everything outside that stanza is the page's text by construction.
* Every snippet names `127.0.0.1:7421` and the check rewrites it to a free
  port. The substitution must fire exactly once per snippet, so a snippet that
  stops connecting to the head node fails instead of quietly passing.

The first run found the TOML panel invalid: `bucket` sat directly under
`[storage]`, where the field is `[storage.s3] bucket`. It had been on the
page since the page was written, and the page said the snippets had been
executed. They had; the TOML had only been read.

CI runs it on every push (`.github/workflows/ci.yml`, the `quickstarts` job),
which is where it found the two failures above and two more besides: the
snippets sharing one node, and the install lines naming packages that are not
published. Locally it is one command.

The page is published from `main` by `.github/workflows/pages.yml`. That
workflow deliberately does not re-run this check — it publishes `site/`
verbatim — but the two are worth reading together, because a quickstart whose
first code block does not work is exactly what the checker is for.

## Keeping it honest

`docs.html` makes claims about what is built. Every one of them is supposed to
be true on `main`, and its "What it is not" section is supposed to match the
README's not-built list. **If you change what the project does, change this
too** — a front page that oversells is the most-read stale documentation a
project has.

The workbench has a second, sharper version of the same duty: it does not
describe behaviour, it exhibits it. If the kernel changes, the page changes
with it on the next deploy, and `site/check/workbench.py` fails in CI if the
change broke the browser. That is the argument for it being the home page.

Deliberately absent, because there is nothing to put in them: a logo wall, an
adoption count, a comparison table, and any claim about production readiness
beyond the one saying there is none.
