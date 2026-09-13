/**
 * The TypeScript adapter of the explorer demo.
 *
 * Speaks the HTTP contract in ../../CONTRACT.md against a slate head node,
 * using the TypeScript client. Two more adapters do the same through the Go and
 * Python clients, and `conformance/` requires all three to answer identically —
 * which is the first thing in this repository that compares the three clients
 * to each other rather than each to the server.
 *
 * `node:http` rather than a framework: six endpoints and no middleware, and a
 * dependency here is a dependency the demo asks a reader to install before they
 * can see anything work.
 */
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { parseArgs } from "node:util";

import {
  agg,
  at,
  count,
  Client,
  eq,
  ge,
  groupGe,
  groupKey,
  gt,
  ilike,
  isIn,
  isNotNull,
  isNull,
  le,
  like,
  lt,
  ne,
  newJoin,
  not,
  and as andOf,
  or as orOf,
  SlateError,
  uint,
  type Expr,
  type Identity,
  type JoinType,
  type Ordinal,
  type Query,
  type Session,
  type Value,
} from "@slate-orm/client";

import { decode, encode, encodeRow, formatFloat } from "./values.js";

/**
 * The demo's three personas.
 *
 * `reader` differs from `app` only in what the *database* grants it, and
 * `stranger` holds a role with no grant on the tables the UI shows. No adapter
 * enforces any of it — that is the point of the switcher.
 */
const IDENTITIES: Record<string, Identity> = {
  app: { principal: "u64:1", tenant: "u64:1", roles: ["app"] },
  reader: { principal: "u64:2", tenant: "u64:1", roles: ["reader"] },
  stranger: { principal: "u64:3", tenant: "u64:1", roles: ["stranger"] },
};

const TABLES = ["authors", "books", "sales"];

interface FilterSpec {
  op: string;
  column?: Ordinal;
  value?: Record<string, unknown>;
  values?: Record<string, unknown>[];
  pattern?: string;
  parts?: FilterSpec[];
  part?: FilterSpec;
}

/**
 * The contract's filter tree, as this client's expressions.
 *
 * A closed grammar rather than a string each adapter parses: three parsers for
 * one little language is three things to keep in agreement, and the first
 * divergence would look like a database bug.
 */
function buildFilter(spec: FilterSpec | null | undefined): Expr | undefined {
  if (!spec) return undefined;
  const column = (spec.column ?? 0) as Ordinal;
  const simple: Record<string, (c: Ordinal, v: Value) => Expr> = {
    eq, ne, lt, le, gt, ge,
  };
  const make = simple[spec.op];
  if (make) {
    if (!spec.value) throw new Error(`${spec.op} needs a value`);
    return make(column, decode(spec.value));
  }
  switch (spec.op) {
    case "like":
      return like(column, spec.pattern ?? "");
    case "ilike":
      return ilike(column, spec.pattern ?? "");
    case "isNull":
      return isNull(column);
    case "isNotNull":
      return isNotNull(column);
    case "in":
      return isIn(column, (spec.values ?? []).map(decode));
    case "and":
      return andOf(...(spec.parts ?? []).map((p) => buildFilter(p)!).filter(Boolean));
    case "or":
      return orOf(...(spec.parts ?? []).map((p) => buildFilter(p)!).filter(Boolean));
    case "not":
      return not(buildFilter(spec.part)!);
    default:
      throw new Error(`no such filter operator: ${spec.op}`);
  }
}

interface QuerySpec {
  table?: string;
  filter?: FilterSpec | null;
  sort?: { column: Ordinal; direction?: string }[];
  limit?: number | null;
  offset?: number;
  columns?: Ordinal[];
}

function buildQuery(spec: QuerySpec): Query {
  if (!spec.table || !TABLES.includes(spec.table)) {
    throw new Error(`no such table: ${spec.table}`);
  }
  const filter = buildFilter(spec.filter);
  return {
    table: spec.table,
    ...(filter ? { filter } : {}),
    ...(spec.sort
      ? {
          sort: spec.sort.map((key) => ({
            column: key.column,
            direction: key.direction === "desc" ? ("desc" as const) : ("asc" as const),
          })),
        }
      : {}),
    ...(spec.limit !== undefined && spec.limit !== null ? { limit: spec.limit } : {}),
    ...(spec.offset ? { offset: spec.offset } : {}),
    ...(spec.columns?.length ? { columns: spec.columns } : {}),
  };
}

