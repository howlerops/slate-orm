#!/usr/bin/env python3
"""Drive the workbench in a real browser and check it answers.

The home page is the only part of the site that *runs* rather than describes,
so it is the only part that can be broken in a way reading cannot catch: a
renamed field in the binding, a control wired to nothing, a wasm bundle that
never loads. The quickstart checker executes the docs page's code snippets for
the same reason; this executes the application.

    python3 site/check/workbench.py

The page under test is `site/workbench.html`. It was `index.html` until the
landing page took that name; the constant below is spelled once so the move
could not leave this opening a 404 — which would render as a blank document,
and a blank document has no console errors to fail on.

Needs the wasm built (`sh site/build-wasm.sh`) and Playwright's Chromium,
which the demo's frontend already depends on.

What is asserted here and not in `crates/slate-wasm/tests/`: only the things
that are about the *browser*. Whether a query returns the right rows is a
Rust test — faster, and able to say which row was wrong. Whether clicking a
table in the tree puts rows on screen cannot be, and that is this file's job.
"""

from __future__ import annotations

import http.server
import itertools
import json
import re
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
// The Log tab times each statement separately, and a write is the one the
// status bar cannot show on its own — a buffer's total is one number. This is
// the only place a reader sees what the index maintenance cost, so check the
// write actually carries a number rather than a blank where one should be.
await page.locator('[data-tab="log"]').click();
out.writeLog = await page.locator('[data-app="log"] .entry:not(.bad)').allInnerTexts();
out.failedLog = await page.locator('[data-app="log"] .entry.bad').allInnerTexts();
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

// 9c. The kitchen sink, clicked from the sidebar like a reader would.
//     Every example is executed by `crates/slate-wasm/tests/examples.rs`;
//     this is the one that has to survive the *click*, because it is the only
//     one that is two statements and the only one with a semicolon in it.
await page.locator('.examples button:has-text("Kitchen sink")').click();
await page.waitForTimeout(500);
out.sink = {
  status: await page.locator('[data-app="status"]').innerText(),
  headers: await page.locator('[data-app="grid"] th').allInnerTexts(),
  rows: await page.locator('[data-app="grid"] tbody tr').count(),
  refusal: await page.locator('[data-app="grid"] .refusal').count(),
};
await page.locator('[data-tab="log"]').click();
out.sink.logged = await page.locator('[data-app="log"] .entry').count();
await page.locator('[data-tab="results"]').click();

// 9c2. HAVING filters the groups, and the refusals hold.
//      In the browser rather than only in Rust because the clause reaches the
//      kernel through the wasm boundary and a spec field that has to survive
//      serde on the way — the tests either side of that boundary would both
//      pass with the field dropped in the middle.
out.grouped226 = await type("SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone");
out.havingFiltered = await type(
  "SELECT pickup_zone, count(*), avg(duration) FROM trips GROUP BY pickup_zone " +
  "HAVING count(*) > 300 AND avg(duration) > 900",
);
out.havingRows = await page.locator('[data-app="grid"] tbody tr').allInnerTexts();
// An integer literal against `avg` over an integer column. If the literal were
// typed from the column it would be an I64, and F64 > I64 is true by class
// rank, so all 226 zones would come back rather than 110.
out.havingTyped = await type(
  "SELECT pickup_zone, avg(duration) FROM trips GROUP BY pickup_zone HAVING avg(duration) > 1500",
);
out.havingRefused = await type("SELECT * FROM trips HAVING count(*) > 1");
out.havingRefusal = await page.locator('[data-app="grid"] .refusal').innerText();

// 9c3. Time functions: a computed column, grouped, through the wasm boundary.
//      The Rust suite covers the arithmetic; what this covers is that a
//      `compute` field survives serde in both directions and that the header
//      says what the column is rather than printing its ordinal.
out.byHour = await type(
  "SELECT hour(pickup_time), count(*) FROM trips GROUP BY hour(pickup_time) " +
  "ORDER BY hour(pickup_time)",
);
out.byHourRows = await page.locator('[data-app="grid"] tbody tr').allInnerTexts();
await page.locator('[data-tab="spec"]').click();
out.byHourSpec = await page.locator('[data-app="spec"]').innerText();
await page.locator('[data-tab="results"]').click();
// ClickHouse's Q4, which could not be written here at all until year() existed.
out.clickhouse = await type(
  "SELECT passengers, year(pickup_time), round(distance), count(*) FROM trips " +
  "GROUP BY passengers, year(pickup_time), round(distance) " +
  "ORDER BY year(pickup_time), count(*) DESC LIMIT 20",
);
out.clickhouseHeaders = await page.locator('[data-app="grid"] th').allInnerTexts();
// A timestamp is an integer of seconds, so this cannot mean anything.
out.notATimestamp = await type("SELECT hour(payment), count(*) FROM trips GROUP BY hour(payment)");
out.notATimestampWhy = await page.locator('[data-app="grid"] .refusal').innerText();

