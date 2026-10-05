/** The demo's panels. Each is one thing the database does. */
import { createMemo, createSignal, For, Show, type JSX } from "solid-js";
import { createQuery } from "@tanstack/solid-query";

import {
  api,
  money,
  render,
  TABLES,
  VIEWS,
  type Persona,
  type QuerySpec,
  type Answer,
  type BatchOutcome,
  type Sdk,
  type Tagged,
  type WindowSpec,
  SDKS,
} from "./api";
import { Bars, Result, Segmented, ValueTable } from "./parts";

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

  const columns = () => TABLES[table()] ?? VIEWS[table()] ?? [];

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
              <For each={[...Object.keys(TABLES), ...Object.keys(VIEWS)]}>
                {(name) => <option>{name}</option>}
              </For>
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
        <p class="why">
          Pick <code>classics</code> above and this panel refuses too, for a
          different reason: it is a view, and only a plain read may go through
          one. The rows still come back — and fewer of them as{" "}
          <code>reader</code>, because the view is substituted away before
          planning and it is <em>books</em>&apos; row policy that runs.
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
  const [measure, setMeasure] = createSignal("count");

  const spec = () => ({
    groupBy: by(),
    // The measure is also the sort key, which is the whole of why it is a
    // control rather than a second column: every group carries both numbers
    // either way, and the only thing that changes visibly is which one the
    // order and the bars are about.
    sort: measure(),
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
    // A decimal's tag carries *units* — cents here — so the bar is drawn in
    // them and only the label is rendered against the scale. Dividing first
    // would put a float on the chart's axis for a value the database keeps
    // exact, which is the one thing a money column exists to avoid.
    return answer.value.groups.map((group) =>
      measure() === "total"
        ? {
            label: `${render(group.key[0])} — ${money(render(group.total))}`,
            value: Number(render(group.total) || "0"),
          }
        : {
            label: render(group.key[0]),
            value: Number(render(group.count) || "0"),
          },
    );
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
          <span>measure</span>
          <select
            value={measure()}
            onChange={(event) => setMeasure(event.currentTarget.value)}
            data-test="groups-measure"
          >
            <option value="count">how many books</option>
            <option value="total">what they cost (decimal)</option>
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
            <div class="note" data-test="groups-note">
              {value.groups.length} groups
              {measure() === "total"
                ? ` · SUM(books.price), in cents, rendered against the column's scale of 2`
                : ""}
            </div>
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
                <code>python3 conformance/conformance.py</code>, which runs
                every endpoint through all three — every join type, group
                orderings with ties, and the refusals.
              </div>
            </>
          );
        }}
      </Show>
    </div>
  );
}

/** A write that names rows by predicate, and hands them back. */
export function PredicateWrites(props: Context): JSX.Element {
  const [kind, setKind] = createSignal<"delete" | "update">("delete");
  const [returning, setReturning] = createSignal(true);
  const [ran, setRan] = createSignal(0);

  const outcome = createQuery(() => ({
    queryKey: ["predicate-write", props.sdk(), props.persona(), kind(), returning(), ran()],
    queryFn: () =>
      api.predicateWrite(props.sdk(), props.persona(), {
        kind: kind(),
        returning: returning(),
      }),
    enabled: ran() > 0,
  }));

  return (
    <div class="panel">
      <h2>Predicate writes</h2>
      <p class="why">
        <code>DELETE … WHERE</code> and <code>UPDATE … SET … WHERE</code>: one
        statement that names its rows by a condition rather than by key. It
        seeds four books, writes over the two whose year is 2002 or later, and
        reports what happened.
      </p>
      <p class="why">
        <b>Ask for the rows back and the difference is the point.</b> Without{" "}
        <code>returning</code> you get a count, which a delete-by-key would
        also give you. With it you get the rows as they were — the only record
        of what a delete destroyed, and the only way to know which rows an
        update touched without reading them again and racing.
      </p>
      <div class="controls">
        <label class="field">
          <span>write</span>
          <select
            value={kind()}
            onChange={(event) =>
              setKind(event.currentTarget.value === "update" ? "update" : "delete")
            }
          >
            <option value="delete">delete where</option>
            <option value="update">update where</option>
          </select>
        </label>
        <label class="field">
          <span>returning</span>
          <select
            value={returning() ? "yes" : "no"}
            onChange={(event) => setReturning(event.currentTarget.value === "yes")}
          >
            <option value="yes">the rows</option>
            <option value="no">a count only</option>
          </select>
        </label>
        <button
          type="button"
          class="seg"
          style={{ padding: "6px 14px", cursor: "pointer" }}
          onClick={() => setRan(ran() + 1)}
          data-test="predicate-run"
        >
          run it
        </button>
      </div>
      <Show when={ran() > 0} fallback={<div class="note">not run yet</div>}>
        <Result answer={outcome.data} pending={outcome.isPending}>
          {(value) => (
            <>
              <div class="badges" data-test="predicate-summary">
                <span class="badge" data-tone="good">
                  affected <b>{value.affected}</b>
                </span>
                <span class="badge">
                  of four left <b>{value.left}</b>
                </span>
                <span class="badge" data-tone={value.rows.length > 0 ? "good" : "warn"}>
                  rows returned <b>{value.rows.length}</b>
                </span>
              </div>
              <Show
                when={value.rows.length > 0}
                fallback={
                  <div class="note">
                    A count and nothing else. The rows are gone and this is all
                    that is left of them — which is the argument for{" "}
                    <code>returning</code>, made by its absence.
                  </div>
                }
              >
                <ValueTable columns={TABLES["books"] ?? []} rows={value.rows} />
              </Show>
            </>
          )}
        </Result>
      </Show>
    </div>
  );
}

