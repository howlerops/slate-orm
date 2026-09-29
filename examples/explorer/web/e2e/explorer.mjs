/**
 * Drive the explorer in a real browser, against a real stack.
 *
 * The unit tests in `test/` cover the contract layer with nothing running. This
 * covers the thing they cannot: that the panels wire that layer to the DOM, and
 * that switching SDK really does change only the base URL — the demo's central
 * claim, and one no amount of unit testing can support, because it is a claim
 * about three processes agreeing.
 *
 * Deliberately plain `playwright` and not `@playwright/test`: this needs a
 * stack that `run.sh` starts, so the process that owns the lifecycle is a shell
 * script, and a test runner underneath it would own nothing except its own
 * config file.
 *
 * Run it through `./run.sh --e2e`, which starts everything on free ports, or
 * against a stack you have up:
 *
 *     node e2e/explorer.mjs http://127.0.0.1:7440
 */
import { chromium } from "playwright";

const base = process.argv[2] ?? "http://127.0.0.1:7440";
const SDKS = ["go", "node", "python"];

/** Where each adapter is, as the page was built to see it.
 *
 * Read from the same variables `run.sh` exported into vite, so this and the
 * page agree by construction rather than by both hard-coding 7431.
 */
const ORIGINS = {
  go: process.env["VITE_GO_URL"] ?? "http://127.0.0.1:7431",
  node: process.env["VITE_NODE_URL"] ?? "http://127.0.0.1:7432",
  python: process.env["VITE_PYTHON_URL"] ?? "http://127.0.0.1:7433",
};

let passed = 0;
const failures = [];

async function check(what, run) {
  try {
    await run();
    passed += 1;
    console.log(`ok    ${what}`);
  } catch (error) {
    failures.push(what);
    // At column zero, and `FAIL  <what>` exactly: this is the house format
    // every `scripts/test_*.py` prints and the one `scripts/mutate.py`'s
    // `python` dialect reads. Indented by two, it matched the summary line
    // and no failure line — so a mutation here scored as "unreadable" while
    // the run in fact said `29 passed, 1 failed` with the right check named.
    // A runner nobody can mutation-test is a runner taken on trust.
    console.log(`FAIL  ${what}`);
    console.log(`        ${String(error).split("\n")[0]}`);
  }
}

/** Click one of the segmented buttons under a label, or a tab. */
async function choose(page, group, option) {
  await page.locator(`.group:has(label:text-is("${group}")) button:text-is("${option}")`).click();
}

async function tab(page, name) {
  await page.locator(`.tabs button:text-is("${name}")`).click();
}

/** Wait until nothing on screen is mid-request.
 *
 * Every panel renders `asking…` while a query is in flight, so the absence of
 * all of them is the settled state — a far better signal than a timeout, and
 * the reason `Result` renders it at all. Counted rather than waited on as one
 * locator, because the rows tab shows two panels and a single `.spinner`
 * locator is a strict-mode violation the moment both are asking.
 */
async function settled(page) {
  await page.waitForFunction(() => document.querySelectorAll(".spinner").length === 0, null, {
    timeout: 20_000,
  });
}

/** Put the app in a known state.
 *
 * Called at the top of every check instead of restoring at the bottom. A check
 * that fails leaves the UI wherever it broke, and a suite that relies on
 * teardown then reports one failure as eight — which is exactly what the first
 * run of this file did.
 */
/**
 * Wait for the *chart* to say what we are about to read, not for the spinner.
 *
 * `settled` waits for no `.spinner`, and a query with data already cached
 * keeps it on screen while it refetches — so there is no spinner and the old
 * answer is still rendered. Switching the grouped panel's measure back to the
 * count hit exactly that: the assertion read the money order and reported that
 * the sort had not followed, on an unmutated tree.
 */
async function chartShowsPrices(page, showing) {
  await page.waitForFunction(
    (want) => {
      const rows = [...document.querySelectorAll(".chart .row")];
      if (rows.length === 0) return false;
      const priced = rows.some((row) => /\u2014 \d+\.\d{2}/.test(row.textContent ?? ""));
      return priced === want;
    },
    showing,
    { timeout: 20_000 },
  );
}

async function at(page, { sdk = "go", identity = "app", panel }) {
  await choose(page, "sdk", sdk);
  await choose(page, "identity", identity);
  await tab(page, panel);
  await settled(page);
}

const browser = await chromium.launch();
const page = await browser.newPage();

// A page that throws on load renders blank, and every assertion below would
// then fail with "element not found" — true, but useless. Surface the cause.
const crashes = [];
page.on("pageerror", (error) => crashes.push(String(error)));

console.log(`explorer at ${base}:`);

