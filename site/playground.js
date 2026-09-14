// The playground: the record layer's kernel, in this tab.
//
// `slate_wasm.js` and `slate_wasm_bg.wasm` are built from `crates/slate-wasm`
// and are *not* committed — CI builds them and the Pages deploy carries them,
// so a stale binary cannot drift from the kernel it claims to be. The panel
// stays hidden until the module loads, so a reader who arrives before the
// build lands (or with wasm disabled) sees the prose and not a dead widget.

const panel = document.querySelector("[data-playground]");
if (panel) {
  // Scoped to the panel for its own controls; the start block is a *sibling*
  // of the panel (it has to outlive the panel being hidden), so it is looked
  // up from the document. Querying it through `panel` finds nothing, which is
  // how the load button ended up wired to undefined the first time.
  const el = (name) => panel.querySelector(`[data-play="${name}"]`);
  const outer = (name) => document.querySelector(`[data-play="${name}"]`);

  const state = { schema: [], playground: null, columns: new Set() };

  const show = (message) => {
    el("status").textContent = message;
  };

  /** Rebuild the column and sort pickers for the selected table. */
  function describeTable() {
    const table = state.schema.find((t) => t.name === el("table").value);
    if (!table) return;

    for (const select of [el("column"), el("column2"), el("sort")]) {
      select.innerHTML = "";
      if (select === el("sort")) {
        select.append(new Option("(none)", ""));
      }
      for (const column of table.columns) {
        // Indexed columns are marked, because which one you pick is the
        // whole point of the exercise.
        const mark = table.indexed.includes(column.ordinal) ? " ·idx" : "";
        select.append(new Option(`${column.name}${mark}`, String(column.ordinal)));
      }
    }

    // Default the filter to an indexed column when the table has one. The
    // picker used to open on the first column, which for both fixture tables
    // is the primary key, so the first plan a reader ever saw was a one-row
    // Point Get: true, and the least interesting thing the planner does.
    //
    // It also disagreed with the prose directly above the panel, which tells
    // the reader to filter on the indexed column and then narrow the
    // projection. Opening there means step one is already done: the plan on
    // screen is a table scan over 4,824 rows, and unchecking three boxes
    // turns it index-only. That is the demonstration; `id = 1` was not.
    const interesting = table.columns.find((c) => table.indexed.includes(c.ordinal));
    if (interesting) el("column").value = String(interesting.ordinal);

    // Every column selected by default: that is the shape that *cannot* be
    // index-only, so the reader starts from the uninteresting plan and
    // narrows towards the interesting one.
    state.columns = new Set(table.columns.map((c) => c.ordinal));
    const boxes = el("columns");
    boxes.innerHTML = "";
    for (const column of table.columns) {
      const label = document.createElement("label");
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = true;
      box.addEventListener("change", () => {
        if (box.checked) state.columns.add(column.ordinal);
        else state.columns.delete(column.ordinal);
        run();
      });
      label.append(box, document.createTextNode(column.name));
      boxes.append(label);
    }
  }

  function spec() {
    const table = state.schema.find((t) => t.name === el("table").value);
    const all = table ? table.columns.length : 0;
    const value = el("value").value;
    const limit = Number.parseInt(el("limit").value, 10);
    const sort = el("sort").value;

    // An empty value means no condition at all, rather than one against the
    // empty string — which would quietly return nothing and look like a bug in
    // the database. The second condition additionally needs an operator.
    const filters = [];
    if (value !== "") {
      filters.push({ column: Number(el("column").value), op: el("op").value, value });
    }
    if (el("op2").value !== "" && el("value2").value !== "") {
      filters.push({
        column: Number(el("column2").value),
        op: el("op2").value,
        value: el("value2").value,
      });
    }

    return {
      table: el("table").value,
      ...(filters.length ? { filters } : {}),
      ...(sort === "" ? {} : { sort: [{ column: Number(sort), descending: el("direction").value === "desc" }] }),
      ...(Number.isFinite(limit) && limit > 0 ? { limit } : {}),
      // Sending every column is the same as sending none, and the kernel
      // reads "no projection" as "all of them" — so send [] and let it say so.
      columns: state.columns.size === all ? [] : [...state.columns].sort((a, b) => a - b),
    };
  }

  function run() {
    if (!state.playground) return;
    const answer = JSON.parse(state.playground.run(JSON.stringify(spec())));

    if (answer.error) {
      el("badges").innerHTML = "";
      el("plan").textContent = "";
      el("rows").innerHTML = "";
      show(answer.error);
      return;
    }

    const plan = answer.plan;
    const badges = [
      [plan.access, plan.indexOnly ? "good" : null],
      [`decodes [${plan.decodes.join(", ")}]`, null],
      [`cost ${plan.estimatedCost.toFixed(3)}`, null],
      [`est. ${Math.round(plan.estimatedRows)} rows`, null],
      plan.sorts ? ["sorts", "warn"] : null,
    ].filter(Boolean);

    el("badges").innerHTML = "";
    for (const [text, tone] of badges) {
      const span = document.createElement("span");
      span.className = "badge";
      if (tone) span.dataset.tone = tone;
      span.textContent = text;
      el("badges").append(span);
    }
    el("plan").textContent = plan.display;

    const table = state.schema.find((t) => t.name === el("table").value);
    const shown = spec().columns;
    const headers = shown.length
      ? shown.map((o) => table.columns[o].name)
      : table.columns.map((c) => c.name);

    // Rows always come back whole; a projection changes what the *plan*
    // decodes, not the shape of the answer. Showing only the projected
    // columns would hide that, so the table shows everything and the
    // `decodes` badge above is where the narrowing is visible.
    const head = `<tr>${table.columns.map((c) => `<th>${c.name}</th>`).join("")}</tr>`;
    const body = answer.rows
      .map((row) => `<tr>${row.map((v) => `<td>${escape(v)}</td>`).join("")}</tr>`)
      .join("");
    el("rows").innerHTML = `<table><thead>${head}</thead><tbody>${body}</tbody></table>`;

    const narrowed = headers.length < table.columns.length ? `, reading ${headers.join(", ")}` : "";
    show(`${answer.returned} row${answer.returned === 1 ? "" : "s"}${narrowed}`);
  }

  /** One text input per column, for the write row. */
  function describeWriteFields() {
    const table = state.schema.find((t) => t.name === el("table").value);
    const fields = el("fields");
    fields.innerHTML = "";
    if (!table) return;
    for (const column of table.columns) {
      const label = document.createElement("label");
      const input = document.createElement("input");
      input.size = column.type === "string" ? 14 : 6;
      input.placeholder = column.name;
      input.dataset.column = String(column.ordinal);
      label.append(input);
      fields.append(label);
    }
  }

  const writeValues = () =>
    [...el("fields").querySelectorAll("input")].map((input) => input.value);

  function wrote(text) {
    el("wrote").textContent = text;
  }

  function handleWrite(result) {
    const answer = JSON.parse(result);
    wrote(answer.error ?? answer.ok);
    // Re-run whatever the reader was looking at, so an insert that lands in
    // the current filter's range shows up immediately — which is the point of
    // writing from here at all.
    run();
  }

  /** The join panel, which asks the binding a different question entirely. */
  function runJoin() {
    if (!state.playground) return;
    const country = el("country").value;
    const groupBy = el("groupby").value;
    const agg = el("agg").value;

    const spec = {
      ...(country === "" ? {} : { authors: [{ column: 2, op: "eq", value: country }] }),
      ...(groupBy === "" ? {} : { groupBy: Number(groupBy) }),
      aggregates: [{ kind: "count" }, ...(agg === "" ? [] : [{ kind: agg, column: 3 }])],
      limit: 50,
    };

    const answer = JSON.parse(state.playground.join(JSON.stringify(spec)));
    if (answer.error) {
      el("joinbadges").innerHTML = "";
      el("joinplan").textContent = "";
      el("joinrows").innerHTML = "";
      el("joinstatus").textContent = answer.error;
      return;
    }

    el("joinbadges").innerHTML = "";
    for (const input of answer.inputs) {
      const span = document.createElement("span");
      span.className = "badge";
      if (input.indexOnly) span.dataset.tone = "good";
      span.textContent =
        `${input.table} ${input.access} decodes [${input.decodes.join(", ")}]` +
        (input.algorithm ? ` · ${input.algorithm}` : "");
      el("joinbadges").append(span);
    }
    el("joinplan").textContent = answer.display;

    const grouped = answer.groups.length > 0;
    const body = (grouped ? answer.groups : answer.rows)
      .map((row) => `<tr>${row.map((v) => `<td>${escape(v)}</td>`).join("")}</tr>`)
      .join("");
    el("joinrows").innerHTML = `<table><tbody>${body}</tbody></table>`;
    el("joinstatus").textContent = grouped
      ? `${answer.returned} group${answer.returned === 1 ? "" : "s"}`
      : `${answer.returned} joined rows`;
  }

  const escape = (text) =>
    String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

  /**
   * Fetch and start the kernel. Called once, by a click or by the panel
   * scrolling into view — never on page load.
   *
   * The bundle is around 600 KB gzipped, which is a real cost to put on every
   * reader of a landing page, most of whom will not touch the panel. Deferring
   * it means a visitor who scrolls past pays nothing, and one who wants it
   * waits a moment and knows why they are waiting.
   */
  let starting = null;
  const start = () => {
    if (starting) return starting;
    outer("load").disabled = true;
    outer("load").textContent = "Loading…";
    starting = boot();
    return starting;
  };

  async function boot() {
    try {
      const module = await import("./slate_wasm.js");
      await module.default();
      state.playground = new module.Playground();
      state.schema = JSON.parse(state.playground.schema());

      const tables = el("table");
      for (const table of state.schema) tables.append(new Option(table.name, table.name));
      tables.value = "books";

      describeTable();
      tables.addEventListener("change", () => {
        el("value").value = "";
        el("value2").value = "";
        el("op2").value = "";
        describeTable();
        describeWriteFields();
        run();
      });
      for (const name of [
        "column", "op", "value", "column2", "op2", "value2", "sort", "direction", "limit",
      ]) {
        const control = el(name);
        control.addEventListener(control.tagName === "SELECT" ? "change" : "input", run);
      }
      el("all").addEventListener("click", () => {
        describeTable();
        run();
      });

      outer("start").hidden = true;
      describeWriteFields();
      el("insert").addEventListener("click", () =>
        handleWrite(state.playground.insert(el("table").value, JSON.stringify(writeValues()))),
      );
      el("remove").addEventListener("click", () => {
        // Delete takes the primary key alone, which for both fixture tables is
        // the first column.
        handleWrite(state.playground.delete(el("table").value, JSON.stringify([writeValues()[0]])));
      });
      el("reset").addEventListener("click", () => {
        state.playground.reset();
        wrote("the fixture is back");
        run();
      });
      el("runjoin").addEventListener("click", runJoin);

      panel.hidden = false;
      run();
    } catch (error) {
      // The panel stays hidden and the button says what happened, rather than
      // a broken widget or a silent nothing. The prose above still explains
      // what the playground would have shown.
      outer("load").disabled = false;
      outer("load").textContent = "Loading failed — try again";
      console.error("the playground did not load", error);
      starting = null;
    }
  }

  outer("load").addEventListener("click", start);

  // And automatically once the panel is actually on screen, so a reader who
  // scrolls to it does not have to ask twice. `rootMargin` starts the fetch
  // slightly before it arrives; readers who never scroll here never trigger
  // it, which is the entire point.
  if ("IntersectionObserver" in window) {
    const watcher = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          watcher.disconnect();
          start();
        }
      },
      { rootMargin: "200px" },
    );
    watcher.observe(outer("start"));
  }
}