// 9c4. A fixed timezone offset, which is `Add` underneath and so travels the
//      same serde path as any other computed column — with one extra field
//      that a spec round trip could drop while every Rust test still passed.
out.zoned = await type(
  "SELECT hour(pickup_time, '-05:00'), count(*) FROM trips " +
  "GROUP BY hour(pickup_time, '-05:00') ORDER BY hour(pickup_time, '-05:00')",
);
out.zonedRows = await page.locator('[data-app="grid"] tbody tr').allInnerTexts();
await page.locator('[data-tab="spec"]').click();
out.zonedSpec = await page.locator('[data-app="spec"]').innerText();
await page.locator('[data-tab="results"]').click();
// 9c4b. A *named* zone, resolved through the kernel's transition table rather
//       than refused. January 2024 is wholly Eastern Standard Time, so this
//       must give the same 24 groups as the fixed -05:00 above — a differential
//       between a binary search and arithmetic on a constant, run in the
//       browser where the table has to have survived the wasm build.
out.named = await type(
  "SELECT hour(pickup_time, 'America/New_York'), count(*) FROM trips " +
  "GROUP BY hour(pickup_time, 'America/New_York') " +
  "ORDER BY hour(pickup_time, 'America/New_York')",
);
out.namedRows = await page.locator('[data-app="grid"] tbody tr').allInnerTexts();
await page.locator('[data-tab="spec"]').click();
out.namedSpec = await page.locator('[data-app="spec"]').innerText();
await page.locator('[data-tab="results"]').click();

// And a zone the table does not have is refused by name, with the list. The
// misspelling is the realistic mistake: IANA names are case-sensitive.
out.badZone = await type(
  "SELECT hour(pickup_time, 'america/new_york'), count(*) FROM trips " +
  "GROUP BY hour(pickup_time, 'america/new_york')",
);
out.badZoneWhy = await page.locator('[data-app="grid"] .refusal').innerText();

// 9c5. A computed column on a *join*, which was refused outright until the
//      kernel grew somewhere to put it. Clicked from the sidebar, because the
//      example is the thing a reader will actually run.
//
//      Matched on "hour by hour" rather than on "Manhattan": a substring
//      selector over a list that grows is a collision waiting to happen, and
//      this one happened — adding "Where a Manhattan ride ends up" below made
//      `has-text("Manhattan")` resolve to two buttons and took the whole check
//      down with a strict-mode violation. The distinctive half of a title is
//      the half to match on.
await page.locator('.examples button:has-text("hour by hour")').click();
await page.waitForTimeout(600);
out.joinHour = {
  headers: await page.locator('[data-app="grid"] th').allInnerTexts(),
  rows: await page.locator('[data-app="grid"] tbody tr').allInnerTexts(),
  refusal: await page.locator('[data-app="grid"] .refusal').count(),
};

// 9c6. The key and the aggregate from the *same* table, which the spec could
//      not express until every joined position was resolved in the joined
//      row: a computed column resolved against the left table and an
//      aggregate against the right, so this query had nowhere to land.
await page.locator('.examples button:has-text("the fare")').click();
await page.waitForTimeout(600);
out.joinSameSide = {
  headers: await page.locator('[data-app="grid"] th').allInnerTexts(),
  rows: await page.locator('[data-app="grid"] tbody tr').allInnerTexts(),
  refusal: await page.locator('[data-app="grid"] .refusal').count(),
};

