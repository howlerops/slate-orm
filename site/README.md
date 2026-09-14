# The site

Published at **<https://howlerops.github.io/slate-orm/>**, from `main`, by
[`.github/workflows/pages.yml`](../.github/workflows/pages.yml) on every push.

Two static pages and a stylesheet. No build step, no dependencies, no
generator — open `index.html` in a browser, or:

```sh
python3 -m http.server --directory site 8000
```

## Why there is no build step

The site's job is to let someone find the quickstart and start using an SDK.
That is two pages. A static-site generator would add a toolchain that has to be
installed, upgraded and eventually migrated, in exchange for templating two
files that share one `<header>`.

The trade flips as soon as there are ten pages or the content wants to live in
Markdown. Until then this is the smaller thing.

## The playground runs the real kernel

`index.html` has a panel that answers queries. It is not a mock: `slate-kernel`
and `slate-schema` are compiled to WebAssembly (`crates/slate-wasm`) and the
plan shown beside the rows is the same `Explanation` the head node returns for
`EXPLAIN`.

```sh
sh site/build-wasm.sh            # builds slate_wasm{.js,_bg.wasm} into site/
python3 site/check/playground.py  # drives it in a real browser
```

The two built files are **not committed** and are in `.gitignore`. A checked-in
binary drifts from the kernel it claims to be and nothing notices; CI builds it
before checking the site, and the Pages deploy builds it before publishing, so
what ships is always the current kernel. `build-wasm.sh` also enforces a
gzipped size budget, because the cost of this lands on a reader's connection.

It reads, writes and joins. Insert a book and the index answers for it in the
same breath — an index-only scan goes from four rows to five without ever
reading a row, which is what "the index is maintained inside the write" means
when you can watch it. Two conditions can be ANDed, and a collapsed section
joins `authors` to `books` and groups the result.

The bundle is fetched when the panel is scrolled to, or when the Load button is
pressed — never on page load, so a reader who never scrolls here pays nothing.

The most useful thing the panel shows is counter-intuitive: filtering on the
indexed `author_id` still plans as a *table scan*. On object storage a point
read costs about as much as scanning twenty-four thousand rows, so an index
that still has to fetch rows loses. Narrow the columns to the indexed one and
the plan becomes an index-only scan at two-thirds the cost. That is the whole
argument for covering indexes, on the reader's own query.

## The quickstarts are checked by running them

```sh
cargo build -p slate-serverd --bin slate-serverd
python3 site/check/quickstarts.py
```

That extracts the four `<pre><code>` panels out of `index.html`, validates the
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
landing page since the page was written, and the page said the snippets had
been executed. They had; the TOML had only been read.

CI runs it on every push (`.github/workflows/ci.yml`, the `quickstarts` job),
which is where it found the two failures above and two more besides: the
snippets sharing one node, and the install lines naming packages that are not
published. Locally it is one command.

The page is published from `main` by `.github/workflows/pages.yml`. That
workflow deliberately does not re-run this check — it publishes `site/`
verbatim — but the two are worth reading together, because a landing page whose
first code block does not work is exactly what the checker is for.

## Keeping it honest

The landing page makes claims about what is built. Every one of them is
supposed to be true on `main`, and the "What it is not" section is supposed to
match the README's not-built list. **If you change what the project does,
change this too** — a landing page that oversells is the most-read stale
documentation a project has.

Deliberately absent, because there is nothing to put in them: a logo wall, an
adoption count, a comparison table, and any claim about production readiness
beyond the one saying there is none.
