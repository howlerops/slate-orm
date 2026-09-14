#!/usr/bin/env python3
"""Drive the workbench in a real browser and check it answers.

The home page is the only part of the site that *runs* rather than describes,
so it is the only part that can be broken in a way reading cannot catch: a
renamed field in the binding, a control wired to nothing, a wasm bundle that
never loads. The quickstart checker executes the docs page's code snippets for
the same reason; this executes the application.

    python3 site/check/workbench.py

Needs the wasm built (`sh site/build-wasm.sh`) and Playwright's Chromium,
which the demo's frontend already depends on.

What is asserted here and not in `crates/slate-wasm/tests/`: only the things
that are about the *browser*. Whether a query returns the right rows is a
Rust test — faster, and able to say which row was wrong. Whether clicking a
table in the tree puts rows on screen cannot be, and that is this file's job.
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
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
const problems = [];
page.on("pageerror", (e) => problems.push(`page error: ${e}`));
page.on("console", (m) => { if (m.type() === "error") problems.push(`console: ${m.text()}`); });

await page.goto(url, { waitUntil: "load" });

// The grid appearing *is* the "did the kernel come up" assertion: nothing
// renders a row until the wasm module has loaded, seeded and answered.
// The default query is a grouped scan over 100,000 real trips, so a *row*
// on screen means the wasm loaded, the 1.2 MB trip file arrived, decoded,
// seeded and was analysed, and the planner answered. One assertion for the
// whole chain.
await page.waitForFunction(
  () => document.querySelector('[data-app="grid"] tbody tr'),
  { timeout: 90000 },
);

const read = async () => ({
  status: await page.locator('[data-app="status"]').innerText(),
  headers: await page.locator('[data-app="grid"] th').allInnerTexts(),
  rows: await page.locator('[data-app="grid"] tbody tr').count(),
  // Cells the plan never decoded. Rendering these as the word "null" would
  // be a claim about the data that is not true.
  unread: await page.locator('[data-app="grid"] td.unread').count(),
  refusal: await page.locator('[data-app="grid"] .refusal').count(),
});

const type = async (sql) => {
  await page.locator('[data-app="editor"]').fill(sql);
  await page.locator('[data-app="run"]').click();
  await page.waitForTimeout(160);
  return read();
};

const out = {};
out.engine = await page.locator('[data-app="engine"]').innerText();
out.initial = await read();
out.initialFirstRow = await page.locator('[data-app="grid"] tbody tr').first().innerText();

// The real dataset. These are facts about January 2024, not about the code:
// if the sample is replaced or the join keys the wrong columns, they change.
out.busiest = await type(
  "SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone ORDER BY count(*) DESC LIMIT 5",
);
out.busiestRows = await page.locator('[data-app="grid"] tbody tr').allInnerTexts();
out.named = await type(
  "SELECT * FROM trips JOIN zones ON trips.pickup_zone = zones.id WHERE trips.pickup_zone = 132 LIMIT 5",
);
out.namedFirstRow = await page.locator('[data-app="grid"] tbody tr').first().innerText();
out.counts = await type(
  "SELECT payment, count(*), count(passengers) FROM trips GROUP BY payment",
);
out.countRows = await page.locator('[data-app="grid"] tbody tr').allInnerTexts();

// 1. The schema tree describes the database, including which column carries
//    an index — the one fact that makes the rest of the page worth reading.
out.tables = await page.locator(".tree-name").allInnerTexts();
out.indexMarks = await page.locator(".tree-columns .mark.idx").allInnerTexts();
out.keyMarks = await page.locator(".tree-columns .mark.key").allInnerTexts();

// 2. Clicking a table runs a query for it, rather than only filling the box.
await page.locator('.tree-name[data-table="zones"]').click();
await page.waitForTimeout(160);
out.clickedTable = { ...(await read()), sql: await page.locator('[data-app="editor"]').inputValue() };

// 3. Clicking a column inserts it at the caret.
await page.locator('[data-app="editor"]').fill("SELECT  FROM trips");
await page.locator('[data-app="editor"]').evaluate((el) => { el.selectionStart = el.selectionEnd = 7; });
await page.locator('.tree-column[data-column="pickup_zone"]').first().click();
out.insertedColumn = await page.locator('[data-app="editor"]').inputValue();

// 4. The central claim, typed: same query, narrower projection, index-only.
out.wide = await type("SELECT * FROM trips WHERE pickup_zone = 132 LIMIT 200");
await page.locator('[data-tab="plan"]').click();
out.widePlan = await page.locator('[data-app="plan"]').innerText();

out.narrow = await type("SELECT pickup_zone FROM trips WHERE pickup_zone = 132 LIMIT 200");
out.narrowPlan = await page.locator('[data-app="plan"]').innerText();
out.badges = await page.locator('[data-app="badges"] .badge').allInnerTexts();

// 5. The spec tab shows what the SQL compiled to. This is the page's whole
//    honesty claim: SQL is a front end over the structure the SDKs send.
await page.locator('[data-tab="spec"]').click();
out.spec = await page.locator('[data-app="spec"]').innerText();
await page.locator('[data-tab="results"]').click();

// 6. A refusal is shown where the reader is looking, with the caret moved to
//    the offending token.
out.refused = await type("SELECT * FROM trips WHERE pickup_zone = 132 AND nosuch = 1");
out.caret = await page.locator('[data-app="editor"]').evaluate((el) => el.selectionStart);

// 7. A join, and a grouped join.
out.join = await type(
  "SELECT * FROM authors JOIN books ON authors.id = books.author_id WHERE country = 'US' LIMIT 20",
);
out.grouped = await type(
  "SELECT count(*), max(year) FROM authors JOIN books ON authors.id = books.author_id GROUP BY country",
);

// 8. A write, and the index answering for it in the same breath.
const before = await type("SELECT pickup_zone FROM trips WHERE pickup_zone = 7");
out.beforeWrite = before;
out.afterWrite = await type(
  "INSERT INTO trips VALUES (999001, 7, 1, 1704067200, 600, 2, 5.5, 25.0, 3.0, 31.0, 'cash');\\n" +
  "SELECT pickup_zone FROM trips WHERE pickup_zone = 7",
);
await page.locator('[data-tab="plan"]').click();
out.afterWritePlan = await page.locator('[data-app="plan"]').innerText();
await page.locator('[data-tab="results"]').click();

// 9. Reset puts the fixture back, so a reader cannot wreck the page for good.
//    It drops the 100,000 trips with everything else — the store is rebuilt
//    from scratch — so the button re-seeds them from the bytes it kept. Before
//    it did, this check reported zero rows where 72 were expected: a Reset
//    that emptied the main table and left it empty.
await page.locator('[data-app="reset"]').click();
await page.waitForTimeout(400);
out.afterResetStatus = await page.locator('[data-app="status"]').innerText();
out.afterReset = await type("SELECT pickup_zone FROM trips WHERE pickup_zone = 7");

// 9b. The storage browser: a top-level view of the whole database as folders.
out.modes = await page.locator("[data-mode]").allInnerTexts();
await page.locator('[data-mode="storage"]').click();
await page.waitForTimeout(900);
out.storage = {
  // The query console has to go away, or the two views stack. `display: grid`
  // beats the browser's own `[hidden]` rule, which is exactly how they did.
  consoleHidden: await page.locator(".console").isHidden(),
  treeHidden: await page.locator(".tree").isHidden(),
  keyTotal: await page.locator('[data-app="keytotal"]').innerText(),
  bucketTotal: await page.locator('[data-app="buckettotal"]').innerText(),
  folders: await page.locator(".fs-folder > summary").allInnerTexts(),
  leaves: await page.locator(".fs-leaf > summary").allInnerTexts(),
};
await page.locator('.fs-leaf:has-text("trips/")').first().click();
await page.waitForTimeout(400);
out.storage.keys = await page.locator(".fs-keys .fs-key").allInnerTexts();
out.storage.pager = await page.locator(".fs-more").first().innerText();
await page.locator(".fs-more").first().click();
await page.waitForTimeout(300);
out.storage.afterPaging = await page.locator(".fs-keys .fs-key").count();
// The *keys*, not just the count: a pager that fetched page one twice would
// still show fifty rows. It did, until this line existed.
out.storage.pagedKeys = await page.locator(".fs-keys .fs-key code").allInnerTexts();
out.bucketRows = await page.locator(".bk-row").allInnerTexts();
await page.locator('[data-mode="query"]').click();
await page.waitForTimeout(200);
out.backToQuery = await page.locator(".console").isVisible();

// 10. The log kept every statement.
await page.locator('[data-tab="log"]').click();
out.log = await page.locator('[data-app="log"] .entry').count();

// 11. Ctrl+Enter runs, because an editor that only has a button is not one.
await page.locator('[data-tab="results"]').click();
await page.locator('[data-app="editor"]').fill("SELECT * FROM authors LIMIT 3");
await page.locator('[data-app="editor"]').press("Control+Enter");
await page.waitForTimeout(160);
out.keyboard = await read();

// 12. The docs are one click away, which is the other half of the request
//     this page was built for.
out.docsLink = await page.locator('header a[href="docs.html"]').count();

out.problems = problems;
console.log(JSON.stringify(out));
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

    driver = RUNNER / "workbench-check.mjs"
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
        "the kernel loads and the editor answers on arrival",
        seen["initial"]["rows"] > 0,
        f"initial: {seen['initial']}",
    )
    check(
        "the real trip file loads and the engine says how much",
        "100,000 trips" in seen["engine"] and "265 zones" in seen["engine"],
        f"engine: {seen['engine']!r}",
    )
    check(
        "the busiest pickup zone is JFK, as it is in the real month",
        seen["busiestRows"] and seen["busiestRows"][0].split()[0] == "132",
        f"got {seen['busiestRows'][:3]}",
    )
    check(
        "the zone join puts the TLC's own names on a trip",
        "JFK Airport" in seen["namedFirstRow"] and "Queens" in seen["namedFirstRow"],
        f"{seen['namedFirstRow']!r}",
    )
    check(
        "count(*) and count(column) differ, because the data has real nulls",
        any(
            len(row.split("\t")) == 3 and row.split("\t")[1] != row.split("\t")[2]
            for row in seen["countRows"]
        ),
        f"{seen['countRows']}",
    )
    check(
        "the schema tree lists every table",
        len(seen["tables"]) == 4 and all("cols" in t for t in seen["tables"]),
        f"tables: {seen['tables']}",
    )
    check(
        "and marks the indexed columns and the primary keys",
        len(seen["indexMarks"]) == 2 and len(seen["keyMarks"]) == 4,
        f"idx: {seen['indexMarks']}, pk: {seen['keyMarks']}",
    )
    check(
        "clicking a table queries it rather than only typing",
        seen["clickedTable"]["rows"] > 0
        and "zones" in seen["clickedTable"]["sql"]
        and seen["clickedTable"]["headers"][:2] == ["ID", "BOROUGH"],
        f"{seen['clickedTable']}",
    )
    check(
        "clicking a column inserts it at the caret",
        seen["insertedColumn"] == "SELECT pickup_zone FROM trips",
        f"got {seen['insertedColumn']!r}",
    )
    check(
        "a filter on the indexed column plans as a table scan",
        "Table Scan" in seen["widePlan"],
        f"got {seen['widePlan']!r}",
    )
    check(
        "narrowing the projection makes the same query index-only",
        "Index Only Scan" in seen["narrowPlan"],
        f"got {seen['narrowPlan']!r}",
    )
    check(
        "and the index-only plan is marked as the good one",
        any("Index Only" in b for b in seen["badges"]),
        f"badges: {seen['badges']}",
    )
    check(
        "the plan reports its estimate beside what actually came back",
        any(b.startswith("est.") for b in seen["badges"])
        and any(b.startswith("actual") for b in seen["badges"]),
        f"badges: {seen['badges']}",
    )
    check(
        "a column the plan never read is not rendered as a null",
        seen["narrow"]["unread"] > 0 and seen["wide"]["unread"] == 0,
        f"narrow {seen['narrow']['unread']} unread, wide {seen['wide']['unread']}",
    )
    check(
        "the spec tab shows what the SQL compiled to",
        '"table": "trips"' in seen["spec"] and '"columns"' in seen["spec"],
        f"spec: {seen['spec']!r}",
    )
    check(
        "a refusal is shown in the results pane, not swallowed",
        seen["refused"]["refusal"] == 1 and "nosuch" in seen["refused"]["status"],
        f"{seen['refused']}",
    )
    check(
        "and the caret moves to the token that was wrong",
        seen["caret"] == len("SELECT * FROM trips WHERE pickup_zone = 132 AND "),
        f"caret at {seen['caret']}",
    )
    check(
        "a join returns joined rows with both tables' columns",
        seen["join"]["rows"] > 0
        and any(h.startswith("AUTHORS.") for h in seen["join"]["headers"])
        and any(h.startswith("BOOKS.") for h in seen["join"]["headers"]),
        f"{seen['join']}",
    )
    check(
        "a grouped join returns the key and the aggregates",
        seen["grouped"]["rows"] > 0
        and seen["grouped"]["headers"] == ["COUNTRY", "COUNT(*)", "MAX(YEAR)"],
        f"{seen['grouped']}",
    )
    check(
        "an inserted row appears, and the index answers for it",
        seen["afterWrite"]["rows"] == seen["beforeWrite"]["rows"] + 1
        and "Index Only Scan" in seen["afterWritePlan"],
        f"{seen['beforeWrite']['rows']} then {seen['afterWrite']['rows']}, "
        f"plan {seen['afterWritePlan']!r}",
    )
    check(
        "reset puts the fixture back",
        seen["afterReset"]["rows"] == seen["beforeWrite"]["rows"],
        f"{seen['afterReset']['rows']} rows, expected {seen['beforeWrite']['rows']}",
    )
    storage = seen["storage"]
    check(
        "storage is a top-level view, not something buried in a tab",
        seen["modes"] == ["Query", "Storage"],
        f"{seen['modes']}",
    )
    check(
        "and switching to it puts the query console away",
        storage["consoleHidden"] and storage["treeHidden"],
        f"console hidden {storage['consoleHidden']}, tree hidden {storage['treeHidden']}",
    )
    check(
        "the whole database is there: every table and every index",
        len(storage["leaves"]) == 6
        and any("trips/" in l and "100,000 keys" in l for l in storage["leaves"])
        and any("zones/" in l and "265 keys" in l for l in storage["leaves"]),
        f"{storage['leaves']}",
    )
    check(
        "rows and index entries are separate prefixes of one space",
        len(storage["folders"]) == 2
        and storage["folders"][0].startswith("rows/")
        and storage["folders"][1].startswith("index/"),
        f"{storage['folders']}",
    )
    check(
        "an index entry is one key per row, and carries no value",
        any(
            "by_pickup_zone/" in l and "100,000 keys" in l and "0 B values" in l
            for l in storage["leaves"]
        ),
        f"{storage['leaves']}",
    )
    check(
        "opening a folder shows real keys, with the layout's own header",
        storage["keys"]
        and storage["keys"][0].startswith("01 00000003 | ")
        and "trips row id=" in storage["keys"][0],
        f"{storage['keys'][:1]}",
    )
    check(
        "and 100,000 keys are paged rather than rendered at once",
        "25 of 100,000" in storage["pager"] and storage["afterPaging"] == 50,
        f"pager {storage['pager']!r}, after paging {storage['afterPaging']}",
    )
    check(
        "and the second page is the next keys, not the first ones again",
        len(set(storage["pagedKeys"])) == len(storage["pagedKeys"]) == 50,
        f"{len(storage['pagedKeys'])} keys, {len(set(storage['pagedKeys']))} distinct",
    )
    check(
        "the bucket listing is a real one: SST, WAL and manifest",
        any(".sst" in r and "compacted" in r for r in seen["bucketRows"])
        and any("wal/" in r for r in seen["bucketRows"])
        and any("manifest" in r for r in seen["bucketRows"]),
        f"{seen['bucketRows'][:3]}",
    )
    check(
        "and switching back returns to the query console",
        seen["backToQuery"],
        "the console did not come back",
    )
    check(
        "the log keeps every statement that ran",
        seen["log"] >= 10,
        f"{seen['log']} entries",
    )
    check(
        "ctrl+enter runs the statement",
        seen["keyboard"]["rows"] == 3,
        f"{seen['keyboard']}",
    )
    check(
        "the docs are one click from the workbench",
        seen["docsLink"] >= 1,
        "no header link to docs.html",
    )
    check("no page or console errors", not seen["problems"], f"{seen['problems']}")

    print()
    if failures:
        print(f"{len(failures)} failed: {', '.join(failures)}")
        return 1
    print("the workbench runs the kernel in a browser")
    return 0


if __name__ == "__main__":
    sys.exit(main())
