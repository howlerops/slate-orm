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

const EXAMPLES = [
  ["Everything, first 20", "SELECT * FROM books LIMIT 20"],
  ["Filter on the indexed column", "SELECT * FROM books WHERE author_id = 2"],
  [
    "The same query, index-only",
    "SELECT author_id FROM books WHERE author_id = 2",
  ],
  [
    "Two conditions",
    "SELECT * FROM books WHERE author_id = 1 AND year > 1970",
  ],
  [
    "A pattern, sorted",
    "SELECT * FROM books WHERE title LIKE 'The %' ORDER BY year DESC LIMIT 10",
  ],
  [
    "Join the two tables",
    "SELECT * FROM authors JOIN books ON authors.id = books.author_id\n  WHERE country = 'US' LIMIT 20",
  ],
  [
    "Count books per country",
    "SELECT count(*), max(year) FROM authors JOIN books ON authors.id = books.author_id\n  GROUP BY country",
  ],
  [
    "Write, and watch the index",
    "INSERT INTO books VALUES (9001, 2, 'A New Book', 2024);\nSELECT author_id FROM books WHERE author_id = 2",
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
  const started = performance.now();
  const results = JSON.parse(state.playground.sql(text));
  const took = performance.now() - started;

  logAll(results, took);

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
  status(summarise(results, took));
}

function summarise(results, took) {
  const last = state.shown;
  const writes = results.filter((r) => r.kind === "write").length;
  const parts = [];
  if (last && last.kind !== "write") {
    parts.push(`${last.returned} row${last.returned === 1 ? "" : "s"}`);
  }
  if (writes) parts.push(`${writes} write${writes === 1 ? "" : "s"}`);
  parts.push(`${took.toFixed(1)} ms`);
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
  const body = result.rows
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

function renderSpec(result) {
  $("spec").textContent = JSON.stringify(result.spec, null, 2);
}

function logAll(results, took) {
  const box = $("log");
  for (const result of results) {
    const entry = document.createElement("div");
    entry.className = `entry${result.error ? " bad" : ""}`;
    const outcome = result.error
      ? result.error.message
      : result.kind === "write"
        ? result.message
        : `${result.returned} row${result.returned === 1 ? "" : "s"}`;
    entry.innerHTML =
      `<pre>${escape(result.sql)}</pre><span>${escape(outcome)}</span>`;
    box.prepend(entry);
  }
  const last = box.firstChild;
  if (last) last.dataset.took = `${took.toFixed(1)} ms`;
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

// --- boot -----------------------------------------------------------------

async function boot() {
  try {
    const module = await import("./slate_wasm.js");
    await module.default();
    state.playground = new module.Playground();
    state.schema = JSON.parse(state.playground.schema());

    describeSchema();
    describeExamples();

    const rows = state.schema
      .map((t) => t.name)
      .join(" + ");
    $("engine").textContent = `kernel in wasm · ${rows}`;

    $("run").addEventListener("click", run);
    $("reset").addEventListener("click", () => {
      state.playground.reset();
      // Deliberately *not* followed by `run()`. The editor may hold the
      // INSERT the reader just ran, and re-running it puts the row straight
      // back — a Reset button that does not reset. The browser check caught
      // exactly that: five rows where four were expected.
      state.shown = null;
      $("grid").innerHTML =
        '<div class="empty">the fixture is back — run a query</div>';
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