/**
 * Retiring a row, and taking it back.
 *
 * The half of soft delete that is easy to forget exists, and until recently did
 * not: a retired row could be written by nobody at any privilege, so the only
 * thing that could ever happen to one was being erased when the retention
 * window closed. A retention window is an *undo* window, and this is the undo.
 *
 * The three points are the panel. "It is live now" is also what a handler that
 * quietly inserted a fresh row at the same key would report, so the row's
 * columns are shown carried through — that is the difference between restoring
 * a row and replacing it, and it is not visible from the id alone.
 */
export function SoftDelete(props: Context): JSX.Element {
  const [ran, setRan] = createSignal(0);
  const [asking, setAsking] = createSignal(false);

  const outcome = createQuery(() => ({
    queryKey: ["restore", props.sdk(), props.persona(), ran()],
    queryFn: () => api.restore(props.sdk(), props.persona()),
    enabled: ran() > 0,
  }));

  // A plain read of `shipments`, with and without the flag.
  //
  // `ran()` is in the key on purpose: the retire-and-undo above writes to this
  // table, so a read cached across it would show the state from before. That
  // is the only coupling between the two halves of this panel, and it is one
  // line rather than a second endpoint.
  const shipments = createQuery(() => ({
    queryKey: ["shipments", props.sdk(), props.persona(), asking(), ran()],
    queryFn: () =>
      api.query(props.sdk(), props.persona(), {
        table: "shipments",
        sort: [{ column: 0, direction: "asc" }],
        ...(asking() ? { includeDeleted: true } : {}),
      }),
  }));

  return (
    <div class="panel">
      <h2>Soft delete, and undo</h2>
      <p class="why">
        <code>shipments</code> declares <code>soft_delete = "deleted_at"</code>,
        so a <code>delete</code> stamps the row and leaves it where it is. Every
        ordinary read hides it. This retires a shipment, shows that an ordinary
        read no longer returns it, and then brings it back.
      </p>
      <p class="why">
        <b>There is no restore verb.</b> Bringing a row back is an ordinary{" "}
        <code>update</code> at its key with null in the soft-delete column — the
        client's generated <code>restored()</code> clears whichever column the
        catalog names, so nothing here hard-codes <code>deleted_at</code>. It
        needs the <code>read_deleted</code> action, the same grant that lets you{" "}
        <i>see</i> a retired row: a write that names a key reaches a hidden row
        only for a caller who could have read it.
      </p>
      <div class="controls">
        <button
          type="button"
          class="seg"
          style={{ padding: "6px 14px", cursor: "pointer" }}
          onClick={() => setRan(ran() + 1)}
          data-test="restore-run"
        >
          retire it, then undo
        </button>
      </div>
      <Show when={ran() > 0} fallback={<div class="note">not run yet</div>}>
        <Result answer={outcome.data} pending={outcome.isPending}>
          {(value) => (
            <>
              <div class="badges" data-test="restore-summary">
                <span class="badge" data-tone={value.retired_before ? "good" : "warn"}>
                  retired first <b>{value.retired_before ? "yes" : "no"}</b>
                </span>
                <span
                  class="badge"
                  data-tone={value.hidden_while_retired.length === 0 ? "good" : "warn"}
                >
                  an ordinary read saw <b>{value.hidden_while_retired.length}</b>
                </span>
                <span
                  class="badge"
                  data-tone={value.visible_after.length === 1 ? "good" : "warn"}
                >
                  after the undo it sees <b>{value.visible_after.length}</b>
                </span>
                <span
                  class="badge"
                  data-tone={value.retired_after.every((r) => !r) ? "good" : "warn"}
                >
                  still stamped <b>{value.retired_after.filter(Boolean).length}</b>
                </span>
              </div>
              <div class="note" data-test="restore-carried">
                The row came back with <code>status</code>{" "}
                <b>{value.status_after.join(", ") || "—"}</b> and{" "}
                <code>book_id</code> <b>{value.book_id_after.join(", ") || "—"}</b>,
                which is what says it is the same row rather than a fresh one
                written at the key it left free. A replacement would report the
                same id and the same "not stamped" and differ only here.
              </div>
            </>
          )}
        </Result>
      </Show>

      <h3 style={{ "margin-top": "18px" }}>Asking for them anyway</h3>
      <p class="why">
        A retired row is still there, and <code>include_deleted</code> is how a
        caller says so. It is <b>privileged</b> — a separate{" "}
        <code>read_deleted</code> action rather than part of <code>read</code> —
        which is the whole reason it is a control here and not a checkbox on
        every panel: switch the identity above to <code>reader</code> and the
        same request comes back <i>refused</i> rather than empty. That is the
        distinction a soft delete has to make and an ordinary filter cannot:
        &ldquo;there are none&rdquo; and &ldquo;you may not see them&rdquo; are
        different answers, and a read that quietly returned nothing would
        collapse them.
      </p>
      <div class="controls">
        <label class="field">
          <span>retired rows</span>
          <select
            value={asking() ? "yes" : "no"}
            onChange={(event) => setAsking(event.currentTarget.value === "yes")}
            data-test="include-deleted"
          >
            <option value="no">an ordinary read</option>
            <option value="yes">include_deleted</option>
          </select>
        </label>
      </div>
      {/* Wrapped so a check can read *this* read's answer rather than the
          panel's text: the retire-and-undo above is refused for a reader too,
          so an assertion scoped to the panel finds a refusal either way and
          passes while checking nothing. A mutation that never set the flag
          survived exactly that. */}
      <div data-test="include-deleted-answer">
      <Result answer={shipments.data} pending={shipments.isPending}>
        {(value) => (
          <>
            <div class="note" data-test="include-deleted-count">
              {value.rows.length} shipments
              {asking() ? ", retired ones included" : ", retired ones hidden"}
            </div>
            <ValueTable columns={TABLES["shipments"] ?? []} rows={value.rows} />
          </>
        )}
      </Result>
      </div>
    </div>
  );
}

