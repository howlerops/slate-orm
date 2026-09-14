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

export interface JoinedRow {
  authors: Tagged[] | null;
  books: Tagged[] | null;
}

export interface GroupRow {
  key: Tagged[];
  count?: Tagged;
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
  books: ["id", "author_id", "title", "year", "rating"],
  sales: ["id", "book_id", "units"],
};
