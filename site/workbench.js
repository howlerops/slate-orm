// The workbench: slate's kernel, driven from a query editor.
//
// `slate_wasm.js` and `slate_wasm_bg.wasm` are built from `crates/slate-wasm`
// and are *not* committed — CI builds them and the Pages deploy carries them,
// so a stale binary cannot drift from the kernel it claims to be.
//
// The module is fetched on load rather than lazily. That reverses the choice
// made when this was a panel two thirds of the way down a prose page, where a
// reader who never scrolled to it should not pay for it. It is now the page:
// a visitor who waits for a shell and then has to ask for the database has
// been charged the latency without being given the thing. The cost is real and
// stated in `site/README.md` rather than hidden.

const $ = (name) => document.querySelector(`[data-app="${name}"]`);

const state = {
  playground: null,
  schema: [],
  /// The decompressed trip file, so Reset can re-seed without a refetch.
  tripBytes: null,
  /// The last statement that produced rows, so the tabs describe one thing.
  shown: null,
};

/// Text into HTML. Everything below builds markup from strings, and every
/// string here is either a fixture value or something the reader typed.
const escape = (text) =>
  String(text).replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );

//: Where the trip file lives, relative to the page.
const TRIPS = "data/trips.bin.gz";

const EXAMPLES = [
  ["Busiest pickup zones", "SELECT pickup_zone, count(*) FROM trips\n  GROUP BY pickup_zone ORDER BY count(*) DESC LIMIT 10"],
  [
    "…with their names",
    "SELECT * FROM trips JOIN zones ON trips.pickup_zone = zones.id\n  WHERE trips.pickup_zone = 132 LIMIT 20",
  ],
  [
    "How people pay",
    "SELECT payment, count(*), avg(total), max(tip) FROM trips\n  GROUP BY payment ORDER BY count(*) DESC",
  ],
  [
    "count(*) is not count(column)",
    "-- 4.7% of real trips have no passenger count.\nSELECT payment, count(*), count(passengers) FROM trips\n  GROUP BY payment",
  ],
  [
    "Trips by hour of day",
    "-- pickup_time is an integer of seconds; hour() is a computed column,\n" +
      "-- evaluated per row and grouped like any other. Look at the Spec tab.\n" +
      "SELECT hour(pickup_time), count(*), avg(distance) FROM trips\n" +
      "  GROUP BY hour(pickup_time)\n" +
      "  ORDER BY hour(pickup_time)",
  ],
  [
    "…and by day of week",
    "-- 0 is Sunday. January 2024 opened on a Monday, so there are five of\n" +
      "-- those in the sample and four Sundays.\n" +
      "SELECT day_of_week(pickup_time), count(*), avg(total) FROM trips\n" +
      "  GROUP BY day_of_week(pickup_time)\n" +
      "  ORDER BY count(*) DESC",
  ],
  [
    "ClickHouse's taxi Q4, as written",
    "-- ClickHouse's New York taxi tutorial ends with this. It used to be\n" +
      "-- unwritable here -- it keys on toYear(pickup_datetime), and there was\n" +
      "-- no year to key on -- so the site substituted a column and said so.\n" +
      "-- This is the query, not an adaptation of it.\n" +
      "SELECT passengers, year(pickup_time), round(distance), count(*)\n" +
      "  FROM trips\n" +
      "  GROUP BY passengers, year(pickup_time), round(distance)\n" +
      "  ORDER BY year(pickup_time), count(*) DESC\n" +
      "  LIMIT 20",
  ],
  [
    "…in another timezone",
    "-- Every time function reads UTC unless told otherwise. This sample is\n" +
      "-- already New York wall clock -- the trough at 04:00 and the peak at\n" +
      "-- 18:00 are the evidence -- so the shift here is what you would write\n" +
      "-- for a column that really did hold UTC instants.\n" +
      "--\n" +
      "-- Fixed offsets only. 'America/New_York' needs a timezone database to\n" +
      "-- know about daylight saving, and guessing would be wrong for a third\n" +
      "-- of the year, so it is refused rather than approximated. Try it.\n" +
      "SELECT hour(pickup_time, '-05:00'), count(*) FROM trips\n" +
      "  GROUP BY hour(pickup_time, '-05:00')\n" +
      "  ORDER BY hour(pickup_time, '-05:00')",
  ],
  [
    "Manhattan's day, hour by hour",
    "-- A computed column on a join, which used to be refused outright. It is\n" +
      "-- evaluated over the joined row and lands after *both* tables'\n" +
      "-- columns -- which is why one declared on a single side is refused\n" +
      "-- instead: it would sit exactly where the other table's first column\n" +
      "-- belongs, and be read as that column with no error anywhere.\n" +
      "--\n" +
      "-- The group key comes from the left table and the aggregates from the\n" +
      "-- right, so `borough` narrows the zones scan and the hour keys the\n" +
      "-- trips. count(*) needs no column and works either way.\n" +
      "SELECT hour(pickup_time), count(*)\n" +
      "  FROM trips JOIN zones ON trips.pickup_zone = zones.id\n" +
      "  WHERE borough = 'Manhattan'\n" +
      "  GROUP BY hour(pickup_time)",
  ],
  [
    "Busy zones only (HAVING)",
    "-- WHERE filters rows before grouping; HAVING filters the groups after.\n" +
      "-- Swap this for `WHERE count(*) > 3000` and the parser will tell you\n" +
      "-- why that cannot mean anything.\n" +
      "SELECT pickup_zone, count(*), avg(duration) FROM trips\n" +
      "  GROUP BY pickup_zone\n" +
      "  HAVING count(*) > 300 AND avg(duration) > 900\n" +
      "  ORDER BY count(*) DESC",
  ],
  ["Long, expensive rides", "SELECT * FROM trips WHERE distance > 20 AND total > 100 LIMIT 50"],
  [
    "One zone, every column",
    "SELECT * FROM trips WHERE pickup_zone = 132 LIMIT 50",
  ],
  [
    "…the same rows, index-only",
    "SELECT pickup_zone FROM trips WHERE pickup_zone = 132 LIMIT 50",
  ],
  ["Write a row, watch the index", "INSERT INTO trips VALUES (999001, 132, 1, 1704067200, 600, 2, 5.5, 25.0, 3.0, 31.0, 'cash');\nSELECT pickup_zone FROM trips WHERE pickup_zone = 132"],
  ["The small fixture, for contrast", "SELECT * FROM books WHERE author_id = 2"],
  [
    "Kitchen sink",
    "-- Everything the grammar has, in two statements.\n" +
      "--\n" +
      "-- One: a grouped join. The conditions are split by side, so each scan\n" +
      "-- is narrowed before the hash join ever sees it -- look at the Plan tab.\n" +
      "SELECT count(*), min(fare), max(total), avg(distance)\n" +
      "  FROM zones JOIN trips ON zones.id = trips.pickup_zone\n" +
      "  WHERE borough = 'Manhattan' AND total > 50 AND payment = 'credit card'\n" +
      "  GROUP BY borough;\n" +
      "\n" +
      "-- Two: everything else at once. Six conditions over three types\n" +
      "-- (u64, f64, text), a pattern and a regular expression, two group keys,\n" +
      "-- seven aggregates, a HAVING over two of them, ordered by an aggregate\n" +
      "-- and then by a key, paged.\n" +
      "--\n" +
      "-- It is two statements rather than one because ORDER BY and a second\n" +
      "-- group key are not available on the join path. The grammar says so\n" +
      "-- rather than quietly ignoring them.\n" +
      "SELECT pickup_zone, passengers,\n" +
      "       count(*), count(passengers), count(distinct dropoff_zone),\n" +
      "       min(fare), max(tip), sum(total), avg(distance)\n" +
      "  FROM trips\n" +
      "  WHERE pickup_zone > 100 AND pickup_zone < 200\n" +
      "    AND total > 20 AND distance < 10\n" +
      "    AND payment LIKE 'c%' AND payment ~ '^credit'\n" +
      "  GROUP BY pickup_zone, passengers\n" +
      "  HAVING count(*) > 5 AND avg(distance) < 5\n" +
      "  ORDER BY count(*) DESC, pickup_zone\n" +
      "  LIMIT 20 OFFSET 5",
  ],
];