class Adapter {
  readonly clients: Record<string, Client> = {};

  constructor(head: string) {
    for (const [name, identity] of Object.entries(IDENTITIES)) {
      this.clients[name] = Client.connect(head, identity);
    }
  }

  async meta(): Promise<unknown> {
    const status = await this.clients["app"]!.leadership();
    return { sdk: "node", leader: status.leader, tables: TABLES };
  }

  async query(session: Session, body: QuerySpec): Promise<unknown> {
    const rows = await session.query(buildQuery(body)).collect();
    return { rows: rows.map(encodeRow) };
  }

  async join(session: Session, body: { type?: string; limit?: number }): Promise<unknown> {
    const kinds: Record<string, JoinType> = {
      inner: "inner", left: "left", right: "right", full: "full",
    };
    const kind = kinds[body.type ?? ""];
    if (!kind) throw new Error(`no such join type: ${body.type}`);

    const b = newJoin();
    const authors = b.add({ table: "authors" });
    b.add({
      table: "books",
      type: kind,
      on: [{ earlier: at(authors, 0), own: 1 }],
    });

    const joined = await session
      .join(b.query(body.limit !== undefined ? { limit: body.limit } : {}))
      .collect();
    const rows = joined.map((row) => ({
      authors: row[0] ? encodeRow(row[0]) : null,
      books: row[1] ? encodeRow(row[1]) : null,
    }));
    // A join's row order is the plan's business. Sorted so the three adapters
    // are comparable and the table does not reshuffle when a hint changes the
    // algorithm.
    rows.sort((x, y) => (JSON.stringify(x) < JSON.stringify(y) ? -1 : 1));
    return { rows };
  }

  async aggregate(
    session: Session,
    body: {
      groupBy?: string;
      having?: { minCount: number } | null;
      sort?: string;
      direction?: string;
      limit?: number;
    },
  ): Promise<unknown> {
    let key;
    switch (body.groupBy) {
      case "author":
        key = at(0, 1);
        break;
      case "country":
        key = at(0, 2);
        break;
      case "decade":
        // Not a column. The kernel can compute one with a scalar expression;
        // hand-bucketing it here would be the adapter doing the database's job.
        throw new Error(
          "grouping by decade needs a computed column, which this demo does not declare",
        );
      default:
        throw new Error(`no such grouping: ${body.groupBy}`);
    }

    const b = newJoin();
    const authors = b.add({ table: "authors" });
    b.add({ table: "books", on: [{ earlier: at(authors, 0), own: 1 }] });

    const column = body.sort === "key" ? groupKey(0) : agg(0);
    const direction = body.direction === "desc" ? ("desc" as const) : ("asc" as const);
    const groups = await session
      .aggregateJoin(b.query(), {
        groupBy: [key],
        aggregates: [count()],
        ...(body.having ? { having: groupGe(agg(0), uint(body.having.minCount)) } : {}),
        // A tie-break on the key, so equal counts do not come back in whatever
        // order the hash produced — which would differ between adapters.
        sort: [
          { column, direction },
          { column: groupKey(0), direction: "asc" },
        ],
        ...(body.limit !== undefined ? { limit: body.limit } : {}),
      })
      .collect();

    return {
      groups: groups.map((group) => ({
        key: encodeRow(group.key),
        ...(group.values[0] ? { count: encode(group.values[0]) } : {}),
      })),
    };
  }

  async explain(session: Session, body: QuerySpec): Promise<unknown> {
    const plan = await session.explain(buildQuery(body));
    // `estimatedCost` is deliberately absent: a float the three clients may
    // render differently, and the contract compares text.
    return {
      table: plan.table,
      access: plan.access,
      residual: plan.residual,
      indexOnly: plan.indexOnly,
      sorts: plan.sorts,
      descending: plan.descending,
      estimatedRows: formatFloat(plan.estimatedRows),
      display: plan.display,
    };
  }

