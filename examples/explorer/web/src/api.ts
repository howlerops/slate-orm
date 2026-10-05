/**
 * The client half of ../../CONTRACT.md.
 *
 * One module, because every adapter speaks the same contract: switching SDK is
 * a change of base URL and nothing else. If this file ever needs a branch on
 * which adapter is selected, the contract has been broken and
 * `conformance/conformance.py` will say so before the UI does.
 */

import { CATALOG_TABLES, CATALOG_VIEWS } from "./catalog.js";

export type Sdk = "go" | "node" | "python" | "edge";

/** Where each adapter is, when nobody says otherwise: the ports `run.sh` uses
 * for its interactive modes, so opening the UI needs no configuration. */
export const DEFAULT_ADAPTERS: Record<Sdk, string> = {
  go: "http://127.0.0.1:7431",
  node: "http://127.0.0.1:7432",
  python: "http://127.0.0.1:7433",
  // The Worker under `wrangler dev`, which `run.sh` does not start for the UI.
  // Only `hostedFrom` ever selects it, and then at the page's own origin.
  edge: "http://127.0.0.1:8787",
};

/**
 * The adapter URLs, from the environment if it names them.
 *
 * Taking an env record rather than reading `import.meta.env` directly is what
 * makes this testable: `import.meta.env` exists only under Vite, so a unit test
 * running on plain node would otherwise be testing nothing. The exported
 * constant below passes the real one, or `{}` where there is none.
 *
 * A blank value is treated as absent. An env var set to the empty string is
 * what a shell produces from an unset variable it expanded anyway, and pointing
 * the UI at `""` — which resolves against the page's own origin — is never what
 * anybody meant.
 */
export function adaptersFrom(env: Record<string, string | undefined>): Record<Sdk, string> {
  const named: Record<Sdk, string | undefined> = {
    go: env["VITE_GO_URL"],
    node: env["VITE_NODE_URL"],
    python: env["VITE_PYTHON_URL"],
    edge: env["VITE_EDGE_URL"],
  };
  return {
    go: named.go?.trim() || DEFAULT_ADAPTERS.go,
    node: named.node?.trim() || DEFAULT_ADAPTERS.node,
    python: named.python?.trim() || DEFAULT_ADAPTERS.python,
    edge: named.edge?.trim() || DEFAULT_ADAPTERS.edge,
  };
}

/**
 * Which adapters this build of the page can reach, and where.
 *
 * Two shapes. **Local**, the default: the three SDK adapters on the ports
 * `run.sh` uses, with the switch between them that the demo exists to show.
 * **Hosted** (`VITE_HOSTED=edge`): the page is served by the Cloudflare Worker
 * (`examples/edge`), which is the one adapter there is, at the page's own
 * origin. There is nothing to switch between, so the switch is not drawn.
 *
 * The hosted base is `""` on purpose — a path resolved against the page — and
 * is the one place an empty base is meant. `adaptersFrom` refuses a blank URL
 * because nobody *setting* one means that; here it is not set, it is chosen.
 */
export function hostedFrom(env: Record<string, string | undefined>): {
  sdks: readonly Sdk[];
  adapters: Record<Sdk, string>;
} {
  if (env["VITE_HOSTED"]?.trim() === "edge") {
    return { sdks: ["edge"], adapters: { ...adaptersFrom(env), edge: "" } };
  }
  return { sdks: ["go", "node", "python"], adapters: adaptersFrom(env) };
}

const HOSTING = hostedFrom(
  (import.meta as unknown as { env?: Record<string, string | undefined> }).env ?? {},
);

export const ADAPTERS: Record<Sdk, string> = HOSTING.adapters;

/** The SDKs the switch offers: three locally, the Worker alone when hosted. */
export const SDKS: readonly Sdk[] = HOSTING.sdks;

export type Persona = "app" | "reader" | "stranger" | "analyst";

/**
 * Which head node the hosted Worker forwards to, sent as `x-demo-node`.
 *
 * Module state rather than an argument to every call, because it is not part
 * of any question the panels ask: it is where the question goes. The switch
 * that sets it clears the query cache, since no panel's query key names it.
 * Locally there is one node and the header is never sent.
 */
let headNode: string | undefined;

export function setHeadNode(name: string | undefined): void {
  headNode = name;
}

/**
 * A value as the contract carries it: tagged, with 64-bit integers as strings.
 *
 * The strings are not a quirk of the wire — `JSON.parse` rounds integers above
 * 2^53, and a primary key is where that shows up latest and hurts most. The
 * table renders them as they arrived.
 */