// --- the schema tree ------------------------------------------------------

function describeSchema() {
  const box = $("tables");
  box.innerHTML = "";
  for (const table of state.schema) {
    const block = document.createElement("div");
    block.className = "tree-table";

    const head = document.createElement("button");
    head.type = "button";
    head.className = "tree-name";
    head.dataset.table = table.name;
    // "4 cols", not "4": a bare number next to a table name reads as a row
    // count, and this fixture has 4,824 books.
    head.innerHTML =
      `${escape(table.name)} <span class="count">${table.columns.length} cols</span>`;
    // Clicking a table is the "show me this" gesture every database client
    // has. It runs immediately rather than only filling the editor: a click
    // that silently edits a text box reads as broken.
    head.addEventListener("click", () => {
      $("editor").value = `SELECT * FROM ${table.name} LIMIT 20`;
      run();
    });
    block.append(head);

    const list = document.createElement("ul");
    list.className = "tree-columns";
    for (const column of table.columns) {
      const item = document.createElement("li");
      const indexed = table.indexed.includes(column.ordinal);
      const key = table.primaryKey.includes(column.ordinal);
      item.innerHTML =
        `<button type="button" class="tree-column" data-column="${escape(column.name)}">` +
        `${escape(column.name)}</button>` +
        `<span class="type">${escape(column.type)}</span>` +
        (key ? '<span class="mark key" title="primary key">pk</span>' : "") +
        (indexed ? '<span class="mark idx" title="secondary index">idx</span>' : "");
      // Insert at the caret: the reader is usually mid-statement.
      item.querySelector("button").addEventListener("click", () => insert(column.name));
      list.append(item);
    }
    block.append(list);
    box.append(block);
  }
}

