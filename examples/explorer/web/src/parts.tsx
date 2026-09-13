/** Small pieces shared by the panels. */
import { For, Show, type JSX } from "solid-js";

import { kindOf, render, type Answer, type SlateFailure, type Tagged } from "./api";

/** A segmented control, which is what every switcher in this UI is. */
export function Segmented<T extends string>(props: {
  label: string;
  value: T;
  options: readonly T[];
  onChange: (value: T) => void;
}): JSX.Element {
  return (
    <div class="group">
      <label>{props.label}</label>
      <div class="seg">
        <For each={props.options}>
          {(option) => (
            <button
              type="button"
              data-on={option === props.value}
              onClick={() => props.onChange(option)}
            >
              {option}
            </button>
          )}
        </For>
      </div>
    </div>
  );
}

/**
 * A refusal, shown as a result rather than as a crash.
 *
 * "Nothing matched" and "you may not ask" are different answers, and a UI that
 * renders a denied query as an empty table teaches the wrong thing about what
 * the database is doing.
 */
export function Refusal(props: { error: SlateFailure }): JSX.Element {
  const why = (): string | undefined => {
    switch (props.error.kind) {
      case "permission-denied":
        return "The database refused this, not the adapter. Row-level security and RBAC are enforced inside the kernel, so every SDK gets the same answer.";
      case "not-found":
        return "The table or row does not exist for this caller.";
      case "invalid-request":
        return "The server would never accept this request as sent, so retrying it unchanged cannot help.";
      case "conflict":
        return "A write lost a race. Retryable.";
      default:
        return undefined;
    }
  };
  return (
    <div class="error">
      <div class="kind">{props.error.kind}</div>
      <div class="message mono">{props.error.message}</div>
      <Show when={why()}>
        <div class="why">{why()}</div>
      </Show>
    </div>
  );
}

/** Loading, refused, or the thing itself. */
export function Result<T>(props: {
  answer: Answer<T> | undefined;
  pending: boolean;
  children: (value: T) => JSX.Element;
}): JSX.Element {
  return (
    <Show when={!props.pending} fallback={<div class="spinner">asking…</div>}>
      <Show when={props.answer} keyed>
        {(answer) =>
          answer.ok ? props.children(answer.value) : <Refusal error={answer.error} />
        }
      </Show>
    </Show>
  );
}

/**
 * A table of tagged values.
 *
 * The header carries each column's *wire type* alongside its name. That is not
 * decoration: an `i64` and a `u64` of the same magnitude are different values
 * to this database, and a grid that renders both as `1` hides the single most
 * surprising thing about the value model.
 */
export function ValueTable(props: {
  columns: string[];
  rows: (Tagged | undefined)[][];
  empty?: string;
}): JSX.Element {
  return (
    <Show
      when={props.rows.length > 0}
      fallback={<div class="note">{props.empty ?? "no rows matched"}</div>}
    >
      <div class="scroll">
        <table>
          <thead>
            <tr>
              <For each={props.columns}>
                {(name, index) => (
                  <th>
                    {name}{" "}
                    <span class="kind">{kindOf(props.rows[0]?.[index()])}</span>
                  </th>
                )}
              </For>
            </tr>
          </thead>
          <tbody>
            <For each={props.rows}>
              {(row) => (
                <tr>
                  <For each={props.columns}>
                    {(_name, index) => (
                      <td class={row[index()] ? "" : "absent"}>
                        {row[index()] ? render(row[index()]) : "—"}
                      </td>
                    )}
                  </For>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </Show>
  );
}

/**
 * A bar chart, drawn with divs.
 *
 * No charting library: the demo draws one horizontal bar per group, and a
 * dependency for that would be a dependency a reader has to trust before they
 * can see a count. The data comes from a grouped join, which is the part worth
 * looking at.
 */
export function Bars(props: {
  rows: { label: string; value: number }[];
}): JSX.Element {
  const largest = () => Math.max(1, ...props.rows.map((row) => row.value));
  return (
    <div class="chart">
      <For each={props.rows}>
        {(row) => (
          <div class="row">
            <div class="label" title={row.label}>
              {row.label}
            </div>
            <div class="track">
              <div class="fill" style={{ width: `${(row.value / largest()) * 100}%` }} />
            </div>
            <div class="value mono">{row.value}</div>
          </div>
        )}
      </For>
    </div>
  );
}
