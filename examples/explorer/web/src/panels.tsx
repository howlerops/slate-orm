/** The demo's five panels. Each is one thing the database does. */
import { createMemo, createSignal, For, Show, type JSX } from "solid-js";
import { createQuery } from "@tanstack/solid-query";

import {
  api,
  render,
  TABLES,
  type Persona,
  type QuerySpec,
  type Sdk,
} from "./api";
import { Bars, Result, ValueTable } from "./parts";

interface Context {
  sdk: () => Sdk;
  persona: () => Persona;
}

/** Rows, with a filter, a sort and a projection. */
export function Explore(props: Context): JSX.Element {
  const [table, setTable] = createSignal("books");
  const [column, setColumn] = createSignal(3);
  const [op, setOp] = createSignal("ge");
  const [text, setText] = createSignal("1960");
  const [sortColumn, setSortColumn] = createSignal(0);
  const [descending, setDescending] = createSignal(false);
  const [limit, setLimit] = createSignal(25);

  const columns = () => TABLES[table()] ?? [];

  /** The filter, typed the way the column is declared.
   *
   * A `u64` column and an `i64` column want different tags for the same
   * digits, and sending the wrong one asks a different question. The UI knows
   * each column's type from the schema it holds, like every client does.
   */
  const spec = (): QuerySpec => {
    const name = columns()[column()] ?? "";
    const raw = text();
    const numeric = /^-?\d+$/.test(raw);
    const unsigned = name === "id" || name.endsWith("_id");
    const value = !numeric
      ? { str: raw }
      : unsigned
        ? { u64: raw }
        : { i64: raw };

    const filter =
      raw === ""
        ? undefined
        : op() === "like" || op() === "ilike"
          ? { op: op(), column: column(), pattern: raw }
          : { op: op(), column: column(), value };

    return {
      table: table(),
      ...(filter ? { filter } : {}),
      sort: [{ column: sortColumn(), direction: descending() ? "desc" : "asc" }],
      limit: limit(),
    };
  };

  const rows = createQuery(() => ({
    queryKey: ["query", props.sdk(), props.persona(), spec()],
    queryFn: () => api.query(props.sdk(), props.persona(), spec()),
  }));

  const plan = createQuery(() => ({
    queryKey: ["explain", props.sdk(), props.persona(), spec()],
    queryFn: () => api.explain(props.sdk(), props.persona(), spec()),
  }));

  return (
    <>
      <div class="panel">
        <h2>Rows</h2>
        <p class="why">
          A filter, an ordering, and a limit, lowered by whichever client is
          selected. Switch identity to watch the row policy take books away
          without anything in the adapter changing.
        </p>
        <div class="controls">
          <label class="field">
            <span>table</span>
            <select
              value={table()}
              onChange={(event) => {
                setTable(event.currentTarget.value);
                setColumn(0);
                setSortColumn(0);
              }}
            >
              <For each={Object.keys(TABLES)}>{(name) => <option>{name}</option>}</For>
            </select>
          </label>
          <label class="field">
            <span>where</span>
            <select
              value={String(column())}
              onChange={(event) => setColumn(Number(event.currentTarget.value))}
            >
              <For each={columns()}>
                {(name, index) => <option value={index()}>{name}</option>}
              </For>
            </select>
          </label>
          <label class="field">
            <span>op</span>
            <select value={op()} onChange={(event) => setOp(event.currentTarget.value)}>
              <For each={["eq", "ne", "lt", "le", "gt", "ge", "like", "ilike"]}>
                {(name) => <option>{name}</option>}
              </For>
            </select>
          </label>
          <label class="field">
            <span>value</span>
            <input value={text()} onInput={(event) => setText(event.currentTarget.value)} />
          </label>
          <label class="field">
            <span>order by</span>
            <select
              value={String(sortColumn())}
              onChange={(event) => setSortColumn(Number(event.currentTarget.value))}
            >
              <For each={columns()}>
                {(name, index) => <option value={index()}>{name}</option>}
              </For>
            </select>
          </label>
          <label class="field">
            <span>direction</span>
            <select
              value={descending() ? "desc" : "asc"}
              onChange={(event) => setDescending(event.currentTarget.value === "desc")}
            >
              <option value="asc">asc</option>
              <option value="desc">desc</option>
            </select>
          </label>
          <label class="field">
            <span>limit</span>
            <input
              type="number"
              min="1"
              max="200"
              value={limit()}
              onInput={(event) => setLimit(Number(event.currentTarget.value) || 1)}
            />
          </label>
        </div>

        <Result answer={rows.data} pending={rows.isPending}>
          {(value) => (
            <>
              <ValueTable columns={columns()} rows={value.rows} />
              <div class="note">{value.rows.length} rows</div>
            </>
          )}
        </Result>
      </div>

      <div class="panel">
        <h2>The plan</h2>
        <p class="why">
          What the planner would do with the query above, without running it.
          Asking is its own privilege: a plan is costed against statistics
          describing rows a policy may hide, so <code>reader</code> is refused
          here while still being allowed to read.
        </p>
        <Result answer={plan.data} pending={plan.isPending}>
          {(value) => (
            <>
              <div class="badges">
                <span class="badge">
                  access <b>{value.access}</b>
                </span>
                <span class="badge" data-tone={value.indexOnly ? "good" : undefined}>
                  index-only <b>{String(value.indexOnly)}</b>
                </span>
                <span class="badge" data-tone={value.sorts ? "warn" : "good"}>
                  sorts <b>{String(value.sorts)}</b>
                </span>
                <span class="badge">
                  est. rows <b>{value.estimatedRows}</b>
                </span>
              </div>
              <pre>{value.display}</pre>
            </>
          )}
        </Result>
      </div>
    </>
  );
}