// 9c7. A group key that is a bare column of the *right* table, which is
//      shifted by the left table's width rather than by nothing. Getting it
//      wrong reads `trips.pickup_zone` and returns 260 numeric groups where
//      this asks for a handful of borough names.
await page.locator('.examples button:has-text("Boroughs")').click();
await page.waitForTimeout(600);
out.joinRightKey = {
  headers: await page.locator('[data-app="grid"] th').allInnerTexts(),
  rows: await page.locator('[data-app="grid"] tbody tr').allInnerTexts(),
  refusal: await page.locator('[data-app="grid"] .refusal').count(),
};

// 9c7a2. A subquery: `pickup_zone IN (SELECT id FROM zones WHERE borough =
//        'Brooklyn')`. The inner query runs once and its single column
//        becomes the candidate list of an ordinary IN, so what is checked is
//        that the list arrived — an empty one matches nothing and would come
//        back as a confident zero.
await page.locator('.examples button:has-text("Brooklyn rides begin")').click();
await page.waitForTimeout(900);
out.subquery = {
  headers: await page.locator('[data-app="grid"] th').allInnerTexts(),
  rows: await page.locator('[data-app="grid"] tbody tr').allInnerTexts(),
  refusal: await page.locator('[data-app="grid"] .refusal').count(),
};
// The Spec tab keeps both halves, which is the thing the panel exists to show.
await page.locator('[data-tab="spec"]').click();
out.subquerySpec = await page.locator('[data-app="spec"]').innerText();
await page.locator('[data-tab="results"]').click();

// 9c7b. One table read twice under two aliases, which is the query the taxi
//       schema is actually for: `trips` reaches `zones` through both
//       `pickup_zone` and `dropoff_zone`. Until aliases existed this was a
//       refusal — two inputs called `zones` made every column reference
//       ambiguous — so what is checked is that it answers at all, that the
//       group key keeps its qualifier (`borough` alone would name both ends
//       and neither), and that the boroughs are names rather than zone ids.
await page.locator('.examples button:has-text("Manhattan ride ends up")').click();
await page.waitForTimeout(900);
out.aliasedChain = {
  headers: await page.locator('[data-app="grid"] th').allInnerTexts(),
  rows: await page.locator('[data-app="grid"] tbody tr').allInnerTexts(),
  refusal: await page.locator('[data-app="grid"] .refusal').count(),
};
// The plan needs its own tab, and the grid tab has to come back afterwards so
// the next check reads a grid rather than whatever was last shown.
await page.locator('[data-tab="plan"]').click();
out.aliasedChainPlan = await page.locator('[data-app="plan"]').innerText();
await page.locator('[data-tab="results"]').click();

// 9c7c. The same table twice under *one* name, which is still a refusal — and
//       the refusal names the alias as the way out, because now there is one.
await type(
  "SELECT * FROM trips JOIN zones ON trips.pickup_zone = zones.id " +
  "JOIN zones ON trips.dropoff_zone = zones.id",
);
// The refusal's *text*, not the counts `type` returns: what is being checked
// is that the message names the way out, and `read()` only says how many
// refusals there were.
out.aliasCollisionWhy = await page.locator('[data-app="grid"] .refusal').innerText();

// 9c8. Ordering a grouped join's groups, which `JoinSpec` had no field for at
//      all — so the whole clause was refused on a join. `count(*) DESC` rather
//      than the key, because the key order is what comes back anyway.
out.joinOrdered = await type(
  "SELECT borough, count(*) FROM trips JOIN zones " +
  "ON trips.pickup_zone = zones.id " +
  "GROUP BY borough ORDER BY count(*) DESC LIMIT 3",
);
out.joinOrderedRows = await page.locator('[data-app="grid"] tbody tr').allInnerTexts();

// 9c9. And an unqualified name both tables have. `trips` and `zones` both have
//      `id`, the rule is left-first, and until now nothing said so on screen.
//      The query runs; the caution above the table is the point.
out.ambiguous = await type(
  "SELECT id, count(*) FROM trips JOIN zones " +
  "ON trips.pickup_zone = zones.id GROUP BY id LIMIT 5",
);
out.ambiguousCaution = await page.locator('[data-app="grid"] .caution').allInnerTexts();
// Qualifying it is the advice the caution gives, so it had better work.
out.qualified = await type(
  "SELECT zones.id, count(*) FROM trips JOIN zones " +
  "ON trips.pickup_zone = zones.id GROUP BY zones.id LIMIT 5",
);
out.qualifiedCaution = await page.locator('[data-app="grid"] .caution').count();

