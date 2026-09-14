// The playground: the record layer's kernel, in this tab.
//
// `slate_wasm.js` and `slate_wasm_bg.wasm` are built from `crates/slate-wasm`
// and are *not* committed — CI builds them and the Pages deploy carries them,
// so a stale binary cannot drift from the kernel it claims to be. The panel
// stays hidden until the module loads, so a reader who arrives before the
// build lands (or with wasm disabled) sees the prose and not a dead widget.

const panel = document.querySelector("[data-playground]");
if (panel) {
  const el = (name) => panel.querySelector(`[data-play="${name}"]`);

  const state = { schema: [], playground: null, columns: new Set() };

  const show = (message) => {
    el("status").textContent = message;
  };

  /** Rebuild the column and sort pickers for the selected table. */
  function describeTable() {
    const table = state.schema.find((t) => t.name === el("table").value);
    if (!table) return;

    for (const select of [el("column"), el("sort")]) {
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

    return {
      table: el("table").value,
      // An empty value means no filter at all, rather than a filter against
      // the empty string — which would quietly return nothing and look like
      // a bug in the database.
      ...(value === "" ? {} : { filter: { column: Number(el("column").value), op: el("op").value, value } }),
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

  const escape = (text) =>
    String(text).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

  (async () => {
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
        describeTable();
        run();
      });
      for (const name of ["column", "op", "value", "sort", "direction", "limit"]) {
        const control = el(name);
        control.addEventListener(control.tagName === "SELECT" ? "change" : "input", run);
      }
      el("all").addEventListener("click", () => {
        describeTable();
        run();
      });

      panel.hidden = false;
      run();
    } catch (error) {
      // Left hidden rather than showing a broken panel: the prose above still
      // explains what the playground would have shown.
      console.error("the playground did not load", error);
    }
  })();
}
