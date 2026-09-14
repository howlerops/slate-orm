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
    console.log(`  ok    ${what}`);
  } catch (error) {
    failures.push(what);
    console.log(`  FAIL  ${what}`);
    console.log(`          ${String(error).split("\n")[0]}`);
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

  await check("the agreement panel says the three are identical", async () => {
    await at(page, { panel: "agreement" });
    const badge = await page.locator('.panel:has(h2:text-is("Do the three agree?")) .badge').first().innerText();
    if (!badge.includes("identical")) throw new Error(`the panel says ${JSON.stringify(badge)}`);
  });
} finally {
  await browser.close();
}

if (crashes.length) {
  console.log("\nthe page threw:");
  for (const crash of crashes) console.log(`  ${crash}`);
}

console.log();
console.log(`${passed} passed, ${failures.length} failed`);
process.exit(failures.length || crashes.length ? 1 : 0);