// 9d. The timing in the status bar is the kernel's, not the round trip.
//     A grouped query returning 226 rows spends ~1% of its wall clock on
//     JSON, so no breakdown is shown; `SELECT *` over 100,000 rows spends
//     most of it there, so it is. One number for both would say this
//     database is slow at `SELECT *` when what is slow is serde_json.
// Named `timedGroup`, not `grouped`: `out.grouped` is the grouped *join*
// several steps above, and reusing the name silently overwrote it — the join
// check then read a trips group-by and failed on its headers.
out.timedGroup = await type("SELECT pickup_zone, count(*) FROM trips GROUP BY pickup_zone");
out.groupedStatus = await page.locator('[data-app="status"]').innerText();
// 100,000 rows: the query is fast and the *table* is what used to freeze the
// tab for 29.5 seconds. The grid is capped now, so this has to come back
// quickly and say what it truncated.
const before100k = Date.now();
out.everything = await type("SELECT * FROM trips");
out.everythingMs = Date.now() - before100k;
out.everythingStatus = await page.locator('[data-app="status"]').innerText();
out.everythingNote = await page.locator('[data-app="grid"] .empty').innerText();

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
out.docsLink = await page.locator('header a[href="docs/index.html"]').count();

// 13. What a write costs, which is the number a record layer most wants to
//     show and the one this page could not show at all until writes were
//     timed. It cannot be asserted on a *single* insert: `performance.now()`
//     is clamped to 0.1 ms in a page that is not cross-origin isolated, and
//     one insert into `trips` — the row and its `by_pickup_zone` entry —
//     takes about 3.5 microseconds, so it reads as `0.00 ms` 293 times out of
//     300. An earlier version of this check asserted `> 0` on one insert and
//     passed by luck.
//
//     Two hundred of them in one buffer sum to ~0.7 ms, which is seven clock
//     grains and a number the status bar can honestly print. Last, because it
//     leaves 200 rows in `trips` and the storage counts above are exact.
// The log accumulates over the whole run and one earlier step fails on
// purpose, so this is a baseline to compare against, not a count of zero.
await page.locator('[data-tab="log"]').click();
out.badBefore = await page.locator('[data-app="log"] .entry.bad').count();
await page.locator('[data-tab="results"]').click();
const many = [];
for (let i = 0; i < 200; i++) {
  many.push(`INSERT INTO trips VALUES (${800000 + i}, 7, 1, 1704067200, 600, 2, 5.5, 25.0, 3.0, 31.0, 'cash')`);
}
await type(many.join(";" + String.fromCharCode(10)));
out.batchStatus = await page.locator('[data-app="status"]').innerText();
await page.locator('[data-tab="log"]').click();
out.batchLogged = await page.locator('[data-app="log"] .entry.bad').count();
await page.locator('[data-tab="results"]').click();

