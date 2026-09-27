#!/usr/bin/env python3
"""Open every documentation page in a real browser and check it came up.

    python3 site/check/pages.py

`site/check/docs.py` reads the pages as text: every element closes the one it
opened, every relative link resolves, every claim is one the repository can
back. It cannot see anything that happens *after* the file is served, and the
whole navigation does happen then — `nav.js` builds the sidebar, the
right-hand contents and the previous/next pair on every page, from a table in
its own source.

So a JavaScript error in `nav.js` empties the sidebar on all ten pages at once
and every static check stays green. That is the hole
`ledger/2026-09-16-what-the-other-orms-have-that-this-does-not.md` recorded and
this closes:

  > **The browser render was a one-off, not a check.** … nothing in CI renders
  > a documentation page. That is a real hole — `nav.js` builds the navigation
  > for all ten pages and a JavaScript error would empty the sidebar site-wide
  > with every static check still green.

`site/check/workbench.py` does this for the one page that *runs*. This is the
same idea for the nine that only describe, and it is much cheaper: no wasm, no
trip file, no kernel — a page is correct here if it renders and its navigation
is there.

# What is asserted, and what deliberately is not

Only what is about the *browser*: that the module loaded, that it found the
three hosts it renders into, and that what it rendered matches what the page's
own HTML says it should be. Whether the prose is true is `docs.py`'s job and
`check_claims.py`'s; whether a link resolves is `docs.py`'s. Repeating either
here would mean two checks failing on one defect and a reader having to work
out which is the real one.

The counts are all *derived*, never written down. The sidebar must hold one
link per page `nav.js` itself lists; the contents must hold one entry per `h2`
the served HTML contains; the pager must hold two links except at the ends of
the reading order. A hard-coded ten would go stale the first time somebody adds
a page, and going stale is the failure this file exists to catch one level up.
"""

from __future__ import annotations

import http.server
import json
import re
import socket
import subprocess
import sys
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SITE = ROOT / "site"
DOCS = SITE / "docs"
#: The demo's frontend is where Playwright lives, as `workbench.py` explains:
#: running from there keeps a second copy of a heavyweight dependency out of
#: the tree.
RUNNER = ROOT / "examples" / "explorer" / "web"

#: One `["page.html", "Label"]` pair out of `nav.js`'s `SECTIONS`.
#:
#: Read out of the source rather than restated here, because a second list of
#: the pages is a second list to keep true — the mistake this whole file is
#: about. The regex is deliberately narrow: it matches the array-of-two-strings
#: shape and nothing else, so a `SECTIONS` rewritten into another shape makes
#: this find zero pages and fail loudly, rather than silently checking a
#: subset.
PAGE_PAIR = re.compile(r'\[\s*"([a-z0-9-]+\.html)"\s*,\s*"([^"]+)"\s*\]')

DRIVER = """
import { chromium } from "playwright";
const [base, ...pages] = process.argv.slice(2);
const browser = await chromium.launch();
const seen = {};
for (const page of pages) {
  const tab = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const problems = [];
  tab.on("pageerror", (e) => problems.push(`page error: ${e}`));
  tab.on("console", (m) => {
    if (m.type() === "error") problems.push(`console: ${m.text()}`);
  });
  // `load` and not `domcontentloaded`: `nav.js` is a module, so it runs after
  // the document is parsed, and asserting on the sidebar before it has run
  // would fail on every page for the wrong reason.
  await tab.goto(`${base}/docs/${page}`, { waitUntil: "load" });
  seen[page] = {
    problems,
    // `h2.nav-section` is the section heading nav.js emits, so the link count
    // has to exclude it; `a` inside the sidebar is exactly one per page.
    sidebarLinks: await tab.$$eval("[data-docs='sidebar'] a", (a) => a.map((x) => x.getAttribute("href"))),
    current: await tab.$$eval("[data-docs='sidebar'] a[aria-current='page']", (a) => a.map((x) => x.getAttribute("href"))),
    tocLinks: await tab.$$eval("[data-docs='toc'] a", (a) => a.length),
    articleH2: await tab.$$eval("[data-docs='article'] h2", (h) => h.length),
    tocPresent: (await tab.$$(".doc-toc")).length,
    pagerLinks: await tab.$$eval("[data-docs='pager'] a", (a) => a.map((x) => x.getAttribute("href"))),
    title: await tab.title(),
  };
  await tab.close();
}
await browser.close();
console.log(JSON.stringify(seen));
"""


