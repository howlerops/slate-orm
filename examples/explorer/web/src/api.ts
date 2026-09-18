/**
 * The client half of ../../CONTRACT.md.
 *
 * One module, because every adapter speaks the same contract: switching SDK is
 * a change of base URL and nothing else. If this file ever needs a branch on
 * which adapter is selected, the contract has been broken and
 * `conformance/conformance.py` will say so before the UI does.
 */

export type Sdk = "go" | "node" | "python";

/** Where each adapter is, when nobody says otherwise: the ports `run.sh` uses
 * for its interactive modes, so opening the UI needs no configuration. */
export const DEFAULT_ADAPTERS: Record<Sdk, string> = {
  go: "http://127.0.0.1:7431",
  node: "http://127.0.0.1:7432",
  python: "http://127.0.0.1:7433",
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
  };
  return {
    go: named.go?.trim() || DEFAULT_ADAPTERS.go,
    node: named.node?.trim() || DEFAULT_ADAPTERS.node,
    python: named.python?.trim() || DEFAULT_ADAPTERS.python,
  };
}

export const ADAPTERS: Record<Sdk, string> = adaptersFrom(
  (import.meta as unknown as { env?: Record<string, string | undefined> }).env ?? {},
);
export type Persona = "app" | "reader" | "stranger";

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
  | { bytes: string };

export interface SlateFailure {
  kind: string;
  message: string;
}

/** Every endpoint answers either a body or a refusal, and a refusal is not an
 * exception: "you may not ask" is a result the UI shows deliberately. */
export type Answer<T> = { ok: true; value: T } | { ok: false; error: SlateFailure };

export interface QuerySpec {
  table: string;
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

async function call<T>(
  sdk: Sdk,
  path: string,
  body: unknown,
  persona: Persona,
): Promise<Answer<T>> {
  const response = await fetch(`${ADAPTERS[sdk]}${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: { "Content-Type": "application/json", "X-Demo-Identity": persona },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const parsed = (await response.json()) as Record<string, unknown>;
  if (parsed && typeof parsed === "object" && "error" in parsed) {
    return { ok: false, error: parsed["error"] as SlateFailure };
  }
  return { ok: true, value: parsed as T };
}

export const api = {
  meta: (sdk: Sdk, persona: Persona) =>
    call<{ sdk: string; leader: boolean; tables: string[] }>(sdk, "/api/meta", undefined, persona),

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

  batch: (sdk: Sdk, persona: Persona, atomicity: "independent" | "all-or-nothing") =>
    call<BatchOutcome>(sdk, "/api/batch", { atomicity }, persona),

  path: (sdk: Sdk, persona: Persona, keys: Tagged[]) =>
    call<PathAnswer>(sdk, "/api/path", { keys }, persona),

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
export function render(value: Tagged | undefined): string {
  if (!value) return "";
  if ("null" in value) return "∅";
  if ("bool" in value) return String(value.bool);
  if ("str" in value) return value.str;
  if ("i64" in value) return value.i64;
  if ("u64" in value) return value.u64;
  if ("f64" in value) return value.f64;
  if ("bytes" in value) return `0x${value.bytes}`;
  return "?";
}

/** The wire type of a tagged value, for the column headers. */
export function kindOf(value: Tagged | undefined): string {
  if (!value) return "";
  return Object.keys(value)[0] ?? "";
}

/** The demo's tables, as the UI needs to label them.
 *
 * A copy of the schema, like every client holds. The server checks it on every
 * request and refuses a declaration that disagrees, so this cannot drift
 * silently — it drifts loudly, on the first query.
 */
export const TABLES: Record<string, string[]> = {
  authors: ["id", "name", "country", "born"],
  books: ["id", "author_id", "title", "year", "rating", "released", "embedding"],
  sales: ["id", "book_id", "units"],
  editions: ["id", "book_id", "format"],
};