try {
  await page.goto(base, { waitUntil: "domcontentloaded" });
  await page.locator("h1:text-is('slate explorer')").waitFor({ timeout: 30_000 });

  await check("the head node reports itself the leader", async () => {
    await page.locator('.badge:has-text("head node")').waitFor({ timeout: 20_000 });
    const text = await page.locator('.badge:has-text("head node")').innerText();
    if (!text.includes("leader")) throw new Error(`badge says ${JSON.stringify(text)}`);
  });

  // --- the central claim -------------------------------------------------
  //
  // Same query, three client libraries, byte-identical tables. Compared as
  // rendered text, because that is what a reader of the demo compares.
  const tables = {};
  for (const sdk of SDKS) {
    await check(`the rows panel asks the ${sdk} adapter, and only it`, async () => {
      // Watching the requests, not just the rendered rows.
      //
      // All three adapters answer identically -- that is the demo -- so a
      // panel that ignored the switch and always used Go would render exactly
      // the right table for all three. Checking the rows alone is an assertion
      // the bug satisfies, which a mutation proved: pinning the panel's client
      // to "go" left every case passing.
      const asked = new Set();
      const listen = (request) => {
        if (request.url().includes("/api/")) asked.add(new URL(request.url()).origin);
      };
      page.on("request", listen);
      try {
        await at(page, { sdk, panel: "rows" });
        // The query is cached by tab and sdk, so nudge it into refetching
        // rather than asserting on a request that a previous check made.
        await page
          .locator('.panel:has(h2:text-is("Rows")) .field:has(span:text-is("limit")) input')
          .fill(String(20 + SDKS.indexOf(sdk)));
        await settled(page);
      } finally {
        page.off("request", listen);
      }

      const wanted = new URL(ORIGINS[sdk]).origin;
      if (!asked.has(wanted)) {
        throw new Error(`selected ${sdk} but asked ${[...asked].join(", ") || "nobody"}`);
      }
      const strays = [...asked].filter(
        (origin) => origin !== wanted && Object.values(ORIGINS).some((u) => new URL(u).origin === origin),
      );
      if (strays.length) throw new Error(`selected ${sdk} but also asked ${strays.join(", ")}`);

      const body = await page.locator(".panel:has(h2:text-is('Rows')) tbody").innerText();
      if (!body.includes("The Dispossessed")) {
        throw new Error(`no seeded row in the table: ${body.slice(0, 200)}`);
      }
      tables[sdk] = body;
    });
  }

  await check("and the three of them render the same rows", () => {
    const distinct = new Set(Object.values(tables));
    if (distinct.size !== 1) {
      throw new Error(
        `${distinct.size} different tables:\n` +
          Object.entries(tables).map(([sdk, body]) => `${sdk}: ${body.slice(0, 160)}`).join("\n"),
      );
    }
  });

  // --- what the identity switch does -------------------------------------

  await check("the row policy takes books away from a reader", async () => {
    // The panel opens on `year >= 1960`, which is *the policy's own predicate*.
    // Under it the two identities legitimately see the same rows, and the first
    // version of this check read that agreement as the policy not applying.
    // Clearing the filter is what makes the difference observable at all.
    const rows = page.locator(".panel:has(h2:text-is('Rows'))");
    const seen = {};
    for (const identity of ["app", "reader"]) {
      await at(page, { identity, panel: "rows" });
      await rows.locator('.field:has(span:text-is("value")) input').fill("");
      await settled(page);
      seen[identity] = await rows.locator("tbody").innerText();
    }

    const count = (body) => body.trim().split("\n").length;
    if (!(count(seen.reader) < count(seen.app))) {
      throw new Error(`unfiltered, the app saw ${count(seen.app)} rows and a reader ${count(seen.reader)}`);
    }
    // `using = "year >= 1960"`, so the 1950s book the app can see must be gone.
    if (!/19[0-5]\d/.test(seen.app)) {
      throw new Error("the fixture has no pre-1960 book, so this proves nothing");
    }
    if (/19[0-5]\d/.test(seen.reader)) {
      throw new Error("the reader can see a book published before 1960");
    }
  });

  await check("a view narrows the rows, and the policy narrows them again", async () => {
    // The whole of `docs/views.md` §1, in a browser: a view is substituted
    // away before planning, so the *base* table's row policy is the one that
    // runs. A view carrying its own `TableId` — the design §1 refuses — would
    // have no policy at all and hand a reader every row the view admits, and
    // this is the check that would see it.
    const rows = page.locator(".panel:has(h2:text-is('Rows'))");
    const pick = (name) =>
      rows.locator('.field:has(span:text-is("table")) select').selectOption(name);
    const count = (body) => body.trim().split("\n").length;

    const seen = {};
    for (const [key, identity, table] of [
      ["books", "app", "books"],
      ["view", "app", "classics"],
      ["reader", "reader", "classics"],
    ]) {
      await at(page, { identity, panel: "rows" });
      await pick(table);
      await rows.locator('.field:has(span:text-is("value")) input').fill("");
      await settled(page);
      seen[key] = await rows.locator("tbody").innerText();
    }

    if (!(count(seen.view) < count(seen.books))) {
      throw new Error(
        `the view did not narrow: ${count(seen.books)} rows from books, ${count(seen.view)} through it`,
      );
    }
    if (!(count(seen.reader) < count(seen.view))) {
      throw new Error(
        `the row policy did not narrow the view: ${count(seen.view)} rows as app, ${count(seen.reader)} as reader`,
      );
    }
    // `classics` is `year < 1980` and `modern_only` is `year >= 1960`, so the
    // 1950s books are inside the view and outside the policy. Asserting the
    // decade rather than only the counts, because two arbitrary numbers
    // shrinking proves less than the right rows disappearing.
    if (!/19[0-5]\d/.test(seen.view)) {
      throw new Error("the view shows no pre-1960 book, so the next assertion proves nothing");
    }
    if (/19[0-5]\d/.test(seen.reader)) {
      throw new Error("a reader can see a pre-1960 book through the view");
    }
    if (/19[89]\d|199\d/.test(seen.view)) {
      throw new Error("the view shows a book from 1980 or later, so its own predicate did nothing");
    }

    // And the plan panel refuses, because `explain` is not a path that reads
    // through a view. Lowercased for the reason the reader-plan check below
    // gives: the stylesheet renders the kind in caps.
    const plan = page.locator(".panel:has(h2:text-is('The plan'))");
    const refusal = (await plan.innerText()).toLowerCase();
    if (!refusal.includes("is a view over")) {
      throw new Error(`the plan panel did not name the view: ${refusal.slice(0, 200)}`);
    }

    // Put the switcher back, so the checks after this one open on `books`
    // as they always have.
    await at(page, { identity: "app", panel: "rows" });
    await pick("books");
    await settled(page);
  });

  await check("a reader is refused a plan, and told it is the database refusing", async () => {
    await at(page, { identity: "reader", panel: "rows" });
    const plan = page.locator(".panel:has(h2:text-is('The plan'))");
    // Lowercased because the stylesheet renders the kind in caps; the assertion
    // is about the kind the server sent, not about text-transform.
    const kind = (await plan.locator(".error .kind").innerText()).trim().toLowerCase();
    if (kind !== "permission-denied") throw new Error(`plan says ${kind}`);
    const why = await plan.locator(".error .why").innerText();
    if (!why.includes("not the adapter")) throw new Error("the refusal does not say who refused");
  });

  await check("a stranger is refused rows outright, not shown an empty table", async () => {
    await at(page, { identity: "stranger", panel: "rows" });
    const rows = page.locator(".panel:has(h2:text-is('Rows'))");
    if ((await rows.locator(".error .kind").count()) === 0) {
      throw new Error("no refusal rendered; an empty table would teach the wrong thing");
    }
  });

  // --- the other panels --------------------------------------------------

  await check("a left join shows an unmatched side as absent", async () => {
    await at(page, { panel: "joins" });
    await page.locator('.panel:has(h2:text-is("Joins")) select').selectOption("left");
    await settled(page);
    const note = await page.locator('.panel:has(h2:text-is("Joins")) .note').innerText();
    if (!/[1-9]\d* with an unmatched side/.test(note)) {
      throw new Error(`a left join found no unmatched row: ${note}`);
    }
  });

  await check("an inner join has no unmatched side", async () => {
    await at(page, { panel: "joins" });
    await page.locator('.panel:has(h2:text-is("Joins")) select').selectOption("inner");
    await settled(page);
    const note = await page.locator('.panel:has(h2:text-is("Joins")) .note').innerText();
    if (!/\b0 with an unmatched side/.test(note)) {
      throw new Error(`an inner join reported an unmatched row: ${note}`);
    }
  });

  await check("the rows panel's operators reach the database", async () => {
    // The controls the panel offers, actually clicked. These were exercised
    // only as HTTP bodies by the conformance runner, which cannot tell whether
    // the *select* is wired to the request — a panel whose operator dropdown
    // did nothing would pass every one of those cases.
    await at(page, { panel: "rows" });
    const rows = page.locator(".panel:has(h2:text-is('Rows'))");
    const body = async () => {
      await settled(page);
      return rows.locator("tbody").innerText();
    };

    // `like` on the title, against a prefix only some books have.
    await rows.locator('.field:has(span:text-is("where")) select').selectOption({ label: "title" });
    await rows.locator('.field:has(span:text-is("op")) select').selectOption("like");
    await rows.locator('.field:has(span:text-is("value")) input').fill("The %");
    const liked = await body();
    if (!/\bThe /.test(liked)) throw new Error(`a prefix pattern matched nothing: ${liked.slice(0, 120)}`);

    // `ilike` with the wrong case must match what `like` did not.
    await rows.locator('.field:has(span:text-is("op")) select').selectOption("ilike");
    await rows.locator('.field:has(span:text-is("value")) input').fill("the %");
    const insensitive = await body();
    if (insensitive.trim() !== liked.trim()) {
      throw new Error("ilike with the wrong case did not match what like matched");
    }

    // And the control: `like` with the wrong case matches nothing, so the two
    // operators are genuinely different rather than both being ilike.
    await rows.locator('.field:has(span:text-is("op")) select').selectOption("like");
    await settled(page);
    const sensitive = await rows.innerText();
    if (!/no rows matched/.test(sensitive)) {
      throw new Error("case-sensitive like matched a differently-cased prefix");
    }
  });

  await check("the table switcher and the sort direction reach the database", async () => {
    await at(page, { panel: "rows" });
    const rows = page.locator(".panel:has(h2:text-is('Rows'))");

    await rows.locator('.field:has(span:text-is("table")) select').selectOption("authors");
    await rows.locator('.field:has(span:text-is("value")) input').fill("");
    await settled(page);
    // Lowercased because the stylesheet renders headers in caps. The
    // assertion is about which columns came back, not about text-transform —
    // the same trap that made an earlier check read `PERMISSION-DENIED` as a
    // wrong error kind.
    const headers = (await rows.locator("thead th").allInnerTexts()).map((h) =>
      h.toLowerCase(),
    );
    if (!headers.some((h) => h.startsWith("country"))) {
      throw new Error(`switching to authors kept the old columns: ${headers.join(", ")}`);
    }

    // Ascending, then descending, over the same column: the first rows must
    // reverse. A direction control that did nothing leaves them identical.
    const firstColumn = async () =>
      (await rows.locator("tbody tr td:first-child").allInnerTexts()).join(",");
    await rows.locator('.field:has(span:text-is("direction")) select').selectOption("asc");
    const ascending = await firstColumn();
    await rows.locator('.field:has(span:text-is("direction")) select').selectOption("desc");
    await settled(page);
    const descending = await firstColumn();
    if (ascending === descending) {
      throw new Error(`the direction control changed nothing: ${ascending}`);
    }
    if (ascending !== descending.split(",").reverse().join(",")) {
      throw new Error(`descending is not ascending reversed: ${ascending} vs ${descending}`);
    }
  });

  await check("the grouped join draws a bar per author", async () => {
    await at(page, { panel: "groups" });
    const bars = await page.locator(".chart .row").count();
    if (bars < 2) throw new Error(`${bars} bars`);

    // Geometry, not just count. The bars are divs whose width is a percentage
    // of the largest value, so a CSS change that collapsed every bar to zero —
    // or a chart that drew them all the same — would pass a count assertion
    // and show a reader nothing.
    const widths = await page
      .locator(".chart .row .fill")
      .evaluateAll((fills) => fills.map((f) => f.getBoundingClientRect().width));
    if (!widths.every((w) => w > 0)) throw new Error(`a bar has no width: ${widths}`);
    const values = (await page.locator(".chart .row .value").allInnerTexts()).map(Number);
    if (new Set(values).size > 1 && new Set(widths.map(Math.round)).size === 1) {
      throw new Error(`different counts drew identical bars: ${values} -> ${widths}`);
    }
    // The largest value's bar is the widest one.
    const widest = widths.indexOf(Math.max(...widths));
    if (values[widest] !== Math.max(...values)) {
      throw new Error(`the widest bar is not the largest count: ${values} -> ${widths}`);
    }
  });

  await check("a computed group key is a grouping a reader can pick", async () => {
    // The point of the dropdown's lower half: most of those keys are not
    // columns. An `upper()`, a `CASE`, a regular expression, a calendar field,
    // an hour in New York — each an expression the SDK sends and the kernel
    // evaluates, and none of them something the browser could bucket for
    // itself without fetching every row.
    //
    // Checked in the browser rather than only by the conformance runner
    // because "surfaced in the demo" means a reader can choose it and see an
    // answer, and a dropdown option that throws is worse than no option.
    await at(page, { panel: "groups" });
    const select = page.locator('.panel:has(h2:text-is("Grouped join")) select').first();
    const byAuthor = await page.locator(".chart .row .label").allInnerTexts();

    for (const [key, expected] of [
      // `shout` upper-cases the author's *name*, so the labels change and the
      // partition does not — same groups, different text.
      ["shout", (labels) => labels.length === byAuthor.length
        && labels.every((l) => l === l.toUpperCase())],
      // `era` splits on 1970, so exactly two groups.
      ["era", (labels) => labels.length === 2 && labels.every((l) => /19\d\d/.test(l))],
      // A calendar field: every label is a four-digit year.
      ["releasedYear", (labels) => labels.every((l) => /^\d{4}$/.test(l))],
      // And an hour of the local day, so every label is 0-23.
      ["releasedHourNY", (labels) => labels.every((l) => Number(l) >= 0 && Number(l) <= 23)],
    ]) {
      await select.selectOption(key);
      await settled(page);
      const labels = await page.locator(".chart .row .label").allInnerTexts();
      if (!labels.length) throw new Error(`grouping by ${key} drew nothing`);
      if (!expected(labels)) {
        throw new Error(`grouping by ${key} drew ${labels.join(", ")}`);
      }
      // The panel shows the spec it sent, and a computed key is visible there
      // as a computed key rather than as a column — which is what says the
      // database did the work.
      const refused = await page
        .locator('.panel:has(h2:text-is("Grouped join")) .error')
        .count();
      if (refused) throw new Error(`grouping by ${key} was refused`);
    }
    await select.selectOption("author");
    await settled(page);
  });

  await check("HAVING removes groups, and removes the smallest ones", async () => {
    await at(page, { panel: "groups" });
    const before = await page.locator(".chart .row .value").allInnerTexts();
    await page.locator('.panel:has(h2:text-is("Grouped join")) input[type=number]').fill("3");
    await settled(page);
    const after = await page.locator(".chart .row .value").allInnerTexts();
    if (!(after.length < before.length)) {
      throw new Error(`having >= 3 left ${after.length} of ${before.length} groups`);
    }
    if (after.some((value) => Number(value) < 3)) {
      throw new Error(`a group below the threshold survived: ${after.join(", ")}`);
    }
    await page.locator('.panel:has(h2:text-is("Grouped join")) input[type=number]').fill("0");
  });

  await check("the grouped panel shows the plan of the grouped read", async () => {
    await at(page, { panel: "groups" });
    const panel = page.locator('.panel:has(h2:text-is("Grouped join"))');
    const badges = await panel.locator(".badges .badge").allInnerTexts();
    if (badges.length < 2) {
      throw new Error(`a two-input grouped join showed ${badges.length} input plans`);
    }
    // `decodes` is the point of the panel: an empty list on every input means
    // the plan came back without it, which is how the field being dropped
    // anywhere between the kernel and the browser would look.
    if (!badges.some((text) => /decodes \[\d/.test(text))) {
      throw new Error(`no input reports a decoded column: ${badges.join(" | ")}`);
    }
    const plan = await panel.locator("pre").innerText();
    if (!plan.startsWith("Group by [")) {
      throw new Error(`the plan does not say what is being grouped: ${plan.slice(0, 80)}`);
    }
  });

  await check("a committed write becomes visible, a rolled-back one does not", async () => {
    await at(page, { panel: "transactions" });
    const panel = page.locator('.panel:has(h2:text-is("Transactions"))');

    // The interesting assertion is the *pair*. `visible inside` true for both
    // endings is what says the transaction can read its own write; `visible
    // afterwards` differing is what says the ending mattered. Either badge
    // alone is satisfied by a panel that ignores the select.
    const seen = {};
    for (const ending of ["commit", "rollback"]) {
      await panel.locator("select").selectOption(ending);
      await panel.locator('button:text-is("run it")').click();
      await settled(page);
      const badges = await panel.locator(".badge").allInnerTexts();
      const inside = badges.find((text) => text.includes("inside"));
      const after = badges.find((text) => text.includes("afterwards"));
      if (!inside || !after) throw new Error(`no outcome rendered: ${badges.join(" | ")}`);
      if (!inside.includes("true")) throw new Error(`${ending}: the transaction cannot read its own write`);
      seen[ending] = after.includes("true");
    }
    if (!seen.commit) throw new Error("a committed write was not visible afterwards");
    if (seen.rollback) throw new Error("a rolled-back write was visible afterwards");
  });

  // The three panels N5 added, each asserting the thing its panel exists to
  // show rather than that it rendered. A panel that renders and teaches the
  // wrong thing is the failure worth catching here.
  // Full-text, asserted on the thing the panel exists to show. Both paths
  // return the same books by construction, so a check on the rows alone would
  // pass against an adapter that ignored the path — the failure the panel's
  // own prose names. The access line is the assertion.
  await check("the two full-text paths return the same books by different plans", async () => {
    await at(page, { panel: "search" });
    const panel = page.locator('.panel:has(h2:text-is("Full-text search"))');

    await panel.locator('[data-test="search-path"]').selectOption("index");
    await settled(page);
    const byIndex = await panel.locator('[data-test="search-summary"]').innerText();
    const indexRows = await panel.locator("tbody tr").count();

    await panel.locator('[data-test="search-path"]').selectOption("scan");
    await settled(page);
    const byScan = await panel.locator('[data-test="search-summary"]').innerText();
    const scanRows = await panel.locator("tbody tr").count();

    if (indexRows === 0) throw new Error("the seeded titles matched nothing by index");
    if (indexRows !== scanRows) {
      throw new Error(`index returned ${indexRows} rows and scan ${scanRows}`);
    }
    if (byIndex === byScan) {
      throw new Error(`both paths reported the same plan, so neither is chosen: ${byIndex}`);
    }
  });

  await check("a search matching nothing is an empty answer, not a failure", async () => {
    await at(page, { panel: "search" });
    const panel = page.locator('.panel:has(h2:text-is("Full-text search"))');
    await panel.locator('[data-test="search-text"]').fill("zzzznotitle");
    await settled(page);
    const summary = await panel.locator('[data-test="search-summary"]').innerText();
    if (!summary.includes("matched 0")) throw new Error(`matched: ${summary}`);
    // An empty answer still carries a plan, which is how it differs from a
    // refusal — the distinction the panel is making.
    if (summary.includes("not visible")) throw new Error(`the app persona lost its plan: ${summary}`);
  });

  // Optimistic concurrency. The unguarded case is the control: without it,
  // "refused" is satisfied by a panel that refuses everything.
  await check("a conditional update lands, and is refused once somebody else writes", async () => {
    await at(page, { panel: "conditional" });
    const panel = page.locator('.panel:has(h2:text-is("Conditional writes"))');

    await panel.locator('[data-test="conditional-run"]').click();
    await settled(page);
    const quiet = await panel.locator('[data-test="conditional-summary"]').innerText();
    if (!quiet.includes("applied")) throw new Error(`unguarded: ${quiet}`);
    if (!quiet.includes("12.50")) throw new Error(`the new price did not land: ${quiet}`);

    await panel.locator('[data-test="conditional-meddle"]').selectOption("moved");
    await panel.locator('[data-test="conditional-run"]').click();
    await settled(page);
    const stale = await panel.locator('[data-test="conditional-summary"]').innerText();
    if (!stale.includes("refused")) throw new Error(`stale: ${stale}`);
    // And the other writer's value is what is stored, not the refused one.
    if (!stale.includes("11.00")) throw new Error(`the other writer's price is not there: ${stale}`);
  });

  await check("a conditional delete refuses a row that is gone, where a plain one would not", async () => {
    await at(page, { panel: "conditional" });
    const panel = page.locator('.panel:has(h2:text-is("Conditional writes"))');
    await panel.locator('[data-test="conditional-write"]').selectOption("delete");
    await panel.locator('[data-test="conditional-meddle"]').selectOption("gone");
    await panel.locator('[data-test="conditional-run"]').click();
    await settled(page);
    const summary = await panel.locator('[data-test="conditional-summary"]').innerText();
    if (!summary.includes("refused")) throw new Error(`gone: ${summary}`);
    // The point of the case: a plain delete would say affected 0 and look the
    // same as a successful one over an already-absent key.
    if (!summary.includes("affected 0")) throw new Error(`affected: ${summary}`);
    if (!summary.includes("row gone")) throw new Error(`the row should be absent: ${summary}`);
  });

  await check("a predicate write hands back the rows it destroyed", async () => {
    await at(page, { panel: "writes" });
    const panel = page.locator('.panel:has(h2:text-is("Predicate writes"))');
    await panel.locator('[data-test="predicate-run"]').click();
    await settled(page);
    const summary = await panel.locator('[data-test="predicate-summary"]').innerText();
    // Two of the four seeded books are 2002 or later.
    if (!summary.includes("affected 2")) throw new Error(`affected: ${summary}`);
    if (!summary.includes("rows returned 2")) throw new Error(`returned: ${summary}`);
    if (!summary.includes("left 2")) throw new Error(`left: ${summary}`);
    const rows = await panel.locator("tbody tr").count();
    if (rows !== 2) throw new Error(`the panel rendered ${rows} returned rows, not 2`);
  });

  await check("without `returning` there is a count and no rows", async () => {
    await at(page, { panel: "writes" });
    const panel = page.locator('.panel:has(h2:text-is("Predicate writes"))');
    await panel.locator('select').nth(1).selectOption("no");
    await panel.locator('[data-test="predicate-run"]').click();
    await settled(page);
    const summary = await panel.locator('[data-test="predicate-summary"]').innerText();
    if (!summary.includes("affected 2")) throw new Error(`affected: ${summary}`);
    if (!summary.includes("rows returned 0")) throw new Error(`returned: ${summary}`);
    const rows = await panel.locator("tbody tr").count();
    if (rows !== 0) throw new Error(`rows were rendered without \`returning\`: ${rows}`);
  });

  // The undo window, asserted on the thing it exists to show rather than on
  // having rendered. The four badges are four separate claims and the third and
  // fourth are the ones a broken restore would get wrong.
  await check("a retired row goes invisible, and comes back the same row", async () => {
    await at(page, { panel: "soft delete" });
    const panel = page.locator('.panel:has(h2:text-is("Soft delete, and undo"))');
    await panel.locator('[data-test="restore-run"]').click();
    await settled(page);
    const summary = await panel.locator('[data-test="restore-summary"]').innerText();
    if (!summary.includes("retired first yes")) throw new Error(`premise: ${summary}`);
    // The whole point of a soft delete: an ordinary read stops returning it.
    if (!summary.includes("an ordinary read saw 0")) throw new Error(`hidden: ${summary}`);
    // And the whole point of the undo: it comes back to an ordinary read.
    if (!summary.includes("after the undo it sees 1")) throw new Error(`back: ${summary}`);
    if (!summary.includes("still stamped 0")) throw new Error(`stamp: ${summary}`);
    // Carried through, which is what says it is the same row and not a fresh
    // one written at the key it left free — the difference the badges cannot
    // show, because a replacement reports the same id and the same "not
    // stamped".
    // By its own `data-test` rather than `.note` last(): the panel grew a
    // second note below this one and `last()` silently started reading it,
    // which is a locator that keeps passing while it checks the wrong thing.
    const note = await panel.locator('[data-test="restore-carried"]').innerText();
    if (!note.includes("pending")) throw new Error(`status not carried through: ${note}`);
  });

  await check("the retired-rows flag is a privilege, not a filter", async () => {
    await at(page, { panel: "soft delete" });
    const panel = page.locator('.panel:has(h2:text-is("Soft delete, and undo"))');

    // As `app`: the flag goes out and the read answers. Nothing is retired in
    // the fixture, so the *count* is the same either way — which is exactly
    // why the interesting half is the refusal below and not this number.
    const plain = await panel.locator('[data-test="include-deleted-count"]').innerText();
    if (!plain.includes("retired ones hidden")) throw new Error(`plain: ${plain}`);
    await panel.locator('[data-test="include-deleted"]').selectOption("yes");
    await settled(page);
    const asked = await panel.locator('[data-test="include-deleted-count"]').innerText();
    if (!asked.includes("retired ones included")) throw new Error(`asked: ${asked}`);

    // As `reader`, which holds `read` and not `read_deleted`: the same request
    // is refused. "There are none" and "you may not see them" are different
    // answers, and this is the one that an ordinary filter could never give.
    await at(page, { identity: "reader", panel: "soft delete" });
    await panel.locator('[data-test="include-deleted"]').selectOption("yes");
    await settled(page);
    // Scoped to this read's own answer. The retire-and-undo above is refused
    // for a reader too, so `panel.innerText()` carries a refusal either way —
    // and a mutation that never set the flag survived that assertion.
    const answer = panel.locator('[data-test="include-deleted-answer"]');
    const refused = await answer.innerText();
    if (!/permission-denied/i.test(refused)) {
      throw new Error(`a reader was not refused the flag: ${refused.slice(0, 400)}`);
    }
    // And without the flag the same reader is served, which is what makes the
    // refusal above about the flag rather than about the table.
    await panel.locator('[data-test="include-deleted"]').selectOption("no");
    await settled(page);
    const served = await answer.innerText();
    if (/permission-denied/i.test(served)) {
      throw new Error(`a reader was refused an ordinary read: ${served.slice(0, 400)}`);
    }
    await at(page, { panel: "soft delete" });
  });

  await check("the chart can be drawn in money instead of rows", async () => {
    await at(page, { panel: "groups" });
    const panel = page.locator('.panel:has(h2:text-is("Grouped join"))');

    // The panel's own controls survive a tab switch — `at` resets the sdk, the
    // identity and the tab, and nothing inside a panel — so the checks above
    // leave the grouping and the HAVING wherever they put them.
    await panel.locator("select").first().selectOption("author");
    await panel.locator('input[type="number"]').fill("0");
    await settled(page);

    await panel.locator('[data-test="groups-measure"]').selectOption("total");
    await settled(page);
    await chartShowsPrices(page, true);

    // The bars are labelled with the sum rendered against the column's scale,
    // so a label carrying a point is the decimal reaching the page — and the
    // note says which aggregate the order is about.
    const note = await panel.locator('[data-test="groups-note"]').innerText();
    if (!note.includes("SUM(books.price)")) throw new Error(`note: ${note}`);
    const labels = await panel.locator(".chart .row").allInnerTexts();
    const totals = labels.map((one) => {
      const found = /\u2014 (\d+\.\d{2})\b/.exec(one);
      return found ? Number(found[1]) : null;
    });
    if (totals.some((one) => one === null) || totals.length < 2) {
      throw new Error(`not every group is labelled with a price: ${labels.join(" | ")}`);
    }

    // And the order is the sum's, checked as a *property* rather than against
    // a leader somebody read off the fixture once. It was written that way and
    // it was wrong: the write panels above leave extra books on one author, so
    // by the time this runs the count order and the money order agree at the
    // head and differ further down. Descending by money means descending by
    // money, whatever the data underneath has become.
    for (let n = 1; n < totals.length; n += 1) {
      if (totals[n] > totals[n - 1]) {
        throw new Error(
          `the bars are not in descending order of money, so the sort did not ` +
            `follow the measure: ${totals.join(", ")}`,
        );
      }
    }
  });

  await check("the two atomicities leave different numbers of rows", async () => {
    await at(page, { panel: "batches" });
    const panel = page.locator('.panel:has(h2:text-is("Batches"))');
    await panel.locator('[data-test="batch-run"]').click();
    // Wait for a badge in *each* side, not for `settled` and not for the first
    // badge anywhere. Two queries start on this click: `settled` only waits
    // for spinners that have already mounted, so it can return between the
    // click and the first render; and the first badge is whichever query won,
    // which leaves the other still empty. Both of those failed here in turn.
    // Waiting for exactly what each assertion reads has no such window.
    await panel.locator('[data-test="batch-independent"] .badge').first().waitFor();
    await panel.locator('[data-test="batch-all-or-nothing"] .badge').first().waitFor();
    await settled(page);
    // Named, not positional. `.split > div` picked something that was not the
    // side it looked like: Solid's `For` puts markers between children, so an
    // index is not the reading order. A name cannot be off by one.
    const independent = await panel.locator('[data-test="batch-independent"]').innerText();
    const atomic = await panel.locator('[data-test="batch-all-or-nothing"]').innerText();
    // The counts are read off the adapter rather than assumed: independent
    // leaves three rows, all-or-nothing leaves one. Guessing them produced a
    // failing test that looked like a broken panel.
    //
    // Independent: the call succeeds, the collision is one outcome, and the
    // two writes that worked are still there.
    if (!independent.includes("succeeded")) throw new Error(`independent: ${independent}`);
    if (!independent.includes("rows left 3")) throw new Error(`independent left: ${independent}`);
    // All-or-nothing: the call fails and nothing landed. If these two ever
    // report the same count, the panel is showing a difference that is not
    // there — which is the only thing it exists to show.
    if (atomic.includes("succeeded")) throw new Error(`atomic should have failed: ${atomic}`);
    if (!atomic.includes("rows left 1")) throw new Error(`atomic left: ${atomic}`);
  });

  await check("a path keeps its middle level, and dropping it is a choice", async () => {
    await at(page, { panel: "relationships" });
    const panel = page.locator('.panel:has(h2:text-is("Relationships"))');
    // Trees: three books, and book 12 has no editions — the level is there
    // and empty, which is the case a hand-written regroup gets wrong.
    const trees = await panel.locator('[data-test="path-trees"]').innerText();
    if (!trees.includes("book 10")) throw new Error(`no book 10 in the tree: ${trees}`);
    if (!trees.includes("no editions")) {
      throw new Error(`book 12's empty level is not shown: ${trees}`);
    }
    // Through: the same request, the far rows only. The middle books must be
    // gone, or "through" is showing what "trees" shows.
    await panel.locator('.seg button:text-is("through")').click();
    await settled(page);
    const through = await panel.locator('[data-test="path-through"]').innerText();
    if (!through.includes("edition")) throw new Error(`no editions through: ${through}`);
    if (through.includes("Predicate")) throw new Error(`a book row leaked into through: ${through}`);
  });

  await check("the agreement panel says the three are identical", async () => {
    await at(page, { panel: "agreement" });
    const badge = await page.locator('.panel:has(h2:text-is("Do the three agree?")) .badge').first().innerText();
    if (!badge.includes("identical")) throw new Error(`the panel says ${JSON.stringify(badge)}`);
  });
} finally {
  await browser.close();
}

// A page error is a *named* failure, not a bare exit code.
//
// It used to be neither: the summary said `30 passed, 0 failed` and the
// process exited 1, which reads to anything parsing the output as a suite that
// passed and to the shell as one that did not. `scripts/mutate.py` calls that
// its second lie — "nothing ran" and "nothing failed" are the same empty
// output — and it refused to score a run here for exactly this reason, on a
// crash that was intermittent and nothing to do with the mutation.
//
// The same fix `examples/explorer/conformance/test_conformance.py` took an
// hour earlier, for the same reason, in the other browser-free runner.
for (const crash of crashes) {
  failures.push("the page threw");
  console.log("FAIL  the page threw");
  console.log(`        ${crash.split("\n")[0]}`);
}

console.log();
console.log(`${passed} passed, ${failures.length} failed`);
process.exit(failures.length ? 1 : 0);
