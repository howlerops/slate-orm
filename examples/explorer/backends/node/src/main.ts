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
  caseWhen,
  col,
  computed0,
  compare,
  concat,
  count,
  Client,
  distance,
  div,
  extract,
  eq,
  ge,
  groupGe,
  groupKey,
  gt,
  ilike,
  isIn,
  isNotNull,
  isNull,
  int,
  inZone,
  joinComputed,
  le,
  lit,
  like,
  lower,
  lt,
  monthStart,
  mul,
  ne,
  newJoin,
  not,
  ref,
  regexpReplace,
  str,
  upper,
  vector,
  year,
  and as andOf,
  or as orOf,
  SlateError,
  uint,
  type Expr,
  type Grouping,
  type Identity,
  type JoinQuery,
  type JoinType,
  type Ordinal,
  type Query,
  type Scalar,
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

/** The body of `/api/aggregate` and `/api/explain-aggregate`. */
interface AggregateSpec {
  groupBy?: string;
  having?: { minCount: number } | null;
  sort?: string;
  direction?: string;
  limit?: number;
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

  /**
   * The embedding every `/api/nearest` request measures against.
   *
   * Fixed rather than taken from the request body, because the point is that
   * three SDKs build the same `Distance` scalar and agree on the order it
   * produces. A vector from the body would let a caller ask a question the
   * other two adapters were not asked.
   */
  static readonly QUERY_VECTOR = [0.1, 0.2, 0.3, 0.4];

  /**
   * Books ranked by cosine distance from `QUERY_VECTOR`.
   *
   * The last scalar family the three SDKs were never compared on. It is also
   * the only one whose *result* cannot be compared: a distance is an f64 and
   * the three clients format floats differently, which is why `/api/explain`
   * excludes `estimatedCost` for the same reason. So this returns the titles
   * in order and not the distances — the order is the claim, and it is total
   * because the sort breaks ties on the id.
   */
  async nearest(session: Session, body: { limit?: number }): Promise<unknown> {
    const rows = await session
      .query({
        table: "books",
        compute: [distance(col(6), lit(vector(Adapter.QUERY_VECTOR)), "cosine")],
        columns: [0, 2],
        sort: [
          // `column` is required by the interface and overridden by `ref`, so
          // it is written as the computed slot's own index rather than as a
          // placeholder — a zero there would read as "sort by the id".
          { column: 0, ref: computed0(0), direction: "asc" },
          { column: 0, direction: "asc" },
        ],
        ...(body.limit !== undefined ? { limit: body.limit } : {}),
      })
      .collect();
    return { titles: rows.map((row) => encode(row[2]!)) };
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

  /**
   * The join and grouping the contract's aggregate body names.
   *
   * Shared by `aggregate` and `explainAggregate` for the same reason the kernel
   * shares its narrowing between running a grouped read and explaining one: an
   * explanation of a *different* request is worse than none.
   */
  #buildAggregate(body: AggregateSpec): { join: JoinQuery; grouping: Grouping } {
    const b = newJoin();
    const authors = b.add({ table: "authors" });
    const books = b.add({ table: "books", on: [{ earlier: at(authors, 0), own: 1 }] });

    // `decade` is not a column at all. It used to be refused here, in all three
    // adapters, with "needs a computed column, which this demo does not
    // declare" — true of the clients rather than of the database, since the
    // kernel has had scalar expressions throughout. Hand-bucketing it here
    // would have been the adapter doing the database's job.
    //
    // `books.year / 10 * 10`, computed over the *joined* row. Integer division
    // truncates toward zero, which is what a decade means for these years.
    let key;
    let compute: Scalar[] = [];
    switch (body.groupBy) {
      case "author":
        key = at(0, 1);
        break;
      case "country":
        key = at(0, 2);
        break;
      case "decade":
        compute = [mul(div(ref(at(books, 3)), lit(int(10))), lit(int(10)))];
        key = joinComputed(0);
        break;
      // Everything below is a different *kind* of scalar rather than a
      // different column. Arithmetic was the only expression the three SDKs
      // were ever compared on, and division is the one operation every
      // language spells identically — so agreement on it proved much less
      // than it looked.
      case "shout":
        // A string function, on the *left* input. The author's *name* rather
        // than the country, because every country here is already upper case
        // — so an adapter that dropped the `upper` would have passed.
        compute = [upper(ref(at(authors, 1)))];
        key = joinComputed(0);
        break;
      case "era":
        // A conditional whose branches are strings and whose test is on an
        // integer column, so the types differ across the expression.
        compute = [
          caseWhen(
            // 1970 rather than 2000, because every book here predates 2000
            // — so the `otherwise` branch was never taken and the conditional
            // was a constant. Five fall either side of 1970.
            [{ when: compare(at(books, 3), "lt", int(1970)), then: lit(str("before 1970")) }],
            lit(str("from 1970")),
          ),
        ];
        key = joinComputed(0);
        break;
      case "tidy":
        // A regular expression over a lower-cased title: the pattern dialect
        // and the case folding both have to agree.
        compute = [regexpReplace(lower(ref(at(books, 2))), "[^a-z]+", "-")];
        key = joinComputed(0);
        break;
      case "releasedYear":
        // A calendar field over seconds since the epoch. Seven of the eleven
        // books are before 1970, so this runs on negative instants.
        compute = [year(ref(at(books, 5)))];
        key = joinComputed(0);
        break;
      case "releasedMonth":
        // A calendar *truncation*, which is not a division: a month has no
        // fixed number of seconds.
        compute = [monthStart(ref(at(books, 5)))];
        key = joinComputed(0);
        break;
      case "releasedHourNY":
        // A named timezone, resolved through the server's transition table.
        // Some of these dates are in daylight saving and some are not, so
        // this is not a constant shift.
        compute = [extract("hour", inZone("America/New_York", ref(at(books, 5))))];
        key = joinComputed(0);
        break;
      case "label":
        // Concatenation across *both* inputs, which no input's own compute
        // could express.
        compute = [
          concat(
            ref(at(authors, 2)),
            lit(str("/")),
            ref(at(books, 2)),
            lit(str("/")),
            // An i64 spliced into a string, which is where `concat` was
            // rendering Rust's `Debug` form: `1968` came out as `I64(1968)`.
            ref(at(books, 3)),
          ),
        ];
        key = joinComputed(0);
        break;
      default:
        throw new Error(`no such grouping: ${body.groupBy}`);
    }

    const column = body.sort === "key" ? groupKey(0) : agg(0);
    const direction = body.direction === "desc" ? ("desc" as const) : ("asc" as const);
    return {
      join: { ...b.query(), compute },
      grouping: {
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
      },
    };
  }