export type Tagged =
  | { null: true }
  | { bool: boolean }
  | { str: string }
  | { i64: string }
  | { u64: string }
  | { f64: string }
  | { bytes: string }
  | { uuid: string }
  // Stored units as text, not the rendered amount: the scale belongs to the
  // column and the wire carries a value. `1250` at scale 2 is 12.50.
  | { decimal: string }
  // Elements pre-formatted, because the three adapters' float formatters do
  // not agree on the last digit and the conformance runner compares text.
  | { vector: string[] }
  // Each element tagged in turn, so a list of `"1"` and a list of `1` stay
  // different — the confusion tagging exists to stop, one level down.
  | { array: Tagged[] };

export interface SlateFailure {
  kind: string;
  message: string;
}

/** Every endpoint answers either a body or a refusal, and a refusal is not an
 * exception: "you may not ask" is a result the UI shows deliberately. */
export type Answer<T> = { ok: true; value: T } | { ok: false; error: SlateFailure };

export interface QuerySpec {
  table: string;
  /**
   * Reach rows a soft delete retired.
   *
   * A privileged read: it needs the `read_deleted` action, which the demo's
   * `reader` persona does not hold. That refusal is the interesting half —
   * without a control for this, the flag is one a client can set and a column
   * it cannot find.
   */
  includeDeleted?: boolean;
  filter?: unknown;
  sort?: { column: number; direction: "asc" | "desc" }[];
  limit?: number | null;
  offset?: number;
  columns?: number[];
}

export interface Plan {
  table: string;
  access: string;
  residual: string;
  indexOnly: boolean;
  sorts: boolean;
  descending: boolean;
  estimatedRows: string;
  display: string;
}

/** One input's plan inside a grouped read's explanation. */
export interface GroupedInputPlan {
  table: string;
  access: string;
  indexOnly: boolean;
  /** The columns this input decodes — the point of the panel. */
  decodes: number[];
  algorithm: string;
}

export interface GroupedPlan {
  inputs: GroupedInputPlan[];
  display: string;
}

export interface JoinedRow {
  authors: Tagged[] | null;
  books: Tagged[] | null;
}

export interface GroupRow {
  key: Tagged[];
  count?: Tagged;
  /**
   * `SUM(books.price)` for the group, as a decimal.
   *
   * Beside the count rather than instead of it: the chart draws the count, and
   * the sum is here because it is the one aggregate in this contract that
   * returns money — which is what makes it worth comparing across three
   * clients, and which no chart of counts would show.
   */
  total?: Tagged;
}

/** What a predicate write reports. */
export interface PredicateWrite {
  /** How many rows the predicate matched and the write touched. */
  affected: number;
  /** The rows themselves, when `returning` asked for them. Empty otherwise. */
  rows: Tagged[][];
  /** How many of the handler's four rows survive, so a delete's *effect* is
   *  visible and not only its report. */
  left: number;
}

/** What the restore handler reports, at three points rather than one.
 *
 *  Three because "it is live now" is also what a handler that quietly inserted
 *  a fresh row at the same key would say. The columns carried through are what
 *  separate a restore from a replacement, which is why they are in the answer
 *  and on the screen.
 */
export interface Restore {
  /** The row was retired before any of this — the premise, checked. */
  retired_before: boolean;
  /** Ids an *ordinary* read returned while it was retired: none. */
  hidden_while_retired: number[];
  /** Ids an ordinary read returns now. */
  visible_after: number[];
  /** Whether each of those still carries a stamp: no. */
  retired_after: boolean[];
  /** The columns the restore had to carry through, unchanged. */
  status_after: string[];
  book_id_after: number[];
}

/** One operation's outcome inside an independent batch. */
export type BatchOne = { ok: number } | { kind: string; reason: string };

/** What a batch reports under one atomicity. */
export interface BatchOutcome {
  /** The error kind when the whole call failed, which only `all-or-nothing`
   *  can do. Null when the request itself succeeded. */
  failed: string | null;
  /** One entry per operation — empty under `all-or-nothing`, which has no
   *  per-operation outcome to report because they all landed or none did. */
  outcomes: BatchOne[];
  /** Rows surviving afterwards: the difference the two atomicities make. */
  left: number;
}

/** One node of a relationship path: a row, and the rows below it. */
/** What a full-text search returned, and which access path answered it. */
export interface SearchAnswer {
  rows: Tagged[][];
  /**
   * The plan's access path, or `null` for a caller without the `explain`
   * grant. Absent rather than a refusal: EXPLAIN is privileged because a plan
   * is costed against statistics covering rows a policy hides, and refusing
   * the whole search over a diagnostic would make full-text the one feature a
   * restricted reader cannot use at all.
   */
  access: string | null;
}

