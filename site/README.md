# The site

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
