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

The three snippets on the landing page and the TOML beside them were extracted
from the page and executed against a head node started from that same TOML.
All three insert a row and read it back.

They are **not** checked automatically. There is no test that re-extracts them,
so they can rot — and example code that does not compile is the most
embarrassing kind of stale documentation. If you change a client's API, run
them:

```sh
# start a node from the TOML tab, then paste each snippet into a file and run it
```

Wiring that into CI is worth doing and is not done.

## Keeping it honest

The landing page makes claims about what is built. Every one of them is
supposed to be true on `main`, and the "What it is not" section is supposed to
match the README's not-built list. **If you change what the project does,
change this too** — a landing page that oversells is the most-read stale
documentation a project has.

Deliberately absent, because there is nothing to put in them: a logo wall, an
adoption count, a comparison table, and any claim about production readiness
beyond the one saying there is none.