out.problems = problems;
console.log(JSON.stringify(out));
await browser.close();
"""


def stale_sources() -> list[Path]:
    """Source files newer than the wasm bundle that was built from them.

    The bundle is a build artifact, gitignored, and the browser loads whatever
    is on disk — so a check run after editing `slate-wasm` and before
    rebuilding drives a *previous* kernel. Nothing said so: the docstring asked
    for `sh site/build-wasm.sh` and that was the whole enforcement.

    It bit immediately. Three checks for table aliases failed against a bundle
    built ninety minutes earlier, and the failure read as "aliases do not work"
    rather than "you are running last hour's code". The dangerous direction is
    the other one, though: a check that *passes* against a stale bundle is a
    green run that exercised nothing it claims to, which is the failure this
    repository keeps finding in new places.

    So the same guard `slate-serverd` got in task #140, for the same reason and
    by the same rule: a prebuilt artifact older than its sources is a hard
    error, never a warning and never a silent pass.

    Timestamps rather than hashes: the bundle is not reproducible byte for byte
    across toolchains, so a content check would need a manifest the build script
    does not write. `mtime` is coarse and enough — the failure being guarded
    against is minutes or hours old, not milliseconds.
    """
    bundle = (SITE / "slate_wasm_bg.wasm").stat().st_mtime
    watched: list[Path] = []
    for crate in ("slate-wasm", "slate-kernel", "slate-schema", "slate-tuple"):
        watched.extend((ROOT / "crates" / crate / "src").rglob("*.rs"))
        watched.append(ROOT / "crates" / crate / "Cargo.toml")
    # The page's own files are served directly and need no build, so they are
    # deliberately not here: editing `workbench.js` does not stale the bundle.
    return sorted(p for p in watched if p.exists() and p.stat().st_mtime > bundle)


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def main() -> int:
    for needed in ("slate_wasm.js", "slate_wasm_bg.wasm"):
        if not (SITE / needed).exists():
            print(f"FAIL  {needed} is not built; run `sh site/build-wasm.sh`")
            return 1

    stale = stale_sources()
    if stale:
        print(
            f"FAIL  site/slate_wasm_bg.wasm is older than {len(stale)} of its "
            f"sources; run `sh site/build-wasm.sh`"
        )
        for path in stale[:5]:
            print(f"        {path.relative_to(ROOT)}")
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
            ["node", str(driver), f"http://127.0.0.1:{port}/workbench.html"],
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
        and any(
            "trips/" in line and "100,000 keys" in line for line in storage["leaves"]
        )
        and any(
            "zones/" in line and "265 keys" in line for line in storage["leaves"]
        ),
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
            "by_pickup_zone/" in line
            and "100,000 keys" in line
            and "0 B values" in line
            for line in storage["leaves"]
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
    # The listing used to total 21.7 MB for 11.0 MB of rows, because the WAL
    # segment that carried the load was still sitting beside the SST holding
    # the same data. Nothing caught it: every assertion here was about which
    # *kinds* of object appear, and both listings have an SST and a WAL. This
    # one is about the arithmetic, so a regenerated `bucket.json` that
    # double-counts fails rather than quietly reappearing on the page.
    scale = {"B": 1, "KB": 1024, "MB": 1024 * 1024}

    def size_of(row: str) -> float | None:
        found = re.search(r"([\d.]+)\s*(B|KB|MB)\b", row)
        return float(found.group(1)) * scale[found.group(2)] if found else None

    sizes = [n for n in (size_of(r) for r in seen["bucketRows"]) if n is not None]
    total_shown = seen["storage"]["bucketTotal"]
    check(
        "the bucket total does not count the rows twice",
        len(sizes) == len(seen["bucketRows"]) and sum(sizes) < max(sizes) * 1.5,
        f"{total_shown!r}: largest object {max(sizes) if sizes else 0:.0f}, "
        f"total {sum(sizes):.0f} across {len(sizes)}/{len(seen['bucketRows'])} rows",
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
        "the kitchen sink example runs when a reader clicks it",
        seen["sink"]["refusal"] == 0
        and seen["sink"]["rows"] == 20
        and seen["sink"]["headers"][:3] == ["PICKUP_ZONE", "PASSENGERS", "COUNT(*)"]
        and len(seen["sink"]["headers"]) == 9,
        f"{seen['sink']}",
    )
    check(
        "a grouped query reports its time with no JSON breakdown",
        "ms" in seen["groupedStatus"] and "JSON" not in seen["groupedStatus"],
        f"{seen['groupedStatus']!r}",
    )
    check(
        "and a 100,000-row result separates the marshalling from the query",
        "JSON" in seen["everythingStatus"],
        f"{seen['everythingStatus']!r} — the JSON cost should be called out here",
    )
    check(
        "a 100,000-row result does not build 100,000 rows into the page",
        seen["everything"]["rows"] == 1000 and seen["everythingMs"] < 8000,
        f"{seen['everything']['rows']} rows rendered in {seen['everythingMs']} ms",
    )
    check(
        "and it says what it truncated rather than quietly showing less",
        "100,000 rows" in seen["everythingNote"] and "1,000" in seen["everythingNote"],
        f"{seen['everythingNote']!r}",
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
        "no header link to docs/index.html",
    )
    # Every statement in the buffer is timed, the write included. `> 0` rather
    # than merely present: a binding that reported a blank, or a zero, for a
    # write would otherwise pass — which it did, until writes were timed at all.
    # Newest first, and it accumulates over the whole run, so the write is
    # found by what it says rather than by where it sits. A statement that
    # failed is excluded: there is no kernel time to report for a query that
    # never ran, and printing a `0.00 ms` beside a parse error would be a
    # measurement of nothing.
    log = seen["writeLog"]
    timed = [(line, re.search(r"\u00b7 ([0-9.]+) ms", line)) for line in log]
    check(
        "every statement that ran is timed",
        len(log) >= 2 and all(m for _, m in timed),
        f"{sum(1 for _, m in timed if m)} of {len(log)} lines carry a time",
    )
    check(
        "and one that did not run is not",
        seen["failedLog"]
        and not any(re.search(r"\u00b7 [0-9.]+ ms", line) for line in seen["failedLog"]),
        f"{seen['failedLog']}",
    )
    # A single write cannot be asserted non-zero — see the driver's step 13 —
    # so what is checked here is that the write carries a number at all, and
    # the magnitude is checked on a buffer of 200 below.
    writes = [line for line, m in timed if m and "inserted" in line]
    check(
        "the write carries a time of its own",
        len(writes) == 1,
        f"{writes} among {len(log)} log lines",
    )
    batch = re.search(r"([0-9.]+) ms", seen["batchStatus"])
    check(
        "200 writes report what the index maintenance cost",
        seen["batchLogged"] == seen["badBefore"]
        # `batch is not None`, not a bare `batch`: `re.search` returns `None`
        # on no match, and `a and None and b` is `None` — which `check` would
        # have read as a failure by luck rather than by saying so.
        and batch is not None
        and float(batch.group(1)) > 0,
        f"{seen['batchStatus']!r}, "
        f"{seen['batchLogged'] - seen['badBefore']} of the 200 failed",
    )

    check(
        "HAVING keeps only the groups that pass",
        seen["grouped226"]["rows"] == 226 and seen["havingFiltered"]["rows"] == 12,
        f"{seen['grouped226']['rows']} zones, {seen['havingFiltered']['rows']} after HAVING",
    )
    check(
        "and the survivors really do pass both tests",
        all(
            int(cells[1]) > 300 and float(cells[2]) > 900
            for cells in (row.split("\t") for row in seen["havingRows"])
        ),
        f"{seen['havingRows'][:3]}",
    )
    check(
        "an integer literal against a double aggregate still filters",
        seen["havingTyped"]["rows"] == 110,
        f"{seen['havingTyped']['rows']} of 226 — the literal is typed from the column",
    )
    check(
        "and HAVING without a GROUP BY is refused",
        seen["havingRefused"]["refusal"] == 1
        and "GROUP BY" in seen["havingRefusal"],
        f"{seen['havingRefusal']!r}",
    )

    # `innerText` returns *rendered* text and the stylesheet uppercases `th`,
    # so the comparison is case-insensitive. Asserting the exact string here
    # would be asserting the stylesheet.
    lower = [h.lower() for h in seen["byHour"]["headers"]]
    check(
        "a time function groups by a computed column",
        seen["byHour"]["rows"] == 24 and lower[:1] == ["hour(pickup_time)"],
        f"{seen['byHour']['rows']} rows, headers {seen['byHour']['headers']}",
    )
    check(
        "and the hours come back in order, summing to the sample",
        [int(r.split("\t")[0]) for r in seen["byHourRows"]] == list(range(24))
        and sum(int(r.split("\t")[1]) for r in seen["byHourRows"]) == 100_000,
        f"{seen['byHourRows'][:2]}",
    )
    check(
        "the spec carries the computation, not a column name",
        '"compute"' in seen["byHourSpec"] and '"hour"' in seen["byHourSpec"],
        f"{seen['byHourSpec'][:160]!r}",
    )
    check(
        "ClickHouse's taxi Q4 runs as written",
        seen["clickhouse"]["rows"] == 20
        and [h.lower() for h in seen["clickhouseHeaders"]]
        == ["passengers", "year(pickup_time)", "round(distance)", "count(*)"],
        f"{seen['clickhouse']['rows']} rows, {seen['clickhouseHeaders']}",
    )
    check(
        "and a time function on text is refused with the reason",
        seen["notATimestamp"]["refusal"] == 1
        and "timestamp" in seen["notATimestampWhy"],
        f"{seen['notATimestampWhy']!r}",
    )

    # The offset is a rotation, so the two histograms hold the same counts in a
    # different order. Comparing the multisets is what makes this a check on
    # the shift rather than on one hour: a dropped offset gives an identical
    # list, and a wrong one gives a differently-ordered same multiset — so the
    # ordering is asserted too.
    plain = [int(r.split("\t")[1]) for r in seen["byHourRows"]]
    zoned = [int(r.split("\t")[1]) for r in seen["zonedRows"]]
    check(
        "a fixed offset rotates the hours and keeps every trip",
        sorted(zoned) == sorted(plain) and zoned != plain and sum(zoned) == 100_000,
        f"{zoned[:4]} against {plain[:4]}, total {sum(zoned)}",
    )
    check(
        "and it is exactly five hours, not some other rotation",
        zoned == plain[5:] + plain[:5],
        f"{zoned[:6]} against {(plain[5:] + plain[:5])[:6]}",
    )
    check(
        "the offset reaches the spec rather than being applied and forgotten",
        '"offset": -18000' in seen["zonedSpec"].replace("\n", " ")
        or '"offset":-18000' in seen["zonedSpec"].replace("\n", " "),
        f"{seen['zonedSpec'][:200]!r}",
    )
    named = [int(row.split("\t")[1]) for row in seen["namedRows"]]
    check(
        "a named zone is answered, in the browser, from the transition table",
        seen["named"]["refusal"] == 0 and len(named) == 24,
        f"{seen['named']['status']!r} {named[:6]}",
    )
    check(
        "and agrees with the fixed offset it was in: January is standard time",
        named == zoned,
        f"{named[:6]} against {zoned[:6]}",
    )
    check(
        "the zone reaches the spec as a name, not as a constant offset",
        '"zone": "America/New_York"' in seen["namedSpec"].replace("\n", " ")
        or '"zone":"America/New_York"' in seen["namedSpec"].replace("\n", " "),
        f"{seen['namedSpec'][:200]!r}",
    )
    check(
        "a zone the table does not have is refused, and the refusal lists them",
        seen["badZone"]["refusal"] == 1
        and "no such timezone" in seen["badZoneWhy"]
        and "America/New_York" in seen["badZoneWhy"],
        f"{seen['badZoneWhy']!r}",
    )

    ordered = [int(row.split("\t")[1]) for row in seen["joinOrderedRows"]]
    check(
        "a grouped join's groups can be ordered, by an aggregate, in the browser",
        seen["joinOrdered"]["refusal"] == 0
        and len(ordered) == 3
        # `pairwise` rather than `zip(xs, xs[1:])`: it says "successive
        # pairs" and needs no opinion about what to do when the two lengths
        # differ, which they cannot here and which `zip` still makes you
        # state.
        and all(a >= b for a, b in itertools.pairwise(ordered)),
        f"{seen['joinOrdered']['status']!r} {ordered}",
    )
    check(
        "an unqualified name both tables have is called out rather than silent",
        seen["ambiguous"]["refusal"] == 0
        and len(seen["ambiguousCaution"]) == 1
        and "`id` is a column of both" in seen["ambiguousCaution"][0]
        and "trips.id" in seen["ambiguousCaution"][0],
        f"{seen['ambiguousCaution']!r}",
    )
    check(
        "and qualifying it — which is what the caution advises — silences it",
        seen["qualified"]["refusal"] == 0 and seen["qualifiedCaution"] == 0,
        f"{seen['qualifiedCaution']!r}",
    )

    # A computed group key on a join lands after *both* tables. If it landed
    # after the left one it would be `zones.id` — which is a real column, so
    # this comes back as a plausible table of numbers rather than an error.
    # The 24 hours are what says it did not.
    join_hours = [int(r.split("\t")[0]) for r in seen["joinHour"]["rows"]]
    check(
        "a computed column works on a join, keyed past both tables",
        seen["joinHour"]["refusal"] == 0
        and sorted(join_hours) == list(range(24))
        # Lowercased before comparing, as the ClickHouse check above does:
        # the grid uppercases headers in CSS and `innerText` reports what is
        # rendered, not what the binding produced.
        and seen["joinHour"]["headers"][0].lower() == "hour(pickup_time)",
        f"{seen['joinHour']['headers']}, {len(join_hours)} keys: {join_hours[:6]}",
    )
    check(
        "and the join narrowed it to Manhattan rather than the whole sample",
        0 < sum(int(r.split("\t")[1]) for r in seen["joinHour"]["rows"]) < 100_000,
        f"{sum(int(r.split(chr(9))[1]) for r in seen['joinHour']['rows'])} trips",
    )

    # The key and the aggregate both read `trips`. Before every joined
    # position resolved in the joined row this was not expressible at all, so
    # the assertion that matters most is simply that it ran.
    same_side = seen["joinSameSide"]
    fares = [float(r.split("\t")[1]) for r in same_side["rows"]]
    check(
        "the hour and the average fare can come from the same table",
        same_side["refusal"] == 0
        and len(fares) == 24
        # New York fares: a few dollars to a few tens. A wrong column here —
        # `duration` in seconds, or `pickup_zone` — lands outside this by an
        # order of magnitude, which is the point of checking the range rather
        # than only the shape.
        and all(3.0 < f < 120.0 for f in fares),
        f"{same_side['headers']}, {len(fares)} hours: {fares[:4]}",
    )

    # One table twice, under two aliases.
    aliased = seen["aliasedChain"]
    ends = [r.split("\t")[0] for r in aliased["rows"]]
    check(
        "one table can be read twice under two aliases",
        aliased["refusal"] == 0
        and 2 <= len(ends) <= 8
        # Names, not ids, and more than one of them: if both aliases had
        # resolved to the same input, every Manhattan pickup would have ended
        # in Manhattan and this would be a single group.
        and all(not e.strip().isdigit() for e in ends)
        and "Manhattan" in ends,
        f"{len(ends)} groups: {ends[:6]}",
    )
    check(
        "an aliased group key keeps the qualifier that tells the ends apart",
        # `borough` alone names both ends and neither, which is the one case
        # where echoing the reader's spelling means keeping the qualifier.
        #
        # Lowercased before comparing: the grid uppercases its headers in CSS
        # and `allInnerTexts()` returns what is *rendered*, so this read
        # `DROPOFF.BOROUGH` and failed against a correct header. Asserting on
        # rendered text means asserting on the stylesheet too.
        [h.lower() for h in aliased["headers"][:1]] == ["dropoff.borough"],
        f"{aliased['headers']}",
    )
    check(
        "the aliased chain plans as three steps",
        seen["aliasedChainPlan"].count("step") == 2
        and seen["aliasedChainPlan"].startswith("Chain"),
        seen["aliasedChainPlan"].replace("\n", " | ")[:160],
    )
    check(
        "the same table twice under one name is refused, and names the way out",
        "is read twice under one name" in seen["aliasCollisionWhy"],
        seen["aliasCollisionWhy"][:160],
    )

    # The subquery example, end to end in the browser.
    subquery = seen["subquery"]
    check(
        "a subquery answers rather than refusing",
        subquery["refusal"] == 0 and len(subquery["rows"]) == 1,
        f"{subquery['refusal']} refusals, {len(subquery['rows'])} rows",
    )
    # 848 of the 100,000 sampled trips start in Brooklyn. Asserted as a band
    # rather than the number, because the sample is a file somebody may
    # replace — but a band well away from 0 and from 100,000, because those
    # are the two answers a broken candidate list gives.
    trips = int(subquery["rows"][0].split("\t")[0].replace(",", ""))
    check(
        "the candidate list reached the outer query",
        0 < trips < 20000,
        f"{trips} trips, expected a few hundred",
    )
    spec = seen["subquerySpec"]
    check(
        "the spec shows the subquery beside the candidates it produced",
        '"subquery"' in spec and '"values"' in spec and '"zones"' in spec,
        spec.replace("\n", " ")[:200],
    )

    # A bare right-table column as the group key.
    boroughs = [r.split("\t")[0] for r in seen["joinRightKey"]["rows"]]
    check(
        "a group key may be a bare column of the right table",
        seen["joinRightKey"]["refusal"] == 0
        and 2 <= len(boroughs) <= 8
        # Names, not zone ids: an unshifted ordinal would have grouped
        # `trips.pickup_zone` and given 260 numbers.
        and all(not b.strip().isdigit() for b in boroughs),
        f"{len(boroughs)} groups: {boroughs[:6]}",
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