function insert(text) {
  const editor = $("editor");
  const { selectionStart: from, selectionEnd: to, value } = editor;
  editor.value = value.slice(0, from) + text + value.slice(to);
  editor.selectionStart = editor.selectionEnd = from + text.length;
  editor.focus();
}

function describeExamples() {
  const list = $("examples");
  list.innerHTML = "";
  for (const [label, sql] of EXAMPLES) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = label;
    button.addEventListener("click", () => {
      $("editor").value = sql;
      run();
    });
    item.append(button);
    list.append(item);
  }
}

// --- running --------------------------------------------------------------

function run() {
  if (!state.playground) return;
  const text = $("editor").value;
  // Three numbers, because they are three different things and only one of
  // them is the database:
  //
  //   kernel  — planning and executing, measured inside the binding
  //   wasm    — that, plus serialising the answer to JSON
  //   parse   — JavaScript turning that JSON back into objects
  //
  // For the examples on this page the last two round to nothing. For
  // `SELECT * FROM trips` they are most of the wall clock: 166 ms of
  // serialising and 90 ms of parsing around a scan. Showing only the outer
  // number would say this database is slow at `SELECT *` when what is slow
  // is the JSON.
  const started = performance.now();
  const results = JSON.parse(state.playground.sql(text));
  const took = performance.now() - started;
  const kernel = results.reduce((a, r) => a + (r.kernelMs ?? 0), 0);
  const marshalling = Math.max(0, took - kernel);

  logAll(results);

  const failed = results.find((r) => r.error);
  if (failed) {
    showError(failed, text);
    return;
  }
  // The last statement that produced a grid is what the tabs describe; a
  // buffer that ends in a write leaves the previous rows on screen rather
  // than blanking them, which is what every database client does.
  const shown = [...results].reverse().find((r) => r.kind !== "write") ?? results.at(-1);
  state.shown = shown;
  renderRows(shown);
  renderPlan(shown);
  renderSpec(shown);
  status(summarise(results, kernel, marshalling));
}

function summarise(results, kernel, marshalling) {
  const last = state.shown;
  const writes = results.filter((r) => r.kind === "write").length;
  const parts = [];
  if (last && last.kind !== "write") {
    parts.push(`${last.returned} row${last.returned === 1 ? "" : "s"}`);
  }
  if (writes) parts.push(`${writes} write${writes === 1 ? "" : "s"}`);
  parts.push(`${kernel.toFixed(1)} ms`);
  // Only mentioned when it is worth mentioning. Appending "+ 0.0 ms JSON" to
  // every query would be noise; hiding it on the one query where it is a
  // third of the time would be the lie this exists to avoid.
  if (marshalling >= 1 && marshalling > kernel * 0.1) {
    parts.push(`+ ${marshalling.toFixed(0)} ms JSON`);
  }
  return parts.join(" · ");
}