/** Several writes in one request, under each of the two atomicities. */
export function Batches(props: Context): JSX.Element {
  const [ran, setRan] = createSignal(0);

  // Both atomicities, side by side, from **one** query that runs them in
  // sequence. The difference between them is the only thing a batch has to
  // teach that a loop of writes does not, and a control that ran one at a time
  // would leave the visitor holding the other one's answer in their head.
  //
  // Sequential, and that is not a style choice. As two concurrent queries they
  // raced: the handler clears and re-seeds the same four rows, so whichever
  // arrived second saw the other's half-finished state and came back a
  // refusal — the panel then showed one column and a timeout in the e2e. Two
  // runs that mutate the same rows have to be ordered, and the number each
  // reports is only meaningful if they are.
  const both = createQuery(() => ({
    queryKey: ["batch", props.sdk(), props.persona(), ran()],
    queryFn: async () => ({
      independent: await api.batch(props.sdk(), props.persona(), "independent"),
      atomic: await api.batch(props.sdk(), props.persona(), "all-or-nothing"),
    }),
    enabled: ran() > 0,
  }));

  return (
    <div class="panel">
      <h2>Batches</h2>
      <p class="why">
        Three writes in one request, the second of which collides with a key
        that is already taken. Both atomicities run, because the{" "}
        <b>difference between them is the whole feature</b> — a batch that
        never failed would be a round-trip saving and nothing more.
      </p>
      <p class="why">
        <code>independent</code> applies each on its own: the request succeeds,
        and the failure is <em>one of the outcomes</em> rather than an
        exception. <code>all-or-nothing</code> puts them in a transaction: the
        call fails, there are no per-operation outcomes to report, and the two
        writes that would have worked did not happen either. Watch the{" "}
        <b>rows left</b> counts.
      </p>
      <div class="controls">
        <button
          type="button"
          class="seg"
          style={{ padding: "6px 14px", cursor: "pointer" }}
          onClick={() => setRan(ran() + 1)}
          data-test="batch-run"
        >
          run both
        </button>
      </div>
      <Show when={ran() > 0} fallback={<div class="note">not run yet</div>}>
        <Show when={both.data} keyed fallback={<div class="spinner">asking…</div>}>
          {(ran) => (
            <div class="split" data-test="batch-outcomes">
              <Side name="independent" answer={ran.independent} />
              <Side name="all-or-nothing" answer={ran.atomic} />
            </div>
          )}
        </Show>
      </Show>
    </div>
  );
}

/**
 * One atomicity's column in the batch panel.
 *
 * Written out twice rather than looped. A `<For>` over a freshly built tuple
 * array gives every item a new reference on each render, so the row is
 * recreated rather than updated — which left one of the two columns showing
 * its heading and nothing else. Two columns are not worth a loop whose
 * reactivity has to be reasoned about.
 */
function Side(props: { name: string; answer: Answer<BatchOutcome> }): JSX.Element {
  return (
    <div data-test={`batch-${props.name}`}>
      <h3>{props.name}</h3>
      <Result answer={props.answer} pending={false}>
        {(value) => (
          <>
            <div class="badges">
              {/* `||`, not `??`: the adapters report "nothing failed" as an
                  empty string rather than as null, and `??` would leave the
                  interesting word blank. */}
              <span class="badge" data-tone={value.failed ? "bad" : "good"}>
                the call <b>{value.failed || "succeeded"}</b>
              </span>
              <span class="badge">
                rows left <b>{value.left}</b>
              </span>
            </div>
            <Show
              when={value.outcomes.length > 0}
              fallback={
                <div class="note">
                  No per-operation outcomes, which is not missing information:
                  they all landed or none did, so there is nothing to report
                  per operation.
                </div>
              }
            >
              <ol class="outcomes">
                <For each={value.outcomes}>
                  {(one) => (
                    <li>
                      {"ok" in one ? (
                        <span class="badge" data-tone="good">
                          wrote <b>{one.ok}</b>
                        </span>
                      ) : (
                        <span class="badge" data-tone="bad">
                          <b>{one.kind}</b> {one.reason}
                        </span>
                      )}
                    </li>
                  )}
                </For>
              </ol>
            </Show>
          </>
        )}
      </Result>
    </div>
  );
}