  /** The one thing a single request cannot show: a write visible only to its
   * own transaction until it commits. */
  async transaction(session: Session, body: { commit?: boolean }): Promise<unknown> {
    const probe = 9001n;
    try {
      await session.delete("books", [uint(probe)]);
    } catch (error) {
      if (!(error instanceof SlateError) || error.kind !== "not-found") throw error;
    }

    const tx = await session.begin();
    let inside = false;
    try {
      await tx.insert("books", [
        uint(probe),
        uint(1n),
        { kind: "string", value: "A Book In Flight" },
        { kind: "int", value: 2026n },
        { kind: "float", value: 5 },
      ]);
      inside = (await tx.get("books", [uint(probe)])) !== undefined;
      if (body.commit) await tx.commit();
      else await tx.rollback();
    } finally {
      await tx.rollback();
    }

    const after = (await session.get("books", [uint(probe)])) !== undefined;
    // Leave nothing behind, so a committed run and a rolled-back run start the
    // same way and the demo is idempotent.
    if (after) await session.delete("books", [uint(probe)]);
    return { visibleInside: inside, visibleAfter: after };
  }
}

/** The contract's spelling of an error kind. */
function kindName(error: SlateError): string {
  return error.kind;
}

async function readBody(request: IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) chunks.push(chunk as Buffer);
  const raw = Buffer.concat(chunks).toString("utf8");
  return raw ? (JSON.parse(raw) as Record<string, unknown>) : {};
}

function send(response: ServerResponse, status: number, body: unknown): void {
  const payload = JSON.stringify(body);
  response.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(payload),
    "access-control-allow-origin": "*",
    "access-control-allow-headers": "content-type, x-demo-identity",
  });
  response.end(payload);
}

async function main(): Promise<void> {
  const { values } = parseArgs({
    options: {
      head: { type: "string", default: "127.0.0.1:7421" },
      listen: { type: "string", default: "127.0.0.1:7432" },
    },
    allowPositionals: true,
  });

  const adapter = new Adapter(values.head!);
  const routes: Record<string, (s: Session, b: never) => Promise<unknown>> = {
    "/api/meta": () => adapter.meta(),
    "/api/query": (s, b) => adapter.query(s, b),
    "/api/join": (s, b) => adapter.join(s, b),
    "/api/aggregate": (s, b) => adapter.aggregate(s, b),
    "/api/explain": (s, b) => adapter.explain(s, b),
    "/api/transaction": (s, b) => adapter.transaction(s, b),
  };

  const server = createServer((request, response) => {
    void (async () => {
      if (request.method === "OPTIONS") {
        send(response, 204, {});
        return;
      }
      const path = (request.url ?? "").split("?")[0] ?? "";
      const route = routes[path];
      if (!route) {
        send(response, 404, { error: { kind: "not-found", message: "no such endpoint" } });
        return;
      }
      const persona = (request.headers["x-demo-identity"] as string) || "app";
      const client = adapter.clients[persona];
      if (!client) {
        send(response, 400, {
          error: { kind: "invalid-request", message: `no such identity: "${persona}"` },
        });
        return;
      }
      try {
        const body = await readBody(request);
        send(response, 200, await route(client.session(), body as never));
      } catch (error) {
        if (error instanceof SlateError) {
          // A slate error keeps its kind; the kinds are spelled the same in all
          // three adapters so the conformance runner compares them.
          send(response, 200, {
            error: { kind: kindName(error), message: error.message.replace(/^[a-z-]+: /, "") },
          });
          return;
        }
        send(response, 400, {
          error: { kind: "adapter", message: (error as Error).message },
        });
      }
    })();
  });

  const [host, port] = values.listen!.split(":");
  server.listen(Number(port), host, () => {
    // The same handshake the head node uses, so `run.sh` can wait for a line
    // rather than poll a port.
    console.log(`LISTENING ${values.listen}`);
  });
}

void main();
