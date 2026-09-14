#!/usr/bin/env python3
"""Drive the playground in a real browser and check it answers.

The panel is the only part of the site that *runs* rather than describes, so it
is the only part that can be broken in a way reading cannot catch: a renamed
field in the binding, a control wired to nothing, a wasm bundle that never
loads. The quickstart checker executes the page's code snippets for the same
reason; this executes the page.

    python3 site/check/playground.py

Needs the wasm built (`sh site/build-wasm.sh`) and Playwright's Chromium,
which the demo's frontend already depends on.
"""

from __future__ import annotations

import http.server
import json
import socket
import subprocess
import sys
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SITE = ROOT / "site"
#: The demo's frontend is where Playwright lives; running from there keeps a
#: second copy of a heavyweight dependency out of the tree.
RUNNER = ROOT / "examples" / "explorer" / "web"

DRIVER = """
import { chromium } from "playwright";
const url = process.argv[2];
const browser = await chromium.launch();
const page = await browser.newPage();
const problems = [];
page.on("pageerror", (e) => problems.push(`page error: ${e}`));
page.on("console", (m) => { if (m.type() === "error") problems.push(`console: ${m.text()}`); });

// Record every request, so "the bundle is not fetched until asked for" is
// an observation rather than a claim about the code.
const fetched = [];
page.on("request", (r) => fetched.push(r.url()));

await page.goto(url, { waitUntil: "load" });

// Deliberately *before* scrolling or clicking: at this point the panel is
// below the fold and the wasm must not have been requested.
await page.waitForTimeout(400);
const eagerlyFetched = fetched.filter((u) => u.endsWith(".wasm"));

// Clicked only if it is still clickable. If something already started the
// load the button is disabled by now, and clicking would time out with a
// message about element stability rather than about the thing that is
// actually wrong — which is what the `eagerlyFetched` count below reports.
const load = page.locator('[data-play="load"]');
if (await load.isEnabled()) await load.click();

// The panel stays hidden until the module loads, so its visibility *is* the
// "did wasm come up" assertion.
await page.waitForSelector(".play:not([hidden])", { timeout: 60000 });
const lazilyFetched = fetched.filter((u) => u.endsWith(".wasm"));

const read = async () => {
  await page.waitForTimeout(120);
  return {
    badges: await page.locator('[data-play="badges"] .badge').allInnerTexts(),
    plan: await page.locator('[data-play="plan"]').innerText(),
    status: await page.locator('[data-play="status"]').innerText(),
    rows: await page.locator('[data-play="rows"] tbody tr').count(),
    columns: await page.locator('[data-play="columns"] input').count(),
  };
};

// 1. It loads and answers something.
const initial = await read();

// 2. Filtering on the indexed column, reading every column: a table scan,
//    because the point reads an index scan implies cost more than the scan.
await page.locator('[data-play="column"]').selectOption({ index: 1 });
await page.locator('[data-play="op"]').selectOption("eq");
await page.locator('[data-play="value"]').fill("1");
const scan = await read();

// 3. Narrow to the indexed column alone and the plan becomes index-only.
const boxes = page.locator('[data-play="columns"] input');
const count = await boxes.count();
for (let i = 0; i < count; i++) if (i !== 1) await boxes.nth(i).uncheck();
const covering = await read();

// 4. A refusal is shown, not swallowed.
for (let i = 0; i < count; i++) await boxes.nth(i).check();
await page.locator('[data-play="value"]').fill("not-a-number");
const refused = await read();

// 5. A write, and the index answering for it. The panel's whole claim about
//    being a record layer rather than a query engine.
await page.locator('[data-play="value"]').fill("2");
const beforeWrite = await read();
const fields = page.locator('[data-play="fields"] input');
for (const [i, v] of ["9100", "2", "A New Book", "2001"].entries()) {
  await fields.nth(i).fill(v);
}
await page.locator('[data-play="insert"]').click();
const afterWrite = await read();

// 6. Two conditions.
await page.locator('[data-play="value"]').fill("1");
await page.locator('[data-play="column2"]').selectOption({ index: 3 });
await page.locator('[data-play="op2"]').selectOption("gt");
await page.locator('[data-play="value2"]').fill("1970");
const conjunction = await read();

// 7. The join panel.
await page.locator(".play-join summary").click();
await page.locator('[data-play="groupby"]').selectOption("2");
await page.locator('[data-play="runjoin"]').click();
await page.waitForTimeout(300);
const join = {
  plan: await page.locator('[data-play="joinplan"]').innerText(),
  status: await page.locator('[data-play="joinstatus"]').innerText(),
  rows: await page.locator('[data-play="joinrows"] tbody tr').count(),
};

console.log(JSON.stringify({
  beforeWrite, afterWrite, conjunction, join,
  initial, scan, covering, refused, problems,
  eagerlyFetched: eagerlyFetched.length,
  lazilyFetched: lazilyFetched.length,
}));
await browser.close();
"""


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def main() -> int:
    for needed in ("slate_wasm.js", "slate_wasm_bg.wasm"):
        if not (SITE / needed).exists():
            print(f"FAIL  {needed} is not built; run `sh site/build-wasm.sh`")
            return 1

    port = free_port()
    handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(  # noqa: E731
        *a, directory=str(SITE), **k
    )
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    driver = RUNNER / "playground-check.mjs"
    driver.write_text(DRIVER)
    try:
        result = subprocess.run(
            ["node", str(driver), f"http://127.0.0.1:{port}/index.html"],
            cwd=RUNNER,
            capture_output=True,
            text=True,
            timeout=180,
        )
    finally:
        driver.unlink(missing_ok=True)
        server.shutdown()

    if result.returncode != 0:
        print("FAIL  the browser driver exited non-zero")
        print(result.stdout.strip())
        print(result.stderr.strip())
        return 1

    seen = json.loads(result.stdout.strip().splitlines()[-1])
    failures: list[str] = []

    def check(name: str, ok: bool, detail: str) -> None:
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            failures.append(name)
            print(f"        {detail}")

    check(
        "the wasm module loads and the panel answers",
        seen["initial"]["rows"] > 0 and bool(seen["initial"]["plan"]),
        f"initial: {seen['initial']}",
    )
    check(
        "a filter on the indexed column plans as a table scan",
        "Table Scan" in seen["scan"]["plan"],
        f"got {seen['scan']['plan']!r}",
    )
    check(
        "narrowing to the indexed column alone makes it index-only",
        "Index Only Scan" in seen["covering"]["plan"],
        f"got {seen['covering']['plan']!r}",
    )
    check(
        "and the covering plan is marked as the good one",
        any("Index Only" in b for b in seen["covering"]["badges"]),
        f"badges: {seen['covering']['badges']}",
    )
    check(
        "the plan's decoded columns narrow with the projection",
        any("decodes [1]" in b for b in seen["covering"]["badges"]),
        f"badges: {seen['covering']['badges']}",
    )
    check(
        "a bad literal is refused in the panel rather than swallowed",
        "whole number" in seen["refused"]["status"],
        f"status: {seen['refused']['status']!r}",
    )
    check(
        "the wasm is not fetched until the reader asks for it",
        seen["eagerlyFetched"] == 0,
        f"{seen['eagerlyFetched']} wasm request(s) before the button was clicked",
    )
    check(
        "and it is fetched once they do",
        seen["lazilyFetched"] >= 1,
        "the panel came up without fetching any wasm, which cannot be right",
    )
    check(
        "an inserted row appears in the current query",
        seen["afterWrite"]["rows"] == seen["beforeWrite"]["rows"] + 1,
        f"{seen['beforeWrite']['rows']} then {seen['afterWrite']['rows']}",
    )
    check(
        "a second condition narrows the result",
        seen["conjunction"]["rows"] == 3,
        f"Le Guin after 1970 should be three books, got {seen['conjunction']['rows']}",
    )
    check(
        "the join panel groups and shows a join plan",
        "Join" in seen["join"]["plan"] and seen["join"]["rows"] >= 1,
        f"{seen['join']}",
    )
    check("no page or console errors", not seen["problems"], f"{seen['problems']}")

    print()
    if failures:
        print(f"{len(failures)} failed: {', '.join(failures)}")
        return 1
    print("the playground runs the kernel in a browser")
    return 0


if __name__ == "__main__":
    sys.exit(main())