/** A relationship path: two steps, one request. */
export function Relationships(props: Context): JSX.Element {
  const [shape, setShape] = createSignal<"trees" | "through">("trees");

  // Books 10, 11 and 12: two editions, one, and none. A uniform fixture would
  // let a wrong regrouping produce a plausible answer.
  const keys: Tagged[] = [{ u64: "10" }, { u64: "11" }, { u64: "12" }];

  const answer = createQuery(() => ({
    queryKey: ["path", props.sdk(), props.persona()],
    queryFn: () => api.path(props.sdk(), props.persona(), keys),
  }));

  return (
    <div class="panel">
      <h2>Relationships</h2>
      <p class="why">
        <code>sales → books → editions</code>: up to the book a sale sold, then
        down to that book's editions. Two steps in opposite directions, in{" "}
        <b>one request and two reads</b> — not two reads per key, which is the
        N+1 this whole layer exists to prevent.
      </p>
      <p class="why">
        The same request, read two ways. <b>tree</b> keeps every level, so a
        book with no editions is a node with nothing under it. <b>through</b>{" "}
        drops the middles and hands back the far rows only. The difference is
        one line in each SDK — and it is the line that reads "the rows at the
        bottom" rather than "the rows with nothing below them", which are the
        same answer on every input except this one.
      </p>
      <div class="controls">
        <Segmented
          label="read it as"
          value={shape()}
          options={["trees", "through"] as const}
          onChange={setShape}
        />
      </div>
      <Result answer={answer.data} pending={answer.isPending}>
        {(value) => (
          <Show
            when={shape() === "trees"}
            fallback={
              <div data-test="path-through">
                <For each={value.through}>
                  {(rows, at) => (
                    <div class="level">
                      <h3>
                        book {render(keys[at()])} — {rows.length} edition
                        {rows.length === 1 ? "" : "s"}
                      </h3>
                      <ValueTable
                        columns={TABLES["editions"] ?? []}
                        rows={rows}
                        empty="no editions, and the book itself is not here — that is what dropping the middle means"
                      />
                    </div>
                  )}
                </For>
              </div>
            }
          >
            <div data-test="path-trees">
              <For each={value.trees}>
                {(tree, at) => (
                  <div class="level">
                    <h3>book {render(keys[at()])}</h3>
                    <Show
                      when={tree.length > 0}
                      fallback={
                        <div class="note">
                          no book at all — an empty tree, which is not the same
                          as a book with no editions
                        </div>
                      }
                    >
                      <For each={tree}>
                        {(node) => (
                          <>
                            <ValueTable
                              columns={TABLES["books"] ?? []}
                              rows={[node.row]}
                            />
                            <ValueTable
                              columns={TABLES["editions"] ?? []}
                              rows={node.related}
                              empty="this book has no editions — the level is kept and empty"
                            />
                          </>
                        )}
                      </For>
                    </Show>
                  </div>
                )}
              </For>
            </div>
          </Show>
        )}
      </Result>
    </div>
  );
}

/**
 * Full-text search, and the access path that answered it.
 *
 * The last surface `contains` was missing: it is on the wire, in all three
 * clients, in the SQL front end and in the conformance corpus, and a visitor
 * could not type a word into it. `ledger/2026-09-21-contains-in-the-sql-front-end.md`
 * recorded that as the only part of F6b left.
 *
 * **The access path is the panel, not the rows.** A text index and a table scan
 * return the same books for the same word — by construction, because a
 * `contains` with no index is the same predicate applied to every row. So a
 * panel showing only results would show nothing a reader could act on, and an
 * adapter that ignored the requested path would look correct. The plan is what
 * tells them apart, which is why it is a badge rather than a footnote.
 */
export function Search(props: Context): JSX.Element {
  const [text, setText] = createSignal("the");
  const [path, setPath] = createSignal<"index" | "scan">("index");

  const outcome = createQuery(() => ({
    queryKey: ["search", props.sdk(), props.persona(), text(), path()],
    queryFn: () => api.search(props.sdk(), props.persona(), { text: text(), path: path() }),
    // Runs on load and on every keystroke's settled value, unlike the write
    // panels: a search is a read and re-running it costs a query.
    enabled: text().length > 0,
  }));

  return (
    <div class="panel">
      <h2>Full-text search</h2>
      <p class="why">
        <code>books.title</code> carries a text index, and{" "}
        <code>title contains "…"</code> can be answered two ways: by walking the
        index for the term, or by scanning the table and testing every row. Both
        return the same books.
      </p>
      <p class="why">
        <b>Which is why the plan is shown and the rows are only the evidence.</b>{" "}
        An adapter that ignored the path you chose would return exactly these
        rows and look right. The <code>access</code> badge is the one thing that
        distinguishes them — and it is absent for the <code>reader</code>{" "}
        persona, because <code>EXPLAIN</code> is privileged: a plan is costed
        against statistics covering rows that reader's policy hides.
      </p>
      <div class="controls">
        <label class="field">
          <span>title contains</span>
          <input
            type="text"
            value={text()}
            onInput={(event) => setText(event.currentTarget.value)}
            data-test="search-text"
          />
        </label>
        <label class="field">
          <span>answer it by</span>
          <select
            value={path()}
            onChange={(event) =>
              setPath(event.currentTarget.value === "scan" ? "scan" : "index")
            }
            data-test="search-path"
          >
            <option value="index">the text index</option>
            <option value="scan">a table scan</option>
          </select>
        </label>
      </div>
      <Show when={text().length > 0} fallback={<div class="note">type a word</div>}>
        <Result answer={outcome.data} pending={outcome.isPending}>
          {(value) => (
            <>
              <div class="badges" data-test="search-summary">
                <span class="badge" data-tone={value.rows.length > 0 ? "good" : "warn"}>
                  matched <b>{value.rows.length}</b>
                </span>
                <span class="badge" data-tone={value.access ? "good" : "warn"}>
                  access <b>{value.access ?? "not visible to this persona"}</b>
                </span>
              </div>
              <Show
                when={value.rows.length > 0}
                fallback={
                  <div class="note">
                    No title contains that. A search matching nothing is an
                    empty answer, not a failure — and it costs the same walk.
                  </div>
                }
              >
                <ValueTable columns={TABLES["books"] ?? []} rows={value.rows} />
              </Show>
            </>
          )}
        </Result>
      </Show>
    </div>
  );
}

/**
 * Optimistic concurrency, both halves, side by side.
 *
 * `update … expected` and `delete … expected`: a write that carries the row as
 * the caller read it and is refused if the stored row has moved. Two entries
 * recorded that the endpoints existed, the conformance corpus drove them, and a
 * visitor saw nothing.
 *
 * **The stale case is the only one that shows anything.** An unconditional
 * update and a conditional one over an unchanged row do exactly the same thing,
 * so a panel that only ever succeeded would demonstrate nothing — which is why
 * the control is *whether somebody else writes first* rather than whether to
 * guard.
 *
 * The delete has three outcomes where the update has two, and the third is the
 * one worth the panel: a *plain* delete of an absent key reports `affected: 0`,
 * and a conditional one refuses it. "Nothing happened" and "somebody else got
 * there" are different answers, and only the conditional form distinguishes
 * them.
 */