  async aggregate(session: Session, body: AggregateSpec): Promise<unknown> {
    const { join, grouping } = this.#buildAggregate(body);
    const groups = await session.aggregateJoin(join, grouping).collect();

    return {
      groups: groups.map((group) => ({
        key: encodeRow(group.key),
        ...(group.values[0] ? { count: encode(group.values[0]) } : {}),
      })),
    };
  }

  /**
   * The plan of the *grouped* read, which is not the plan of the join
   * underneath: grouping narrows each input's projection to the group keys and
   * the aggregates' columns. `decodes` is where that shows.
   */
  async explainAggregate(session: Session, body: AggregateSpec): Promise<unknown> {
    const { join, grouping } = this.#buildAggregate(body);
    const plan = await session.explainAggregateJoin(join, grouping);
    if (!plan.join) throw new Error("a grouped join explained as something other than a join");
    return {
      inputs: plan.join.inputs.map((input) => ({
        table: input.plan.table,
        access: input.plan.access,
        indexOnly: input.plan.indexOnly,
        decodes: input.plan.decodes,
        algorithm: input.algorithm,
      })),
      display: plan.display,
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
      // Every column, in ordinal order, including the two `books` grew for
      // the conformance corpus. A row on the wire is one value per column, so
      // a five-value row here is a refusal — which is how adding them was
      // caught.
      await tx.insert("books", [
        uint(probe),
        uint(1n),
        { kind: "string", value: "A Book In Flight" },
        { kind: "int", value: 2026n },
        { kind: "float", value: 5 },
        { kind: "int", value: 1767225600n },
        vector([0.4, 0.3, 0.2, 0.1]),
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
    "/api/explain-aggregate": (s, b) => adapter.explainAggregate(s, b),
    "/api/nearest": (s, b) => adapter.nearest(s, b),
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