function status(text, tone) {
  const box = $("status");
  box.textContent = text;
  box.dataset.tone = tone ?? "";
}

function showError(result, buffer) {
  const at = result.error.at ?? 0;
  const before = buffer.slice(0, at);
  const line = before.split("\n").length;
  status(result.error.message, "bad");
  $("grid").innerHTML =
    `<div class="refusal"><b>line ${line}</b> — ${escape(result.error.message)}</div>`;
  select("results");
  // Put the caret where the parser stopped. Being told *where* is most of the
  // value of an error message in an editor.
  const editor = $("editor");
  editor.focus();
  editor.selectionStart = editor.selectionEnd = at;
}

/// How many rows the grid will build into the DOM.
///
/// The query is not capped — `SELECT * FROM trips` really does return 100,000
/// rows and the status bar says so. The *table* is, because building 100,000
/// `<tr>` takes **29.5 seconds** and freezes the tab, measured. A page that
/// locks up for half a minute after a 56 ms query is not showing anybody how
/// fast the query was.
const RENDER_CAP = 1000;

function renderRows(result) {
  const grid = $("grid");
  if (!result || (!result.columns.length && !result.rows.length)) {
    grid.innerHTML = `<div class="empty">${escape(result?.message || "done")}</div>`;
    return;
  }
  // Which ordinals the plan actually decoded. A column outside this list came
  // back as `null` because it was never read — late materialization — and
  // printing that as the word "null" would be a lie about the data. Only a
  // single-table read has a plan to ask; joins render everything.
  const decoded = result.plan ? new Set(result.plan.decodes) : null;

  const head = result.columns.map((c) => `<th>${escape(c)}</th>`).join("");
  const shown = result.rows.slice(0, RENDER_CAP);
  const body = shown
    .map((row) => {
      const cells = row
        .map((value, i) => {
          if (value === "null" && decoded && !decoded.has(i)) {
            return '<td class="unread" title="not read: this plan does not decode this column">·</td>';
          }
          return `<td>${escape(value)}</td>`;
        })
        .join("");
      return `<tr>${cells}</tr>`;
    })
    .join("");
  grid.innerHTML = `<table><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
  if (!result.rows.length) {
    grid.innerHTML += '<div class="empty">no rows</div>';
  } else if (result.rows.length > shown.length) {
    // Said plainly, because a silently truncated table is a table that lies
    // about what the query returned.
    grid.innerHTML +=
      `<div class="empty">showing the first ${shown.length.toLocaleString()} of ` +
      `${result.rows.length.toLocaleString()} rows — the query returned them all, ` +
      `the table is capped so the page stays usable</div>`;
  }
}

function renderPlan(result) {
  const badges = $("badges");
  badges.innerHTML = "";
  const add = (text, tone) => {
    const span = document.createElement("span");
    span.className = "badge";
    if (tone) span.dataset.tone = tone;
    span.textContent = text;
    badges.append(span);
  };

  if (result.plan) {
    const plan = result.plan;
    add(plan.access, plan.indexOnly ? "good" : null);
    add(`decodes [${plan.decodes.join(", ")}]`);
    add(`cost ${plan.estimatedCost.toFixed(3)}`);
    add(`est. ${Math.round(plan.estimatedRows)} rows`);
    add(`actual ${result.returned}`);
    if (plan.sorts) add("sorts", "warn");
    $("plan").textContent = plan.display;
    $("plannote").textContent = plan.indexOnly
      ? "Index-only: this plan answers from the index and never reads a row."
      : "An index that still has to fetch rows can cost more than scanning; the cost above is why the planner chose this.";
    return;
  }

  for (const input of result.inputs ?? []) {
    add(`${input.table}: ${input.access}`, input.indexOnly ? "good" : null);
    if (input.algorithm) add(input.algorithm);
  }
  $("plan").textContent = result.message || "";
  $("plannote").textContent = result.inputs?.length
    ? "One line per input, in the order the join reads them."
    : "";
}

/// The keyspace, as a folder tree over the real keys.
///
/// Two levels: `rows/` and `index/`, then one folder per table or index.
/// Opening a folder pages through the keys themselves, in the order the store
/// holds them — which is the point. A viewer that listed prefixes and stopped
/// would be a table of contents; the keys are the thing.
function renderKeyspace() {
  const box = $("keyspace");
  if (!state.playground) return;
  const groups = JSON.parse(state.playground.keyspace());
  const totals = groups.reduce(
    (a, g) => ({ keys: a.keys + g.keys, bytes: a.bytes + g.keyBytes + g.valueBytes }),
    { keys: 0, bytes: 0 },
  );
  $("keytotal").textContent = `${totals.keys.toLocaleString()} keys · ${bytes(totals.bytes)}`;

  const spaces = [
    ["rows", "one key per row, ordered by primary key"],
    ["index", "one key per row per index, ordered by the indexed value"],
  ];

  box.innerHTML = "";
  for (const [space, caption] of spaces) {
    const mine = groups.filter((g) => g.space === space);
    if (!mine.length) continue;

    const folder = document.createElement("details");
    folder.className = "fs-folder";
    folder.open = true;
    folder.innerHTML =
      `<summary><span class="fs-name">${escape(space)}/</span>` +
      `<span class="fs-note">${escape(caption)}</span></summary>`;

    for (const group of mine) {
      const leaf = document.createElement("details");
      leaf.className = "fs-leaf";
      const name = group.path.slice(space.length + 1);
      leaf.innerHTML =
        `<summary><span class="fs-name">${escape(name)}/</span>` +
        `<span class="fs-count">${group.keys.toLocaleString()} keys</span>` +
        `<span class="fs-bytes">${bytes(group.keyBytes)} keys + ${bytes(group.valueBytes)} values</span>` +
        `</summary><div class="fs-keys"></div>`;
      // Keys are fetched when a folder is opened, not before: each call walks
      // the whole store, and opening six folders eagerly would walk it six
      // times before the reader had asked for anything.
      leaf.addEventListener("toggle", () => {
        if (leaf.open) showKeys(leaf.querySelector(".fs-keys"), group);
      });
      folder.append(leaf);
    }
    box.append(folder);
  }
}

/// How many keys one page of a folder shows.
const PAGE = 25;

function showKeys(box, group, offset = 0) {
  if (offset === 0 && box.dataset.loaded) return;
  const keys = JSON.parse(state.playground.keys(group.path, offset, PAGE));
  if (offset === 0) box.innerHTML = "";
  box.dataset.loaded = "1";

  for (const key of keys) {
    const row = document.createElement("div");
    row.className = "fs-key";
    row.innerHTML =
      `<code>${escape(key.key)}</code><span>${escape(key.decoded)}</span>` +
      `<span class="fs-vb">${key.valueBytes} B</span>`;
    box.append(row);
  }

  const shown = offset + keys.length;
  box.querySelector(".fs-more")?.remove();
  if (shown < group.keys) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "fs-more";
    button.textContent =
      `${shown.toLocaleString()} of ${group.keys.toLocaleString()} — show ${PAGE} more`;
    button.addEventListener("click", () => showKeys(box, group, shown));
    box.append(button);
  } else {
    const end = document.createElement("div");
    end.className = "fs-more done";
    end.textContent = `all ${group.keys.toLocaleString()} keys`;
    box.append(end);
  }
}

/// The real bucket listing, captured by `examples/bucket_layout.rs`.
///
/// Static, and labelled as static on the page. There is no SlateDB in the
/// browser — the store here is a `BTreeMap` — so the alternative to shipping a
/// real listing is describing one in prose, and a described bucket is the kind
/// of thing that quietly stops being true.
///
/// It is no longer *unchecked* static, which is the part that used to be a
/// caveat. The file carries a provenance block naming the schema and the row
/// counts it was taken against, and `bucket_provenance.rs` fails when either
/// stops matching what the site ships. That does not prove the byte counts are
/// current — nothing cheap can — but it catches the drift that made the
/// listing misleading rather than merely old.
/// What each kind of object in the bucket is for, longest-lived first.
///
/// Ordered rather than a map because `find` takes the first match and the
/// fragments must not overlap ambiguously; kept in one place because
/// `bucket_provenance.rs` asserts the same set covers every object in the
/// committed listing.
const KINDS = [
  ["/compacted/", "the rows and the index entries, compacted"],
  ["/wal/", "write-ahead log"],
  ["/manifest/", "which SSTs are live"],
  ["/compactions/", "compaction bookkeeping"],
  ["/gc/", "how far the collector has reclaimed"],
];

async function renderBucket() {
  const box = $("bucket");
  if (box.dataset.loaded) return;
  try {
    const listing = await (await fetch("data/bucket.json")).json();
    const entries = listing.objects;
    const source = listing.provenance;
    const total = entries.reduce((a, e) => a + e.bytes, 0);
    const rows = entries
      .map((e) => {
        // The SST holds the data; everything else is bookkeeping. Marking it
        // is the difference between a file list and an explanation.
        //
        // Every fragment here is also in `bucket_provenance.rs`, which fails
        // if the listing grows an object none of them match. That test is why
        // `/gc/` is on this list: it used to fall through to the final label
        // and be shown as "compaction bookkeeping", which it is not — it is
        // the collector's own boundary marker, and the collector is the thing
        // that decides whether the WAL copy of every row is still there.
        const kind = KINDS.find(([fragment]) => e.path.includes(fragment));
        return (
          `<div class="bk-row"><code>${escape(e.path)}</code>` +
          `<span class="bk-kind">${escape(kind ? kind[1] : "unrecognised")}</span>` +
          `<span class="bk-bytes">${bytes(e.bytes)}</span></div>`
        );
      })
      .join("");
    $("buckettotal").textContent =
      `${entries.length} objects · ${bytes(total)} · ` +
      `${source.trips.toLocaleString()} trips and ${source.zones} zones`;
    box.innerHTML = rows;
    box.dataset.loaded = "1";
  } catch (error) {
    box.innerHTML = `<div class="empty">the bucket listing did not load: ${escape(error)}</div>`;
  }
}

const bytes = (n) =>
  n >= 1 << 20
    ? `${(n / (1 << 20)).toFixed(1)} MB`
    : n >= 1024
      ? `${(n / 1024).toFixed(1)} KB`
      : `${n} B`;

function renderSpec(result) {
  $("spec").textContent = JSON.stringify(result.spec, null, 2);
}

function logAll(results) {
  const box = $("log");
  for (const result of results) {
    const entry = document.createElement("div");
    entry.className = `entry${result.error ? " bad" : ""}`;
    const outcome = result.error
      ? result.error.message
      : result.kind === "write"
        ? result.message
        : `${result.returned} row${result.returned === 1 ? "" : "s"}`;
    // Per statement, because the status bar sums a whole buffer and the
    // interesting comparison is between the statements in it — an INSERT
    // against the SELECT that reads it back, say.
    const ms = result.error ? "" : ` · ${(result.kernelMs ?? 0).toFixed(2)} ms`;
    entry.innerHTML =
      `<pre>${escape(result.sql)}</pre><span>${escape(outcome)}${escape(ms)}</span>`;
    box.prepend(entry);
  }
}

// --- tabs -----------------------------------------------------------------

function select(name) {
  for (const tab of document.querySelectorAll("[data-tab]")) {
    tab.setAttribute("aria-selected", String(tab.dataset.tab === name));
  }
  for (const pane of document.querySelectorAll("[data-pane]")) {
    pane.hidden = pane.dataset.pane !== name;
  }
}

/// Switch between the query console and the storage browser.
///
/// The keyspace is rebuilt on every visit rather than cached: the reader may
/// have inserted a row since last time, and a storage view that does not move
/// when the data moves is the one thing this view must not be.
function mode(name) {
  for (const button of document.querySelectorAll("[data-mode]")) {
    button.setAttribute("aria-selected", String(button.dataset.mode === name));
  }
  document.querySelector(".console").hidden = name !== "query";
  document.querySelector(".storage").hidden = name !== "storage";
  document.querySelector(".tree").hidden = name !== "query";
  if (name === "storage") {
    renderKeyspace();
    void renderBucket();
  }
}

// --- boot -----------------------------------------------------------------

/// Fetch and decompress the trip file, and seed it.
///
/// `DecompressionStream` rather than a gzip library: it is in the platform,
/// it streams, and shipping an inflate implementation to decompress a file
/// the browser already knows how to decompress would be the only dependency
/// on this page. Browsers without it (Safari before 16.4) get the workbench
/// with an empty `trips`, and the status bar says so rather than failing
/// silently.
async function loadTrips() {
  if (typeof DecompressionStream !== "function") {
    return { error: "this browser cannot decompress the trip file" };
  }
  const response = await fetch(TRIPS);
  if (!response.ok) return { error: `the trip file returned ${response.status}` };
  const stream = response.body.pipeThrough(new DecompressionStream("gzip"));
  const bytes = new Uint8Array(await new Response(stream).arrayBuffer());
  // Kept so Reset can put them back. `reset()` rebuilds the store from
  // nothing, which drops the trips with everything else — and a Reset button
  // that leaves the main table empty until a page reload is worse than no
  // Reset button. 2.4 MB held in the tab is the price.
  state.tripBytes = bytes;
  return JSON.parse(state.playground.load_trips(bytes));
}

async function boot() {
  try {
    const module = await import("./slate_wasm.js");
    await module.default();
    state.playground = new module.Playground();
    state.schema = JSON.parse(state.playground.schema());

    describeSchema();
    describeExamples();

    // Started here, awaited at the end of boot: the schema tree, the examples
    // and the editor are all usable while it is in flight.
    status("loading 100,000 real taxi trips…");
    const trips = loadTrips()
      .then((outcome) => {
        if (outcome.error) {
          $("engine").textContent = "kernel in wasm · trips unavailable";
          status(outcome.error, "bad");
          return;
        }
        state.trips = outcome.ok;
        $("engine").textContent =
          `kernel in wasm · ${outcome.ok.toLocaleString()} trips, 265 zones`;
      })
      .catch((error) => {
        status(`the trip file did not load: ${error}`, "bad");
      });

    $("engine").textContent = "kernel in wasm";

    $("run").addEventListener("click", run);
    $("reset").addEventListener("click", () => {
      state.playground.reset();
      if (state.tripBytes) {
        const outcome = JSON.parse(state.playground.load_trips(state.tripBytes));
        if (outcome.error) status(outcome.error, "bad");
      }
      // Deliberately *not* followed by `run()`. The editor may hold the
      // INSERT the reader just ran, and re-running it puts the row straight
      // back — a Reset button that does not reset. The browser check caught
      // exactly that: five rows where four were expected.
      state.shown = null;
      $("grid").innerHTML =
        '<div class="empty">the data is back — run a query</div>';
      $("badges").innerHTML = "";
      $("plan").textContent = "";
      $("plannote").textContent = "";
      $("spec").textContent = "";
      select("results");
      status("reset");
    });
    $("editor").addEventListener("keydown", (event) => {
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
        event.preventDefault();
        run();
      }
    });
    for (const tab of document.querySelectorAll("[data-tab]")) {
      tab.addEventListener("click", () => select(tab.dataset.tab));
    }
    for (const button of document.querySelectorAll("[data-mode]")) {
      button.addEventListener("click", () => mode(button.dataset.mode));
    }

    // The data file is fetched in parallel with the module above, so the
    // first paint does not wait for 1.2 MB. Until it lands `trips` is an
    // empty table that queries correctly and returns nothing, which is a
    // better failure than a page that will not answer at all.
    await trips;
    run();
  } catch (error) {
    // The shell stays, with the reason in it. A workbench that fails to a
    // blank page is indistinguishable from one that was never deployed.
    $("engine").textContent = "unavailable";
    status(`the database did not start: ${error}`, "bad");
    $("grid").innerHTML =
      '<div class="refusal">This needs WebAssembly. Everything here is described in ' +
      '<a href="docs.html">the docs</a>, which need nothing but a browser.</div>';
  }
}

boot();