def pages() -> list[str]:
    """Every page `nav.js` lists, in reading order."""
    source = (DOCS / "nav.js").read_text(encoding="utf-8")
    # Only the `SECTIONS` literal, so a pair written in a comment or in the
    # pager's own code cannot add a page that does not exist.
    body = source.split("export const SECTIONS = [", 1)[-1].split("\n];", 1)[0]
    return [href for href, _ in PAGE_PAIR.findall(body)]


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def expected(order: list[str], page: str) -> int:
    """How many pager links this page should have.

    Two, except at the two ends of the reading order — `nav.js` emits a spacer
    rather than a link there, so the count is one.
    """
    at = order.index(page)
    return (1 if at == 0 else 0) + (1 if at == len(order) - 1 else 0) or 2


def check(root: Path = ROOT) -> tuple[int, list[str]]:
    """`(pages checked, problems)`. A missing browser is a problem, not a skip."""
    order = pages()
    problems: list[str] = []
    if len(order) < 2:
        return 0, [
            f"nav.js lists {len(order)} pages; SECTIONS is not the shape "
            f"`pages()` reads, so nothing below was checked"
        ]

    on_disk = {p.name for p in DOCS.glob("*.html")}
    for page in order:
        if page not in on_disk:
            problems.append(f"nav.js lists docs/{page}, which is not on disk")
    for name in sorted(on_disk - set(order)):
        problems.append(f"docs/{name} exists and no nav.js section lists it")
    if problems:
        return 0, problems

    port = free_port()

    class Quiet(http.server.SimpleHTTPRequestHandler):
        """The stock handler, minus its access log.

        Ten pages pull a stylesheet, a module and three fonts each, so the
        default logging buries this check's own output in sixty request lines
        on a green run. `workbench.py` serves one page and does not have the
        problem.
        """

        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=str(SITE), **kwargs)

        def log_message(self, format: str, *args: object) -> None:
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Quiet)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    driver = RUNNER / "pages-check.mjs"
    driver.write_text(DRIVER)
    try:
        result = subprocess.run(
            ["node", str(driver), f"http://127.0.0.1:{port}", *order],
            cwd=RUNNER,
            capture_output=True,
            text=True,
            timeout=300,
        )
    finally:
        driver.unlink(missing_ok=True)
        server.shutdown()

    if result.returncode != 0:
        return 0, [
            "the browser driver exited non-zero",
            result.stdout.strip()[-2000:],
            result.stderr.strip()[-2000:],
        ]

    seen = json.loads(result.stdout.strip().splitlines()[-1])
    for page in order:
        got = seen[page]
        for problem in got["problems"]:
            problems.append(f"{page}: {problem}")
        if len(got["sidebarLinks"]) != len(order):
            problems.append(
                f"{page}: the sidebar has {len(got['sidebarLinks'])} links and "
                f"nav.js lists {len(order)} pages"
            )
        if got["current"] != [page]:
            problems.append(
                f"{page}: the sidebar marks {got['current']} as the current "
                f"page, not [{page!r}]"
            )
        # A page with one `h2` has no contents *by design* — one heading is a
        # restatement of the title — and `nav.js` removes the whole column
        # rather than rendering a list of one.
        if got["articleH2"] < 2:
            if got["tocPresent"]:
                problems.append(
                    f"{page}: has {got['articleH2']} h2 and the contents column "
                    f"is still on the page"
                )
        elif got["tocLinks"] != got["articleH2"]:
            problems.append(
                f"{page}: the contents has {got['tocLinks']} entries and the "
                f"article has {got['articleH2']} h2"
            )
        if len(got["pagerLinks"]) != expected(order, page):
            problems.append(
                f"{page}: the pager has {len(got['pagerLinks'])} links, "
                f"expected {expected(order, page)}"
            )
        if not got["title"].strip():
            problems.append(f"{page}: the document has no title")
    return len(order), problems


def main() -> int:
    counted, problems = check()
    for problem in problems:
        print(f"FAIL  {problem}")
    if problems:
        print(f"\n{len(problems)} problem(s)")
        return 1
    print(
        f"ok    {counted} documentation pages render, each with a full sidebar, "
        f"a contents derived from its own headings, and a pager"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