/** The four join types, over the same two tables. */
export function Joins(props: Context): JSX.Element {
  const [kind, setKind] = createSignal("inner");

  const joined = createQuery(() => ({
    queryKey: ["join", props.sdk(), props.persona(), kind()],
    queryFn: () => api.join(props.sdk(), props.persona(), { type: kind() }),
  }));

  return (
    <div class="panel">
      <h2>Joins</h2>
      <p class="why">
        Authors joined to their books. One author has no books and one book
        names an author who does not exist, so the four types genuinely differ —
        an unmatched side shows as <em>—</em> rather than as a row of nulls,
        because the client keeps them apart.
      </p>
      <div class="controls">
        <label class="field">
          <span>type</span>
          <select value={kind()} onChange={(event) => setKind(event.currentTarget.value)}>
            <For each={["inner", "left", "right", "full"]}>{(name) => <option>{name}</option>}</For>
          </select>
        </label>
      </div>
      <Result answer={joined.data} pending={joined.isPending}>
        {(value) => (
          <>
            <div class="scroll">
              <table>
                <thead>
                  <tr>
                    <For each={["author", "country", "title", "year"]}>
                      {(name) => <th>{name}</th>}
                    </For>
                  </tr>
                </thead>
                <tbody>
                  <For each={value.rows}>
                    {(row) => (
                      <tr>
                        <td class={row.authors ? "" : "absent"}>
                          {row.authors ? render(row.authors[1]) : "—"}
                        </td>
                        <td class={row.authors ? "" : "absent"}>
                          {row.authors ? render(row.authors[2]) : "—"}
                        </td>
                        <td class={row.books ? "" : "absent"}>
                          {row.books ? render(row.books[2]) : "—"}
                        </td>
                        <td class={row.books ? "" : "absent"}>
                          {row.books ? render(row.books[3]) : "—"}
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
            <div class="note">
              {value.rows.length} rows ·{" "}
              {value.rows.filter((row) => !row.authors || !row.books).length} with an
              unmatched side
            </div>
          </>
        )}
      </Result>
    </div>
  );
}

/** A grouped join, drawn as bars. */
export function Groups(props: Context): JSX.Element {
  const [by, setBy] = createSignal("author");
  const [direction, setDirection] = createSignal("desc");
  const [minimum, setMinimum] = createSignal(0);

  const spec = () => ({
    groupBy: by(),
    sort: "count",
    direction: direction(),
    ...(minimum() > 0 ? { having: { minCount: minimum() } } : {}),
  });

  const groups = createQuery(() => ({
    queryKey: ["aggregate", props.sdk(), props.persona(), spec()],
    queryFn: () => api.aggregate(props.sdk(), props.persona(), spec()),
  }));

  const plan = createQuery(() => ({
    queryKey: ["explain-aggregate", props.sdk(), props.persona(), spec()],
    queryFn: () => api.explainAggregate(props.sdk(), props.persona(), spec()),
  }));

  const bars = createMemo(() => {
    const answer = groups.data;
    if (!answer?.ok) return [];
    return answer.value.groups.map((group) => ({
      label: render(group.key[0]),
      value: Number(render(group.count) || "0"),
    }));
  });

  return (
    <div class="panel">
      <h2>Grouped join</h2>
      <p class="why">
        Books per author, counted by the database rather than by the browser:
        the join and the grouping are one request, and the ordering and the
        <code> HAVING</code> are over <em>groups</em>, not over rows. Most of
        the keys below are not columns at all — a decade, a
        <code> CASE</code>, a regular expression, a calendar field, an hour in
        New York — each an expression the SDK sends and the kernel evaluates.
      </p>
      <div class="controls">
        <label class="field">
          <span>group by</span>
          <select value={by()} onChange={(event) => setBy(event.currentTarget.value)}>
            <option value="author">author</option>
            <option value="country">country</option>
            <option value="decade">decade</option>
            {/* Everything below is a *computed* group key of a different
                kind — the point being that a grouping does not have to be a
                column, and that these are the database's expressions rather
                than the browser bucketing rows it fetched. */}
            <option value="shout">author, upper-cased</option>
            <option value="era">era (CASE)</option>
            <option value="tidy">title, tidied (regex)</option>
            <option value="releasedYear">release year (calendar)</option>
            <option value="releasedMonth">release month (calendar)</option>
            <option value="releasedHourNY">release hour, New York</option>
            <option value="label">country/title/year (both tables)</option>
          </select>
        </label>
        <label class="field">
          <span>order</span>
          <select
            value={direction()}
            onChange={(event) => setDirection(event.currentTarget.value)}
          >
            <option value="desc">most first</option>
            <option value="asc">fewest first</option>
          </select>
        </label>
        <label class="field">
          <span>having ≥</span>
          <input
            type="number"
            min="0"
            max="10"
            value={minimum()}
            onInput={(event) => setMinimum(Number(event.currentTarget.value) || 0)}
          />
        </label>
      </div>
      <Result answer={groups.data} pending={groups.isPending}>
        {(value) => (
          <>
            <Bars rows={bars()} />
            <div class="note">{value.groups.length} groups</div>
          </>
        )}
      </Result>

      <h3 style={{ "margin-top": "18px" }}>How the database will do it</h3>
      <p class="why">
        The plan of the <em>grouped</em> read, which is not the plan of the join
        underneath it. Grouping narrows each input to the group keys and the
        aggregates&rsquo; columns — <code>decodes</code> is that list, and it is
        the only visible difference where no index applies, because narrowing
        changes what a row costs to read and not how it is found.
      </p>
      <Result answer={plan.data} pending={plan.isPending}>
        {(value) => (
          <>
            <div class="badges">
              <For each={value.inputs}>
                {(input) => (
                  <span class="badge" data-tone={input.indexOnly ? "good" : undefined}>
                    {input.table} <b>{input.access}</b> decodes{" "}
                    <b>[{input.decodes.join(", ")}]</b>
                    {input.algorithm ? ` · ${input.algorithm}` : ""}
                  </span>
                )}
              </For>
            </div>
            <pre>{value.display}</pre>
          </>
        )}
      </Result>
    </div>
  );
}

/** A write that only its own transaction can see, until it commits. */
export function Transactions(props: Context): JSX.Element {
  const [commit, setCommit] = createSignal(true);
  const [ran, setRan] = createSignal(0);

  const outcome = createQuery(() => ({
    queryKey: ["transaction", props.sdk(), props.persona(), commit(), ran()],
    queryFn: () => api.transaction(props.sdk(), props.persona(), commit()),
    enabled: ran() > 0,
  }));

  return (
    <div class="panel">
      <h2>Transactions</h2>
      <p class="why">
        Inserts a book inside a transaction, reads it back <em>inside</em>, then
        commits or rolls back and looks again from outside. The only thing here
        a single request cannot show.
      </p>
      <div class="controls">
        <label class="field">
          <span>ending</span>
          <select
            value={commit() ? "commit" : "rollback"}
            onChange={(event) => setCommit(event.currentTarget.value === "commit")}
          >
            <option value="commit">commit</option>
            <option value="rollback">roll back</option>
          </select>
        </label>
        <button
          type="button"
          class="seg"
          style={{ padding: "6px 14px", cursor: "pointer" }}
          onClick={() => setRan(ran() + 1)}
        >
          run it
        </button>
      </div>
      <Show when={ran() > 0} fallback={<div class="note">not run yet</div>}>
        <Result answer={outcome.data} pending={outcome.isPending}>
          {(value) => (
            <div class="badges">
              <span class="badge" data-tone={value.visibleInside ? "good" : "bad"}>
                visible inside the transaction <b>{String(value.visibleInside)}</b>
              </span>
              <span class="badge" data-tone={value.visibleAfter ? "good" : "warn"}>
                visible afterwards <b>{String(value.visibleAfter)}</b>
              </span>
            </div>
          )}
        </Result>
      </Show>
    </div>
  );
}

/**
 * The same request through all three SDKs at once.
 *
 * The panel that makes the claim checkable rather than asserted: if the three
 * ever disagree, this says so on screen, and
 * `conformance/conformance.py` says so more thoroughly from a terminal.
 */
export function Agreement(props: Context): JSX.Element {
  const spec: QuerySpec = {
    table: "books",
    sort: [{ column: 0, direction: "asc" }],
    limit: 50,
  };

  const answers = createQuery(() => ({
    queryKey: ["agreement", props.persona()],
    queryFn: async () => {
      const sdks = ["go", "node", "python"] as const;
      const results = await Promise.all(
        sdks.map((sdk) => api.query(sdk, props.persona(), spec)),
      );
      return sdks.map((sdk, index) => ({ sdk, answer: results[index]! }));
    },
  }));

  return (
    <div class="panel">
      <h2>Do the three agree?</h2>
      <p class="why">
        One query, sent to all three adapters at once, and their answers
        compared as text. Each client's own tests run against this same server —
        which catches one client being wrong and cannot catch two being wrong
        the same way. This can.
      </p>
      <Show when={answers.data} keyed fallback={<div class="spinner">asking all three…</div>}>
        {(rows) => {
          const rendered = rows.map((row) => JSON.stringify(row.answer));
          const agree = new Set(rendered).size === 1;
          return (
            <>
              <div class="badges">
                <span class="badge" data-tone={agree ? "good" : "bad"}>
                  <b>{agree ? "identical" : "they disagree"}</b>
                </span>
                <For each={rows}>
                  {(row) => (
                    <span class="badge">
                      {row.sdk}{" "}
                      <b>
                        {row.answer.ok
                          ? `${row.answer.value.rows.length} rows`
                          : row.answer.error.kind}
                      </b>
                    </span>
                  )}
                </For>
              </div>
              <Show when={!agree}>
                <pre>{rendered.join("\n\n")}</pre>
              </Show>
              <div class="note">
                For the thorough version, run{" "}
                <code>python3 conformance/conformance.py</code> — 31 cases,
                including every join type, group orderings with ties, and five
                refusals.
              </div>
            </>
          );
        }}
      </Show>
    </div>
  );
}