export function ConditionalWrites(props: Context): JSX.Element {
  const [write, setWrite] = createSignal<"update" | "delete">("update");
  const [meddle, setMeddle] = createSignal<"none" | "moved" | "gone">("none");
  const [ran, setRan] = createSignal(0);

  const updated = createQuery(() => ({
    queryKey: ["conditional-update", props.sdk(), props.persona(), meddle(), ran()],
    queryFn: () => api.conditionalUpdate(props.sdk(), props.persona(), meddle() === "moved"),
    enabled: ran() > 0 && write() === "update",
  }));

  const deleted = createQuery(() => ({
    queryKey: ["conditional-delete", props.sdk(), props.persona(), meddle(), ran()],
    queryFn: () =>
      api.conditionalDelete(props.sdk(), props.persona(), {
        stale: meddle() === "moved",
        gone: meddle() === "gone",
      }),
    enabled: ran() > 0 && write() === "delete",
  }));

  return (
    <div class="panel">
      <h2>Conditional writes</h2>
      <p class="why">
        A write that carries the row as you read it. The server compares it with
        what is stored and refuses if somebody moved it — the lost-update
        problem solved without holding a lock, and without a version column the
        schema has to carry.
      </p>
      <p class="why">
        <b>Let nobody else write and both forms look like ordinary writes.</b>{" "}
        That is why the control is what the other writer does. The delete has a
        third case the update does not: against a row that is <i>gone</i>, a
        plain delete reports nothing affected and a conditional one refuses —
        "there was nothing to do" and "somebody got there first" are different
        answers and only one form tells you which.
      </p>
      <div class="controls">
        <label class="field">
          <span>write</span>
          <select
            value={write()}
            onChange={(event) =>
              setWrite(event.currentTarget.value === "delete" ? "delete" : "update")
            }
            data-test="conditional-write"
          >
            <option value="update">update … expected</option>
            <option value="delete">delete … expected</option>
          </select>
        </label>
        <label class="field">
          <span>meanwhile somebody</span>
          <select
            value={meddle()}
            onChange={(event) => {
              const chosen = event.currentTarget.value;
              setMeddle(chosen === "moved" ? "moved" : chosen === "gone" ? "gone" : "none");
            }}
            data-test="conditional-meddle"
          >
            <option value="none">does nothing</option>
            <option value="moved">changes the price</option>
            <option value="gone" disabled={write() === "update"}>
              deletes the row
            </option>
          </select>
        </label>
        <button
          type="button"
          class="seg"
          style={{ padding: "6px 14px", cursor: "pointer" }}
          onClick={() => setRan(ran() + 1)}
          data-test="conditional-run"
        >
          run it
        </button>
      </div>
      <Show when={ran() > 0} fallback={<div class="note">not run yet</div>}>
        <Show when={write() === "update"}>
          <Result answer={updated.data} pending={updated.isPending}>
            {(value) => (
              <div class="badges" data-test="conditional-summary">
                <span class="badge" data-tone={value.refused ? "warn" : "good"}>
                  {value.refused ? "refused" : "applied"}{" "}
                  <b>{value.refused || "the guarded update"}</b>
                </span>
                <span class="badge">
                  price now <b>{value.rendered}</b>
                </span>
                <span class="badge">
                  as stored <b>{render(value.price)}</b>
                </span>
              </div>
            )}
          </Result>
        </Show>
        <Show when={write() === "delete"}>
          <Result answer={deleted.data} pending={deleted.isPending}>
            {(value) => (
              <div class="badges" data-test="conditional-summary">
                <span class="badge" data-tone={value.refused ? "warn" : "good"}>
                  {value.refused ? "refused" : "applied"}{" "}
                  <b>{value.refused || "the guarded delete"}</b>
                </span>
                <span class="badge">
                  affected <b>{value.affected}</b>
                </span>
                <span class="badge" data-tone={value.left ? "warn" : "good"}>
                  row <b>{value.left ? "still there" : "gone"}</b>
                </span>
              </div>
            )}
          </Result>
        </Show>
      </Show>
    </div>
  );
}

/**
 * Three tables in one read: authors, their books, and those books' sales.
 *
 * The answer is the join panel's with a third side, and a reader cannot tell
 * from the row count alone whether the third table was attached to the right
 * one: attached to `authors` instead of `books`, it would still return rows.
 * The `sales` column naming the same book as the `books` column is the check.
 */