/** A conditional update's outcome: what the server said, and what is stored. */
export interface ConditionalUpdate {
  /** The refusal's kind, or `""` when the update landed. */
  refused: string;
  /** The price as stored afterwards, as units with no scale. */
  price: Tagged;
  /** The same value rendered at the scale the adapter declares. */
  rendered: string;
}

/** A conditional delete's outcome. Three answers, not two. */
export interface ConditionalDelete {
  /** The refusal's kind, or `""` when the delete landed. */
  refused: string;
  /** Rows removed. Zero when refused. */
  affected: number;
  /** Whether the row is still in the table, which is what the table says. */
  left: boolean;
}

export interface PathNode {
  row: Tagged[];
  related: Tagged[][];
}

/**
 * Both shapes a path can be read as, from one request.
 *
 * Both are indexed **by key**: the outer array has one entry per key the
 * caller passed, in the order it passed them. That is what makes a path
 * batched rather than a loop — one request, a grouped answer.
 */
export interface PathAnswer {
  /** Every level kept, which is `load_nested`. */
  trees: PathNode[][];
  /** The far rows only, which is `load_related_through`. */
  through: Tagged[][][];
}

/** One row of a three-table chain: a `null` side is an outer join's gap. */
export interface ChainedRow {
  authors: Tagged[] | null;
  books: Tagged[] | null;
  sales: Tagged[] | null;
}

export interface WindowSpec {
  function: "rowNumber" | "rank" | "denseRank" | "lag" | "lead" | "sum" | "count";
  partition: boolean;
  running: boolean;
  limit?: number;
}

/** A row and its window value, kept apart as the wire keeps them. */
export interface WindowedRow {
  row: Tagged[];
  windowed: Tagged[];
}

/** One keyset page: the cursor is the last row's key, `null` when provably done. */
export interface PageAnswer {
  rows: Tagged[][];
  cursor: Tagged[] | null;
  isLast: boolean;
}

export interface RenderedDecimal {
  units: string;
  scale: number;
  text: string;
}

/** What `/api/unique` reports: the refusal, its reason, and what the table says. */
export interface UniqueOutcome {
  refused: string;
  reason: string;
  landed: boolean;
  /** Whether the row survived the clean-up. Always false; see CONTRACT.md. */
  left: boolean;
}

