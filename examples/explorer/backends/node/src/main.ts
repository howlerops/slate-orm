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
  add,
  and,
  type Atomicity,
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
   * One relationship, loaded for many parents in one read.
   *
   * `sales.book_id -> books`, the demo's only foreign key: `books.author_id`
   * cannot be one, because `Author Unknown` names author 99 on purpose so the
   * outer joins have an unmatched side to show.
   *
   * Read the `parents` way it goes through `books`, which carries the row
   * policy — so a `reader` asking for the books behind a page of sales must
   * see the same gap in all three SDKs, and a client that resolved the
   * relationship itself rather than asking the server would not have one.
   */
  async related(
    session: Session,
    body: { way?: string; keys?: Record<string, unknown>[]; through?: string },
  ): Promise<unknown> {
    const parents = body.way === "parents";
    const keys = (body.keys ?? []).map(decode);
    // `through` is overridable only so the conformance corpus can name a key
    // that does not exist and compare the three refusals, which is the one
    // thing about this call the three could spell differently.
    const groups = await session.related(
      parents ? "books" : "sales",
      {
        on: "sales",
        through: body.through || "sale_book",
        way: parents ? "parents" : "children",
      },
      keys,
    );
    // A group per key the caller sent, in the caller's order, including the
    // empty ones — the shape all three clients promise.
    return { groups: groups.map((group) => group.map(encodeRow)) };
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
   * One page of `books` by keyset, and where to resume.
   *
   * The cursor comes back as a row so the three adapters encode it the way
   * they encode everything else, and so the corpus compares its *type* as well
   * as its value — a cursor arriving as a bare number would agree across three
   * clients that had all lost the same distinction.
   */
  async page(
    session: Session,
    body: {
      limit?: number;
      after?: Record<string, unknown>[];
      columns?: number[];
      sort?: { column: number; direction?: string }[];
    },
  ): Promise<unknown> {
    const query: Query = {
      table: "books",
      ...(body.limit ? { limit: body.limit } : {}),
      ...(body.after ? { after: body.after.map(decode) } : {}),
      ...(body.columns ? { columns: body.columns } : {}),
      ...(body.sort
        ? {
            sort: body.sort.map((key) => ({
              column: key.column,
              direction: key.direction === "desc" ? ("desc" as const) : ("asc" as const),
            })),
          }
        : {}),
    };
    const page = await session.page(query);
    // `null` rather than an empty list for the last page, so "there is nothing
    // after this" is one value in all three adapters, not two.
    return {
      rows: page.rows.map(encodeRow),
      cursor: page.isLast ? null : (page.cursor ?? []).map(encode),
      isLast: page.isLast,
    };
  }

  /**
   * Three tables in one request: authors, their books, those books' sales.
   *
   * Separate from `/api/join` because it is the thing worth comparing and not
   * a variation on a two-table join. A chain is not a different RPC — a
   * `JoinQuery` carries as many inputs as it is given and the kernel picks its
   * chain path past two — so what could differ between the three SDKs is how
   * each spells the *third* input's attachment: it joins back to the second,
   * and a client that attached it to the first would produce a cross join with
   * the right number of columns.
   */
  async chain(session: Session, body: { type?: string; limit?: number }): Promise<unknown> {
    const kinds: Record<string, JoinType> = {
      inner: "inner", left: "left", right: "right", full: "full",
    };
    const kind = kinds[body.type ?? ""];
    if (!kind) throw new Error(`no such join type: ${body.type}`);

    const b = newJoin();
    const authors = b.add({ table: "authors" });
    const books = b.add({
      table: "books",
      type: kind,
      on: [{ earlier: at(authors, 0), own: 1 }],
    });
    // books.id to sales.book_id: `earlier` names the *second* input, which is
    // what makes this a chain rather than two joins onto the first.
    b.add({
      table: "sales",
      type: kind,
      on: [{ earlier: at(books, 0), own: 1 }],
    });

    const joined = await session
      .join(b.query(body.limit !== undefined ? { limit: body.limit } : {}))
      .collect();
    const rows = joined.map((row) => ({
      authors: row[0] ? encodeRow(row[0]) : null,
      books: row[1] ? encodeRow(row[1]) : null,
      sales: row[2] ? encodeRow(row[2]) : null,
    }));
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
  /**
   * Seed four rows, write over them by predicate, report what came back.
   *
   * The id range is clear of the fixture and of the transaction probe at 9001.
   * Predicate writes mutate, and the runner drives all three adapters against
   * one database, so each run seeds its own rows first and the three see the
   * same four. The fixture is never touched: a case that deleted from it would
   * make every later case depend on which SDK ran first.
   *
   * Self-contained and idempotent, like the transaction probe below and for
   * the same reason: the demo, and the corpus, must give the same answer run
   * twice.
   */
  async predicateWrite(
    session: Session,
    body: { kind?: string; returning?: boolean; noSet?: boolean },
  ): Promise<unknown> {
    const first = 9100n;
    const mine = ge(0, uint(first));
    // Clean slate. A predicate delete is the tidiest way to say "whatever is
    // left from last time", and it exercises the feature on the way in.
    await session.deleteWhere({ table: "books", filter: mine });
    const rows: Value[][] = [];
    for (let n = 0n; n < 4n; n++) {
      rows.push([
        uint(first + n),
        uint(1n),
        { kind: "string", value: `Predicate ${n}` },
        { kind: "int", value: 2000n + n },
        { kind: "float", value: 3 },
        { kind: "int", value: 1767225600n },
        vector([0.1, 0.2, 0.3, 0.4]),
      ]);
    }
    await session.insert("books", ...rows);

    // Rows 9102 and 9103: year >= 2002.
    const recent = and(mine, ge(3, { kind: "int", value: 2002n }));
    const returning = Boolean(body.returning);
    let result;
    if (body.kind === "delete") {
      result = await session.deleteWhere({ table: "books", filter: recent, returning });
    } else if (body.kind === "update") {
      result = await session.updateWhere({
        table: "books",
        filter: recent,
        // rating = rating + 1, read off the row as it was.
        set: body.noSet
          ? []
          : [{ column: 4, value: add(col(4), lit({ kind: "float", value: 1 })) }],
        returning,
      });
    } else {
      throw new Error(`unknown predicate write ${String(body.kind)}`);
    }

    // How many of the four are left, which is what makes a delete's effect
    // visible rather than only its report.
    let left = 0;
    for await (const _ of session.query({ table: "books", filter: mine })) left++;
    return {
      // `Number`, because `affected` is a bigint and `JSON.stringify` refuses
      // one outright — and because the other two adapters send a JSON number,
      // so a string here would be a disagreement about the type rather than
      // about the count. Row counts are far below 2^53.
      affected: Number(result.affected),
      rows: result.rows.map(encodeRow),
      left,
    };
  }

  /**
   * Three writes, one of which collides, under the asked-for atomicity.
   *
   * The duplicate is the point: it is the operation that makes the two
   * guarantees visibly different, and `left` afterwards is how the corpus sees
   * which one happened.
   */
  async batch(session: Session, body: { atomicity?: string }): Promise<unknown> {
    const first = 9200n;
    const mine = ge(0, uint(first));
    await session.deleteWhere({ table: "books", filter: mine });
    await session.insert("books", bookRow(first + 1n, "Already There"));

    const atomicity: Atomicity =
      body.atomicity === "all-or-nothing" ? "all-or-nothing" : "independent";
    const outcomes: unknown[] = [];
    let failed = "";
    try {
      const result = await session.batch({
        atomicity,
        operations: [
          { kind: "insert", table: "books", rows: [bookRow(first, "First")] },
          { kind: "insert", table: "books", rows: [bookRow(first + 1n, "Collides")] },
          { kind: "insert", table: "books", rows: [bookRow(first + 2n, "Third")] },
        ],
      });
      for (const one of result.outcomes) {
        if (one.error) {
          outcomes.push({ kind: one.error.kind, reason: one.error.reason });
        } else {
          outcomes.push({ ok: Number(one.written.affected) });
        }
      }
    } catch (error) {
      // An atomic batch fails the call. Reported as a field rather than
      // rethrown, so the corpus compares the outcome of the two atomicities
      // rather than one being a refusal case and one not.
      if (!(error instanceof SlateError)) throw error;
      failed = error.kind;
    }

    let left = 0;
    for await (const _ of session.query({ table: "books", filter: mine })) left++;
    return { failed, outcomes, left };
  }

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
    "/api/chain": (s, b) => adapter.chain(s, b),
    "/api/page": (s, b) => adapter.page(s, b),
    "/api/related": (s, b) => adapter.related(s, b),
    "/api/batch": (s, b) => adapter.batch(s, b),
    "/api/predicate-write": (s, b) => adapter.predicateWrite(s, b),
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

/** A whole `books` row, every column in ordinal order. */
function bookRow(id: bigint, title: string): Value[] {
  return [
    uint(id),
    uint(1n),
    { kind: "string", value: title },
    { kind: "int", value: 2020n },
    { kind: "float", value: 4 },
    { kind: "int", value: 1767225600n },
    vector([0.5, 0.5, 0.5, 0.5]),
  ];
}