export function Chains(props: Context): JSX.Element {
  const [kind, setKind] = createSignal("inner");
  const chained = createQuery(() => ({
    queryKey: ["chain", props.sdk(), props.persona(), kind()],
    queryFn: () => api.chain(props.sdk(), props.persona(), { type: kind(), limit: 30 }),
  }));
  const cell = (row: Tagged[] | null, column: number) => (
    <td class={row ? "" : "absent"}>{row ? render(row[column]) : "—"}</td>
  );

  return (
    <div class="panel">
      <h2>Chains</h2>
      <p class="why">
        Authors, their books, and each book's sales, as one request. It is not a
        separate kind of query: a join with a third input, which the server
        plans as a chain. The third table attaches to the <i>second</i>, so the
        <code>book</code> column under sales always names the book beside it — a
        client that attached it to authors would still return rows, and they
        would be wrong.
      </p>
      <div class="controls">
        <label class="field">
          <span>type</span>
          <select
            value={kind()}
            onChange={(event) => setKind(event.currentTarget.value)}
            data-test="chain-type"
          >
            <For each={["inner", "left"]}>{(name) => <option>{name}</option>}</For>
          </select>
        </label>
      </div>
      <Result answer={chained.data} pending={chained.isPending}>
        {(value) => (
          <div class="scroll">
            <table data-test="chain-rows">
              <thead>
                <tr>
                  <For each={["author", "title", "sale", "book", "units"]}>
                    {(name) => <th>{name}</th>}
                  </For>
                </tr>
              </thead>
              <tbody>
                <For each={value.rows}>
                  {(row) => (
                    <tr>
                      {cell(row.authors, 1)}
                      {cell(row.books, 2)}
                      {cell(row.sales, 0)}
                      {cell(row.sales, 1)}
                      {cell(row.sales, 2)}
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        )}
      </Result>
    </div>
  );
}

/** Window functions: a value per row, computed over the rows around it. */
export function Windows(props: Context): JSX.Element {
  const [fn, setFn] = createSignal<WindowSpec["function"]>("rank");
  const [partition, setPartition] = createSignal(true);
  const [running, setRunning] = createSignal(false);
  const spec = (): WindowSpec => ({
    function: fn(),
    partition: partition(),
    running: running(),
    limit: 20,
  });
  const windowed = createQuery(() => ({
    queryKey: ["window", props.sdk(), props.persona(), spec()],
    queryFn: () => api.window(props.sdk(), props.persona(), spec()),
  }));
  const aggregate = () => fn() === "sum" || fn() === "count";

  return (
    <div class="panel">
      <h2>Windows</h2>
      <p class="why">
        A window function keeps every row and adds a value computed over its
        neighbours — the rank of a book among its author's, the year of the
        book before it. Grouping would collapse the rows; this does not. The
        window is ordered by year.
      </p>
      <p class="why">
        <b>Turn on "running" for sum or count</b> and the same function means
        something else: with an order, the frame is the rows <i>up to</i> this
        one, so the total grows down the column instead of repeating.
      </p>
      <div class="controls">
        <label class="field">
          <span>function</span>
          <select
            value={fn()}
            onChange={(event) => setFn(event.currentTarget.value as WindowSpec["function"])}
            data-test="window-function"
          >
            <For each={["rowNumber", "rank", "denseRank", "lag", "lead", "sum", "count"]}>
              {(name) => <option>{name}</option>}
            </For>
          </select>
        </label>
        <label class="field">
          <span>per author</span>
          <input
            type="checkbox"
            checked={partition()}
            onChange={(event) => setPartition(event.currentTarget.checked)}
          />
        </label>
        <label class="field">
          <span>running</span>
          <input
            type="checkbox"
            checked={running()}
            disabled={!aggregate()}
            onChange={(event) => setRunning(event.currentTarget.checked)}
          />
        </label>
      </div>
      <Result answer={windowed.data} pending={windowed.isPending}>
        {(value) => (
          <ValueTable
            columns={["id", "author_id", "title", "year", fn()]}
            rows={value.rows.map((one) => [...one.row.slice(0, 4), one.windowed[0]])}
          />
        )}
      </Result>
    </div>
  );
}

/** The fixed vector `/api/nearest` measures from, as CONTRACT.md gives it. */
const NEAREST_TO = [0.1, 0.2, 0.3, 0.4];

/** Cosine distance, computed here only so the order can be checked by eye. */
function cosineDistance(a: number[], b: number[]): number {
  let dot = 0;
  let na = 0;
  let nb = 0;
  a.forEach((x, i) => {
    const y = b[i] ?? 0;
    dot += x * y;
    na += x * x;
    nb += y * y;
  });
  return 1 - dot / Math.sqrt(na * nb);
}

/** Nearest-neighbour search over the books' embeddings. */
export function Nearest(props: Context): JSX.Element {
  const [limit, setLimit] = createSignal(5);
  const ranked = createQuery(() => ({
    queryKey: ["nearest", props.sdk(), props.persona(), limit()],
    queryFn: () => api.nearest(props.sdk(), props.persona(), limit()),
  }));
  // Every book's embedding, read separately, so the page can show the
  // distance beside each title. The server sends titles only — distances are
  // floats and the three clients print floats differently — which left the
  // order impossible to check by eye. Computing them here is for the reader,
  // not part of the answer.
  const embeddings = createQuery(() => ({
    queryKey: ["embeddings", props.sdk(), props.persona()],
    queryFn: () =>
      api.query(props.sdk(), props.persona(), { table: "books", columns: [2, 6], limit: 100 }),
  }));
  const distance = (title: Tagged): string => {
    const all = embeddings.data;
    if (!all?.ok) return "";
    // A projected row keeps every column's ordinal, nulls where it was not
    // asked for, so the title is still at 2 and the embedding at 6.
    const match = all.value.rows.find((row) => render(row[2]) === render(title));
    const vector = match?.[6];
    if (!vector || !("vector" in vector)) return "";
    return cosineDistance(NEAREST_TO, vector.vector.map(Number)).toFixed(4);
  };

  return (
    <div class="panel">
      <h2>Nearest neighbours</h2>
      <p class="why">
        Books ranked by cosine distance from the vector{" "}
        <code>[{NEAREST_TO.join(", ")}]</code>, against each book's four-number
        embedding. The ranking is the server's; the distance column is worked
        out in this page from the stored embeddings so you can check it.
      </p>
      <div class="controls">
        <label class="field">
          <span>how many</span>
          <select
            value={String(limit())}
            onChange={(event) => setLimit(Number(event.currentTarget.value))}
          >
            <For each={[3, 5, 10]}>{(n) => <option value={String(n)}>{n}</option>}</For>
          </select>
        </label>
      </div>
      <Result answer={ranked.data} pending={ranked.isPending}>
        {(value) => (
          <div class="scroll">
            <table data-test="nearest-titles">
              <thead>
                <tr>
                  <For each={["rank", "title", "distance"]}>{(name) => <th>{name}</th>}</For>
                </tr>
              </thead>
              <tbody>
                <For each={value.titles}>
                  {(title, index) => (
                    <tr>
                      <td>{index() + 1}</td>
                      <td>{render(title)}</td>
                      <td>{distance(title)}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        )}
      </Result>
    </div>
  );
}

/** Keyset pages: the cursor is a key, so the next page starts where this ended. */
export function Pages(props: Context): JSX.Element {
  const [limit, setLimit] = createSignal(4);
  // Every cursor followed so far. The first page has none; going back pops.
  const [trail, setTrail] = createSignal<Tagged[][]>([]);
  const after = () => trail().at(-1);
  const page = createQuery(() => ({
    queryKey: ["page", props.sdk(), props.persona(), limit(), after()],
    queryFn: () => {
      const cursor = after();
      return api.page(props.sdk(), props.persona(), {
        limit: limit(),
        ...(cursor ? { after: cursor } : {}),
      });
    },
  }));

  return (
    <div class="panel">
      <h2>Pages</h2>
      <p class="why">
        Each page hands back a <b>cursor</b>: the key of its last row. The next
        page asks for rows after that key, which the server finds by seeking
        the index rather than counting past an offset — so page fifty costs
        what page one does, and a row inserted meanwhile cannot push one you
        have not seen onto the page you already read.
      </p>
      <div class="controls">
        <label class="field">
          <span>page size</span>
          <select
            value={String(limit())}
            onChange={(event) => {
              setLimit(Number(event.currentTarget.value));
              setTrail([]);
            }}
          >
            <For each={[2, 4, 8]}>{(n) => <option value={String(n)}>{n}</option>}</For>
          </select>
        </label>
        <button
          type="button"
          class="seg"
          style={{ padding: "6px 14px", cursor: "pointer" }}
          disabled={trail().length === 0}
          onClick={() => setTrail(trail().slice(0, -1))}
        >
          back
        </button>
        <button
          type="button"
          class="seg"
          style={{ padding: "6px 14px", cursor: "pointer" }}
          disabled={!(page.data?.ok && page.data.value.cursor)}
          onClick={() => {
            const answer = page.data;
            if (answer?.ok && answer.value.cursor) setTrail([...trail(), answer.value.cursor]);
          }}
          data-test="page-next"
        >
          next page
        </button>
      </div>
      <Result answer={page.data} pending={page.isPending}>
        {(value) => (
          <>
            <div class="badges">
              <span class="badge">
                page <b>{trail().length + 1}</b>
              </span>
              <span class="badge" data-tone={value.cursor ? "good" : "warn"}>
                cursor{" "}
                <b data-test="page-cursor">
                  {value.cursor ? value.cursor.map(render).join(", ") : "none — that was the end"}
                </b>
              </span>
            </div>
            <ValueTable
              columns={TABLES["books"] ?? []}
              rows={value.rows}
              empty="nothing after the last cursor"
            />
          </>
        )}
      </Result>
    </div>
  );
}

/** Decimals: a count of the smallest unit, rendered against the column's scale. */
export function Decimals(props: Context): JSX.Element {
  const rendered = createQuery(() => ({
    queryKey: ["render-decimals", props.sdk(), props.persona()],
    queryFn: () => api.renderDecimals(props.sdk(), props.persona()),
  }));

  return (
    <div class="panel">
      <h2>Money</h2>
      <p class="why">
        A decimal is stored as an integer count of its smallest unit — cents,
        for <code>books.price</code> — and the scale belongs to the column, so it
        never travels. Each client renders the number against the scale it
        declares. These are the hard cases, rendered by the{" "}
        <b>{props.sdk()}</b> client: values under one whole unit, negative ones,
        scale 0, and both ends of a 64-bit integer.
      </p>
      <Result answer={rendered.data} pending={rendered.isPending}>
        {(value) => (
          <div class="scroll">
            <table data-test="decimal-rows">
              <thead>
                <tr>
                  <For each={["stored units", "scale", "rendered"]}>{(name) => <th>{name}</th>}</For>
                </tr>
              </thead>
              <tbody>
                <For each={value.rendered}>
                  {(row) => (
                    <tr>
                      <td>{row.units}</td>
                      <td>{row.scale}</td>
                      <td>
                        <b>{row.text}</b>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        )}
      </Result>
    </div>
  );
}

/** A unique index refusing a duplicate, and admitting a fresh value. */
export function Unique(props: Context): JSX.Element {
  const [collide, setCollide] = createSignal(true);
  const [ran, setRan] = createSignal(0);
  const outcome = createQuery(() => ({
    queryKey: ["unique", props.sdk(), props.persona(), collide(), ran()],
    queryFn: () => api.unique(props.sdk(), props.persona(), collide()),
    enabled: ran() > 0,
  }));

  return (
    <div class="panel">
      <h2>Unique</h2>
      <p class="why">
        <code>authors.name</code> carries a unique index. This inserts a new
        author at a key nobody holds — once under a name that is already taken,
        once under a fresh one — then removes it again if it landed.
      </p>
      <p class="why">
        The refusal's <b>reason</b> is the part worth reading. A duplicate
        primary key and a duplicate in a unique index are both{" "}
        <code>already-exists</code>; only the reason says which rule fired, and
        here the key is free, so it can only be the index.
      </p>
      <div class="controls">
        <label class="field">
          <span>name</span>
          <select
            value={collide() ? "taken" : "fresh"}
            onChange={(event) => setCollide(event.currentTarget.value === "taken")}
            data-test="unique-name"
          >
            <option value="taken">Ursula K. Le Guin (taken)</option>
            <option value="fresh">Nobody Yet 9400 (fresh)</option>
          </select>
        </label>
        <button
          type="button"
          class="seg"
          style={{ padding: "6px 14px", cursor: "pointer" }}
          onClick={() => setRan(ran() + 1)}
          data-test="unique-run"
        >
          insert it
        </button>
      </div>
      <Show when={ran() > 0} fallback={<div class="note">not run yet</div>}>
        <Result answer={outcome.data} pending={outcome.isPending}>
          {(value) => (
            <div class="badges" data-test="unique-summary">
              <span class="badge" data-tone={value.refused ? "warn" : "good"}>
                {value.refused ? "refused" : "inserted"}{" "}
                <b>{value.refused || "the new author"}</b>
              </span>
              <Show when={value.reason}>
                <span class="badge">
                  reason <b>{value.reason}</b>
                </span>
              </Show>
              <span class="badge" data-tone={value.landed ? "good" : "warn"}>
                row <b>{value.landed ? "landed, then removed" : "never written"}</b>
              </span>
            </div>
          )}
        </Result>
      </Show>
    </div>
  );
}

/** How many reads the topology panel sends to each node to tally who served them. */
const SAMPLES = 8;

/**
 * Which node leads, and which view of the database answers a read.
 *
 * Asks each node directly, whatever the header switch says, because the
 * comparison between them is the point. Locally there is one node and the
 * Worker is not involved, so `b` answers "no such node" and the panel says so.
 */
export function Topology(props: Context): JSX.Element {
  const [ran, setRan] = createSignal(0);
  // Two only where there are two: the hosted Worker. A local adapter has one
  // node and ignores the header, so asking it twice would draw the same node
  // as if it were two.
  const hosted = SDKS.includes("edge");
  const nodes: (string | undefined)[] = hosted ? ["a", "b"] : [undefined];
  const survey = createQuery(() => ({
    queryKey: ["topology", props.sdk(), props.persona(), ran()],
    queryFn: async () =>
      Promise.all(
        nodes.map(async (node) => {
          const meta = await api.meta(props.sdk(), "app", node);
          const reads: Answer<{ servedBy: string; rows: number }>[] = [];
          // One after another rather than all at once: a burst lands on one
          // replica as easily as spread across them, and the tally is meant
          // to show the spread.
          for (let n = 0; n < SAMPLES; n++) {
            reads.push(await api.servedBy(props.sdk(), props.persona(), node));
          }
          const tally: Record<string, number> = {};
          for (const read of reads) {
            const name = read.ok ? read.value.servedBy : `refused: ${read.error.kind}`;
            tally[name] = (tally[name] ?? 0) + 1;
          }
          return { node: node ?? "this one", asked: node, meta, tally };
        }),
      ),
  }));
  const [wrote, setWrote] = createSignal<
    { node: string; asked: string | undefined; count: number } | undefined
  >();
  const write = createQuery(() => ({
    queryKey: ["topology-write", props.sdk(), props.persona(), wrote()],
    queryFn: () => api.unique(props.sdk(), props.persona(), false, wrote()?.asked),
    enabled: wrote() !== undefined,
  }));

  return (
    <div class="panel">
      <h2>Topology</h2>
      <p class="why">
        One writer, many readers. Each node holds a writer <i>or</i> follows:
        whichever started first took a lease in the bucket and writes; the
        other serves reads and refuses writes, naming the leader. Inside each,
        reads are spread over read replicas — separate readers of the same
        object storage — and the tally shows which one answered each of{" "}
        {SAMPLES} reads.
      </p>
      <div class="controls">
        <button
          type="button"
          class="seg"
          style={{ padding: "6px 14px", cursor: "pointer" }}
          onClick={() => setRan(ran() + 1)}
        >
          ask again
        </button>
      </div>
      <Show when={survey.data} fallback={<div class="spinner">asking both nodes…</div>}>
        {(rows) => (
          <div class="scroll">
            <table data-test="topology-nodes">
              <thead>
                <tr>
                  <For each={["node", "role", "reads served by", "write here"]}>
                    {(name) => <th>{name}</th>}
                  </For>
                </tr>
              </thead>
              <tbody>
                <For each={rows()}>
                  {(row) => (
                    <tr>
                      <td>
                        <b>{row.node}</b>
                      </td>
                      <td>
                        {row.meta.ok
                          ? row.meta.value.leader
                            ? "leader (writes)"
                            : "follower (reads only)"
                          : row.meta.error.message}
                      </td>
                      <td>
                        {Object.entries(row.tally)
                          .map(([name, count]) => `${name} ×${count}`)
                          .join(", ")}
                      </td>
                      <td>
                        <Show when={row.meta.ok}>
                          <button
                            type="button"
                            class="seg"
                            style={{ cursor: "pointer" }}
                            onClick={() =>
                              setWrote({
                                node: row.node,
                                asked: row.asked,
                                count: (wrote()?.count ?? 0) + 1,
                              })
                            }
                          >
                            insert a row
                          </button>
                        </Show>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        )}
      </Show>
      <Show when={wrote()}>
        {(asked) => (
          <Result answer={write.data} pending={write.isPending}>
            {(value) => (
              <div class="badges" data-test="topology-write">
                <span class="badge">
                  on node <b>{asked().node}</b>
                </span>
                <span class="badge" data-tone={value.refused ? "warn" : "good"}>
                  {value.refused ? "refused" : "written"}{" "}
                  <b>{value.refused || "and removed again"}</b>
                </span>
              </div>
            )}
          </Result>
        )}
      </Show>
    </div>
  );
}