async function call<T>(
  sdk: Sdk,
  path: string,
  body: unknown,
  persona: Persona,
  node: string | undefined = headNode,
): Promise<Answer<T>> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    "X-Demo-Identity": persona,
  };
  if (node !== undefined) headers["X-Demo-Node"] = node;
  const response = await fetch(`${ADAPTERS[sdk]}${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers,
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const parsed = (await response.json()) as Record<string, unknown>;
  if (parsed && typeof parsed === "object" && "error" in parsed) {
    return { ok: false, error: parsed["error"] as SlateFailure };
  }
  return { ok: true, value: parsed as T };
}

export const api = {
  // `node` names a head node outright, for the one panel that compares them;
  // every other call goes wherever the switch says.
  meta: (sdk: Sdk, persona: Persona, node?: string) =>
    call<{ sdk: string; leader: boolean; tables: string[] }>(
      sdk,
      "/api/meta",
      undefined,
      persona,
      node ?? headNode,
    ),

  query: (sdk: Sdk, persona: Persona, spec: QuerySpec) =>
    call<{ rows: Tagged[][] }>(sdk, "/api/query", spec, persona),

  join: (sdk: Sdk, persona: Persona, spec: { type: string; limit?: number }) =>
    call<{ rows: JoinedRow[] }>(sdk, "/api/join", spec, persona),

  aggregate: (
    sdk: Sdk,
    persona: Persona,
    spec: {
      groupBy: string;
      having?: { minCount: number } | null;
      sort?: string;
      direction?: string;
      limit?: number;
    },
  ) => call<{ groups: GroupRow[] }>(sdk, "/api/aggregate", spec, persona),

  explain: (sdk: Sdk, persona: Persona, spec: QuerySpec) =>
    call<Plan>(sdk, "/api/explain", spec, persona),

  explainAggregate: (
    sdk: Sdk,
    persona: Persona,
    spec: {
      groupBy: string;
      having?: { minCount: number } | null;
      sort?: string;
      direction?: string;
      limit?: number;
    },
  ) => call<GroupedPlan>(sdk, "/api/explain-aggregate", spec, persona),

  predicateWrite: (
    sdk: Sdk,
    persona: Persona,
    spec: { kind: "delete" | "update"; returning: boolean },
  ) => call<PredicateWrite>(sdk, "/api/predicate-write", spec, persona),

  // No arguments, and no separate endpoint for the mistake beside it. The
  // panel shows the path that works; `/api/restore-unchanged` exists for the
  // conformance runner, which compares refusals across the three SDKs and is
  // the right place for one.
  restore: (sdk: Sdk, persona: Persona) =>
    call<Restore>(sdk, "/api/restore", {}, persona),

  batch: (sdk: Sdk, persona: Persona, atomicity: "independent" | "all-or-nothing") =>
    call<BatchOutcome>(sdk, "/api/batch", { atomicity }, persona),

  path: (sdk: Sdk, persona: Persona, keys: Tagged[]) =>
    call<PathAnswer>(sdk, "/api/path", { keys }, persona),

  search: (
    sdk: Sdk,
    persona: Persona,
    spec: { text: string; path: "index" | "scan"; limit?: number },
  ) => call<SearchAnswer>(sdk, "/api/search", spec, persona),

  conditionalUpdate: (sdk: Sdk, persona: Persona, stale: boolean) =>
    call<ConditionalUpdate>(sdk, "/api/conditional-update", { stale }, persona),

  conditionalDelete: (
    sdk: Sdk,
    persona: Persona,
    spec: { stale: boolean; gone: boolean },
  ) => call<ConditionalDelete>(sdk, "/api/conditional-delete", spec, persona),

  chain: (sdk: Sdk, persona: Persona, spec: { type: string; limit?: number }) =>
    call<{ rows: ChainedRow[] }>(sdk, "/api/chain", spec, persona),

  window: (sdk: Sdk, persona: Persona, spec: WindowSpec) =>
    call<{ rows: WindowedRow[] }>(sdk, "/api/window", spec, persona),

  nearest: (sdk: Sdk, persona: Persona, limit: number) =>
    call<{ titles: Tagged[] }>(sdk, "/api/nearest", { limit }, persona),

  page: (sdk: Sdk, persona: Persona, spec: { limit: number; after?: Tagged[] }) =>
    call<PageAnswer>(sdk, "/api/page", spec, persona),

  renderDecimals: (sdk: Sdk, persona: Persona) =>
    call<{ rendered: RenderedDecimal[] }>(sdk, "/api/render-decimals", {}, persona),

  unique: (sdk: Sdk, persona: Persona, collide: boolean, node?: string) =>
    call<UniqueOutcome>(sdk, "/api/unique", { collide }, persona, node ?? headNode),

  servedBy: (sdk: Sdk, persona: Persona, node?: string) =>
    call<{ servedBy: string; rows: number }>(
      sdk,
      "/api/served-by",
      {},
      persona,
      node ?? headNode,
    ),

  transaction: (sdk: Sdk, persona: Persona, commit: boolean) =>
    call<{ visibleInside: boolean; visibleAfter: boolean }>(
      sdk,
      "/api/transaction",
      { commit },
      persona,
    ),
};

/** A tagged value as text, with its type kept visible.
 *
 * The type is not decoration: an `i64` and a `u64` of the same magnitude are
 * different values to this database, and a table that renders both as `1`
 * hides the single most confusing thing about the value model.
 */
/**
 * `books.price`'s declared scale.
 *
 * Here rather than in `catalog.ts` because the generated catalog carries
 * column *names* and nothing else — a browser app labels columns, and the
 * scale is a property of the type. It is a literal for the same reason the
 * three adapters each hold their own `2`: a decimal on the wire is a count of
 * the column's smallest unit and the scale never travels, so somewhere has to
 * know it locally, and this is the demo's somewhere.
 */
const PRICE_SCALE = 2;

/**
 * A decimal's units rendered against a scale, as the three clients do it.
 *
 * A fourth implementation of `unitsToString`, deliberately: the browser is not
 * a slate client and importing one to format a number would pull a gRPC stack
 * into a page that speaks HTTP. It is nine lines, and the conformance runner's
 * decimal-rendering endpoint holds the three that matter to each other.
 *
 * That endpoint's path is not spelled here on purpose:
 * `scripts/check_demo_surface.py` greps every `/api/…` under this directory
 * and reads one in a comment as the UI calling it, which is generous by
 * design and wrong here.
 */
export function money(units: string | undefined, scale = PRICE_SCALE): string {
  if (units === undefined) return "";
  const value = BigInt(units);
  const negative = value < 0n;
  const magnitude = negative ? -value : value;
  const divisor = 10n ** BigInt(scale);
  const whole = magnitude / divisor;
  const part = (magnitude % divisor).toString().padStart(scale, "0");
  return `${negative ? "-" : ""}${whole}.${part}`;
}

export function render(value: Tagged | undefined): string {
  if (!value) return "";
  if ("null" in value) return "∅";
  if ("bool" in value) return String(value.bool);
  if ("str" in value) return value.str;
  if ("i64" in value) return value.i64;
  if ("u64" in value) return value.u64;
  if ("f64" in value) return value.f64;
  if ("bytes" in value) return `0x${value.bytes}`;
  if ("uuid" in value) return value.uuid;
  if ("decimal" in value) return value.decimal;
  if ("vector" in value) return `[${value.vector.join(", ")}]`;
  if ("array" in value) return `[${value.array.map((e) => render(e)).join(", ")}]`;
  return unhandled(value);
}

/** Compile-time proof that `render` covers every `Tagged` arm.
 *
 * A parameter of type `never` only accepts a value TypeScript has narrowed to
 * nothing, so this call type-checks exactly while the union is fully handled:
 * a new arm on `Tagged` breaks the build here rather than rendering as `?` in
 * a column nobody looks at twice. Four arms did that for as long as they
 * existed — `books` carries a vector and a decimal, and both showed `?`.
 *
 * It still returns something, because a *runtime* value outside the union is
 * possible however good the type is: a server sending a tag this build has
 * never heard of. That is the case `?` is for, and the only one left.
 */
function unhandled(_value: never): string {
  return "?";
}

/** The wire type of a tagged value, for the column headers. */
export function kindOf(value: Tagged | undefined): string {
  if (!value) return "";
  return Object.keys(value)[0] ?? "";
}

/** Tables the catalog carries that the UI deliberately does not show, and why.
 *
 * The `EXPECTED_REFUSALS` idiom this repository uses elsewhere: a list you are
 * forced to edit is a list that stays true. A table added to the catalog fails
 * `test/api.test.ts` until somebody either lets the UI show it or says here
 * why it is not there — the decision made once, rather than never.
 *
 * In `src/` rather than in the test, now that the UI *filters* by it rather
 * than being compared against it. The list is a UI decision; a test is not the
 * place a UI decision lives.
 */
export const NOT_IN_THE_UI: Record<string, string> = {
  // Empty, and kept rather than deleted: the mechanism is what stops the next
  // table being added to the catalog and quietly never shown. `posts` was the
  // one entry — seeded by the Go adapter now, and rendered by `render`'s
  // `array` arm, which the compiler requires because `Tagged` is exhaustive.
};

const shown = <T,>(all: Record<string, T>): Record<string, T> =>
  Object.fromEntries(Object.entries(all).filter(([name]) => !(name in NOT_IN_THE_UI)));

/** The demo's tables, as the UI needs to label them.
 *
 * Derived from `catalog.ts`, which `scripts/codegen.py` writes from
 * `slate-serverd --print-schema` and CI re-checks with `--check`. It used to be
 * a hand-written literal guarded by a test that re-parsed `head.toml` with a
 * regex — which worked, and was the *second implementation of resolution* the
 * generator's own docstring warns about: ordinals come from declaration order,
 * a primary key is named and resolved, a decimal's scale is validated, and
 * thirty lines of regex know none of it. It agreed because this schema is
 * simple.
 *
 * Unlike every other copy of the schema in this repository, **this one never
 * reaches the server**. The adapters' declarations carry a fingerprint and are
 * refused when they disagree; these are column *headers*, they stay in the
 * browser, and nothing hashes them. A stale entry here is a wrong label over a
 * right value, which is the quietest kind of wrong there is — and is exactly
 * why it is generated now rather than checked.
 */
export const TABLES: Record<string, string[]> = shown(CATALOG_TABLES);

/** The demo's views, as name -> the columns a read through one returns.
 *
 * Each maps to its **base table's** column list rather than to a list of its
 * own, and that is the design rather than a shortcut: `docs/views.md` refuses a
 * projection in a view, so a view's ordinals *are* its base table's. A view
 * that could narrow columns would need a list here, and a second list is what
 * drifts. `catalog.ts` builds them that way, sharing the array object.
 *
 * Separate from `TABLES` because a view is not a table. `/api/meta` reports
 * the two separately for the same reason, and only `/api/query` accepts a view
 * — the plan panel asks for one anyway, deliberately, so the refusal is
 * visible rather than described.
 */
export const VIEWS: Record<string, string[]> = shown(CATALOG_VIEWS);