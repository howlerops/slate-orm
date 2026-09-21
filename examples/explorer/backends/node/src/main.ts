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
  type AccessHint,
  type Atomicity,
  col,
  computed0,
  compare,
  concat,
  contains,
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
  nullValue,
  str,
  sub,
  units,
  unitsToString,
  upper,
  vector,
  year,
  rowNumber,
  rank,
  denseRank,
  lag,
  lead,
  aggregateOver,
  over,
  sumOf,
  key0,
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
  type Column as ColumnRef,
  type Query,
  type Scalar,
  type Session,
  type Step,
  type Value,
  type Window,
  answers,
  usingIndex,
  usingTableScan,
} from "@slate-orm/client";

import {
  EditionsForeignKeys,
  SalesForeignKeys,
  TABLES as CATALOG,
  decodeAuthors,
  decodeBooks,
  decodeEditions,
  decodeSales,
  decodeShipments,
  encodeBooks,
  encodeShipments,
  isRetiredShipments,
  restoredShipments,
} from "./schema.js";
import type { Shipments } from "./schema.js";

// The shipment the restore handlers own.
//
// Below the purge handler's ids on purpose. The purge case lists what survives
// at `id >= 8401`, so a row this handler left behind there would change that
// case's answer depending on which ran first — the ordering bug that case's own
// comment records having been bitten by.
const restoreId = 8301n;

// Ordinals of the `books` columns `/api/window` names, so a schema change
// moves one literal rather than five.
const BOOK_ID = 0;
const BOOK_AUTHOR_ID = 1;
const BOOK_TITLE = 2;
const BOOK_YEAR = 3;
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

const TABLES = ["authors", "books", "sales", "shipments"];

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
  /** Ask for rows a soft delete has retired; needs the `read_deleted` grant. */
  includeDeleted?: boolean;
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
    ...(spec.includeDeleted ? { includeDeleted: true } : {}),
  };
}

class Adapter {
  readonly clients: Record<string, Client> = {};

  constructor(head: string) {
    for (const [name, identity] of Object.entries(IDENTITIES)) {
      // Every request from this adapter now carries a schema check. The
      // declaration is generated from the node's own catalog by
      // `scripts/codegen.py`, so the check cannot be satisfied by a
      // declaration that merely agrees with itself — which is what a
      // hand-typed one would be.
      this.clients[name] = Client.connect(head, identity).declaring(CATALOG);
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
   * A window function, one value per input row. See CONTRACT.md.
   *
   * Fixed shape, like `/api/join`: the demo is about which window, not about a
   * general window builder, and three implementations of one would be three
   * places for the same expression language to drift.
   */
  async window(
    session: Session,
    body: { function?: string; partition?: boolean; running?: boolean; limit?: number },
  ): Promise<unknown> {
    // The order is the *window's*, not the query's, and whether there is one
    // is what turns an aggregate's frame from the whole partition into a
    // running value. The ranking functions and lag/lead always get one: the
    // server refuses them without, because the answer would be a number for
    // an order nobody asked for.
    let ordered = body.running === true;
    let fn: Window;
    switch (body.function) {
      case "rowNumber":
        [fn, ordered] = [rowNumber(), true];
        break;
      case "rank":
        [fn, ordered] = [rank(), true];
        break;
      case "denseRank":
        [fn, ordered] = [denseRank(), true];
        break;
      case "lag":
        [fn, ordered] = [lag(key0(BOOK_YEAR), 1), true];
        break;
      case "lead":
        [fn, ordered] = [lead(key0(BOOK_YEAR), 1), true];
        break;
      case "sum":
        fn = aggregateOver(sumOf(key0(BOOK_YEAR)));
        break;
      case "count":
        fn = aggregateOver(count());
        break;
      default:
        throw new Error(`no such window function: ${body.function}`);
    }
    const partition: ColumnRef[] = body.partition ? [key0(BOOK_AUTHOR_ID)] : [];
    const order = ordered
      ? [{ column: BOOK_YEAR, direction: "asc" as const }]
      : [];

    // `author_id <= 6` keeps out book 19, whose author matches nobody: it is
    // here for the outer joins and would be a partition of one in every
    // answer. The query's own sort is by id, so the three adapters compare
    // row for row rather than in whatever order the scan produced.
    const stream = session.query({
      table: "books",
      filter: le(BOOK_AUTHOR_ID, uint(6)),
      sort: [{ column: BOOK_ID, direction: "asc" }],
      // Spread rather than `limit: body.limit`, because
      // `exactOptionalPropertyTypes` makes an explicit `undefined` a different
      // thing from an absent field — and a `Query` with no limit is the latter.
      ...(body.limit === undefined ? {} : { limit: body.limit }),
      window: [over(fn, { partition, order })],
    });
    const rows: unknown[] = [];
    for await (const row of stream.withComputed()) {
      rows.push({
        row: encodeRow(row.values),
        // Its own list, because it is its own list on the wire: a window value
        // is not a column and not a computed value, and an adapter folding it
        // into `row` would return something a caller reads as a different
        // thing.
        windowed: encodeRow(row.windowed),
      });
    }
    return { rows };
  }

  /**
   * Full-text over `books.title`, by index or by scan. See CONTRACT.md.
   *
   * `body.text` goes across whole. Splitting it here would be a fourth
   * tokenizer beside the server's, and a client that split differently finds
   * fewer rows than the table holds with nothing anywhere reporting it.
   */
  async search(
    session: Session,
    body: { text?: string; path?: string; limit?: number },
  ): Promise<unknown> {
    let hint: AccessHint;
    switch (body.path) {
      case "index":
        hint = usingIndex("by_title_text");
        break;
      case "scan":
        hint = usingTableScan();
        break;
      default:
        throw new Error(`no such access path: ${body.path}`);
    }

    const query: Query = {
      table: "books",
      filter: contains(BOOK_TITLE, body.text ?? ""),
      sort: [{ column: BOOK_ID, direction: "asc" }],
      // Spread rather than `limit: body.limit`, for the reason `window` gives:
      // `exactOptionalPropertyTypes` makes an explicit `undefined` a different
      // thing from an absent field.
      ...(body.limit === undefined ? {} : { limit: body.limit }),
      hint,
    };

    // Explained before it is run, because the access path is the only thing
    // that tells the two requests apart: the rows are identical by
    // construction and an adapter ignoring `path` would look correct.
    //
    // A caller without the `explain` grant gets `null` here rather than a
    // refusal. EXPLAIN is privileged on purpose — a plan is costed against
    // statistics covering rows the caller's policy hides — and the demo's
    // `reader` role does not have it. Refusing the whole search over a
    // diagnostic would make full-text the one feature a restricted reader
    // cannot use at all, which is a bigger hole than an absent field. Only
    // `permission-denied` is swallowed; every other failure is still the
    // request's failure.
    let access: string | null = null;
    try {
      access = (await session.explain(query)).access;
    } catch (error) {
      if (!(error instanceof SlateError) || error.kind !== "permission-denied") throw error;
    }

    const rows: unknown[] = [];
    for await (const row of session.query(query)) {
      rows.push(encodeRow(row));
    }
    return { rows, access };
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
    // The key, from the generated declaration rather than three string
    // literals. `answers` is the reason: the table a read decodes as is
    // `sales` one way and `books` the other, and it used to be written out
    // here and again in `path` below. The wrong one is refused by the schema
    // check rather than mis-decoded — measured in Go, see the type's own
    // comment — so this is a convenience, not a fix for a silent bug.
    const key = SalesForeignKeys["sale_book"]!;
    const way = parents ? "parents" : "children";
    // `through` is overridable only so the conformance corpus can name a key
    // that does not exist and compare the three refusals, which is the one
    // thing about this call the three could spell differently.
    const groups = await session.related(
      answers(key, way),
      { on: key.child, through: body.through || key.name, way },
      keys,
    );
    // A group per key the caller sent, in the caller's order, including the
    // empty ones — the shape all three clients promise.
    return { groups: groups.map((group) => group.map(encodeRow)) };
  }

  /**
   * A relationship *path*, resolved level by level in one request.
   *
   * `sales -> books -> editions`: up to the book a sale sold, then down to
   * that book's editions. Two steps in opposite directions, which is the case
   * worth comparing across three SDKs — a path that only ever went one way
   * would agree even with the two directions confused.
   *
   * The keys are `book_id` values read off sale rows, because that is where a
   * path starts: at the column of the caller's own rows that relates them to
   * the first step.
   *
   * Both shapes in one answer. `trees` keeps the middle level, which is
   * `load_nested`; `through` drops it, which is `load_related_through`. The
   * difference is one line in each SDK — "the rows at the bottom" rather than
   * "the rows with nothing below them" — and that line is worth pinning in all
   * three.
   */
  async path(
    session: Session,
    body: { keys?: Record<string, unknown>[] },
  ): Promise<unknown> {
    const keys = (body.keys ?? []).map(decode);
    // Both steps from the generated declaration, so the `table` beside each
    // is the catalog's answer rather than this file's memory of it. The two
    // go in opposite directions, which is exactly where `answers` earns its
    // keep.
    const up = SalesForeignKeys["sale_book"]!;
    const down = EditionsForeignKeys["edition_book"]!;
    const steps: Step[] = [
      { on: up.child, through: up.name, way: "parents", table: answers(up, "parents") },
      {
        on: down.child,
        through: down.name,
        way: "children",
        table: answers(down, "children"),
      },
    ];
    const trees = await session.relatedPath(steps, keys);
    const through = await session.relatedThrough(steps, keys);
    return {
      trees: trees.map((tree) =>
        tree.map((node) => ({
          row: encodeRow(node.row),
          related: node.related.map((leaf) => encodeRow(leaf.row)),
        })),
      ),
      through: through.map((rows) => rows.map(encodeRow)),
    };
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
      case "discounted":
        // Money, and the one arithmetic rule that is not arithmetic: a decimal
        // literal has no scale of its own and takes the column's, so
        // `units(50)` beside a scale-2 price is fifty *cents*. A client that
        // sent `int(50)` instead would be refused by the server — a decimal
        // beside a plain number has no unit — which is the difference this
        // case exists to catch, in three languages that each have their own
        // idea of what an integer literal is.
        compute = [sub(ref(at(books, 7)), lit(units(50)))];
        key = joinComputed(0);
        break;
      case "doubled":
        // The other expressible shape: money times a whole number is still
        // money, at the same scale.
        compute = [mul(ref(at(books, 7)), lit(int(2)))];
        key = joinComputed(0);
        break;
      case "badPrice":
        // Refused by the *server*, at plan time: a decimal added to a plain
        // integer would be a count of nothing. Sent rather than caught here on
        // purpose — the claim is that all three clients surface the same
        // refusal, which an adapter that validated locally would not test.
        compute = [add(ref(at(books, 7)), ref(at(books, 3)))];
        key = joinComputed(0);
        break;
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
    // Built through the *generated* encoder rather than as a positional list.
    // The eight values this replaces were in catalog order with nothing
    // checking the order or the tags — and `int` and `uint` are both `bigint`
    // here, so a swapped pair typechecks and is refused by the server. It is
    // also what stops the encoders being generated, compiled and never called.
    const rows: Value[][] = [];
    for (let n = 0n; n < 4n; n++) {
      rows.push(
        encodeBooks({
          id: first + n,
          author_id: 1n,
          title: `Predicate ${n}`,
          year: 2000n + n,
          rating: 3,
          released: 1767225600n,
          embedding: [0.1, 0.2, 0.3, 0.4],
          price: 1000n,
        }),
      );
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
        units(1000n),
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
  /**
   * Optimistic concurrency, and the decimal column it exists to protect.
   *
   * Seeds one book at 9300 priced 10.00, reads it back, optionally lets
   * somebody else move the price, then tries a conditional update to 12.50
   * guarded by the row as it was read. With `stale: false` it lands; with
   * `stale: true` the server refuses it and the price is whatever the other
   * writer left.
   *
   * One endpoint rather than two because the *pair* is the point: an
   * unconditional update and a conditional one over an unchanged row do
   * exactly the same thing, so only the stale case tells them apart.
   */
  async conditionalUpdate(session: Session, body: { stale?: boolean }): Promise<unknown> {
    const id = 9300n;
    await session.upsert("books", bookRow(id, "Priced"));

    // Read the row back rather than reusing what was written: a conditional
    // update guards against what is *stored*, and a caller that guards with
    // its own draft is testing its memory rather than the database.
    const was = await session.get("books", [uint(id)]);
    if (!was) throw new Error("the seeded row is not there");

    if (body.stale) {
      const moved = [...was];
      moved[7] = units(1100n);
      await session.update("books", moved);
    }

    const next = [...was];
    next[7] = units(1250n);
    let refused = "";
    try {
      await session.updateIfUnchanged("books", { row: next, was });
    } catch (error) {
      if (!(error instanceof SlateError)) throw error;
      // A field rather than an adapter error, so the corpus compares the two
      // cases as ordinary answers instead of one being a refusal case.
      refused = kindName(error);
    }

    const after = await session.get("books", [uint(id)]);
    if (!after) throw new Error("the row vanished");
    const price = after[7];
    if (price?.kind !== "units") {
      throw new Error(`a decimal came back as ${String(price?.kind)}`);
    }
    return {
      refused,
      price: encode(price),
      // The rendering, against the scale this adapter declares. The tagged
      // value above is the units and says nothing about a scale, so this is
      // the only place the three clients' renderers are compared.
      rendered: unitsToString(price.value, 2),
    };
  }

  /**
   * The delete half of optimistic concurrency.
   *
   * `stale` lets somebody else edit the row first; `gone` removes it first.
   * Applied, refused-because-moved and refused-because-absent are three
   * different answers, and the third is the interesting one: a *plain* delete
   * reports an absent key as `affected: 0`, and a conditional one refuses it.
   */
  /**
   * Seed three shipments, retire two, and erase what was retired.
   *
   * Its own id range, and re-seeded on every call, because all three adapters
   * run this case against one database in turn: the first purge erases the
   * rows, and the second and third would find nothing and disagree. The upsert
   * puts them back, which is the same trick `conditionalDelete` uses.
   */
  async purge(session: Session): Promise<unknown> {
    const ids = [8401n, 8402n, 8403n];
    // A purge is **table-wide** — it takes an instant, not a predicate — so it
    // also erases the row the demo seeder retired. Left alone that made this
    // case depend on which adapter ran first: the first purged three rows and
    // the other two purged two, and all three were right.
    //
    // So: clear the table of retired rows first, run the experiment against a
    // known state, and put the seeder's row back at the end. The handler
    // leaves the table as it found it, which is what keeps the corpus free of
    // an ordering rule nobody would think to preserve.
    const bound = () => BigInt(Math.floor(Date.now() / 1000)) + 3600n;
    await session.purgeDeleted("shipments", bound());

    await session.upsert(
      "shipments",
      ...ids.map((id) => [uint(id), uint(10n), str("pending"), nullValue]),
    );
    // Retire two. A delete rather than a write of `deleted_at`, because the
    // stamp is the server's clock and this is the only path that sets it.
    await session.delete("shipments", [uint(ids[0]!)], [uint(ids[1]!)]);

    // The bound is comfortably after the retirement above. The clock is the
    // server's and this is the client's, so a bound of "now" would be a race
    // on a slow machine.
    const purged = await session.purgeDeleted("shipments", bound());

    // What is left, retired rows included, so the answer distinguishes
    // "erased" from "still there but hidden".
    const left = await session.query({
      table: "shipments",
      filter: ge(0, uint(ids[0]!)),
      sort: [{ column: 0, direction: "asc" }],
      includeDeleted: true,
    });
    const rows = await left.collect();
    const answer = {
      purged: Number(purged.affected),
      left: rows.map((row) => Number((row[0] as { value: bigint }).value)),
    };
    // Put the seeder's retired shipment back, so the next adapter to run this
    // case — and any case added later that expects it — finds the database as
    // the seeder left it. Written live and then deleted, because a row cannot
    // be created already retired.
    await session.upsert("shipments", [
      uint(603n),
      uint(13n),
      str("pending"),
      nullValue,
    ]);
    await session.delete("shipments", [uint(603n)]);
    return answer;
  }

  /**
   * Puts `restoreId` in the table, retired, and hands back the decoded row.
   *
   * Upsert then delete, because a row cannot be created already retired — the
   * stamp is the server's clock and `delete` is the only path that sets it.
   * The upsert is also what makes this idempotent now that an upsert at a
   * retired row's key restores it rather than reporting it missing, which is
   * the very behaviour these two handlers exist to demonstrate.
   */
  private async retireRestoreRow(session: Session): Promise<Shipments> {
    await session.upsert("shipments", [
      uint(restoreId),
      uint(10n),
      str("pending"),
      nullValue,
    ]);
    await session.delete("shipments", [uint(restoreId)]);
    const stream = await session.query({
      table: "shipments",
      filter: eq(0, uint(restoreId)),
      includeDeleted: true,
    });
    const rows = await stream.collect();
    if (rows.length !== 1) {
      throw new Error(`expected one retired shipment, got ${rows.length}`);
    }
    return decodeShipments(rows[0]!);
  }

  /**
   * Erases this handler's row and puts the seeder's retired one back.
   *
   * The same shape `purge` uses, and for the same reason: three adapters run
   * every case against one database in turn, so a case that leaves a row
   * behind makes the next adapter's answer depend on the order. A purge is
   * table-wide, so it takes the seeder's row 603 with it and 603 has to be
   * re-retired afterwards.
   */
  private async leaveShipmentsAsFound(session: Session): Promise<void> {
    await session.delete("shipments", [uint(restoreId)]);
    await session.purgeDeleted(
      "shipments",
      BigInt(Math.floor(Date.now() / 1000)) + 3600n,
    );
    await session.upsert("shipments", [
      uint(603n),
      uint(13n),
      str("pending"),
      nullValue,
    ]);
    await session.delete("shipments", [uint(603n)]);
  }

  /**
   * Brings a retired row back, through the generated helper.
   *
   * A retired row used to be writable by nobody at any privilege, so the only
   * thing that could happen to one was being erased. This is the other half of
   * a retention window, and the reason it is a conformance case is that all
   * three clients now generate a `restored` helper and all three have to agree
   * about what it produces and what the server does with it.
   *
   * The answer carries the row's state at three points rather than just the
   * last, because "it is live now" is also what a handler that quietly
   * re-inserted a fresh row would report.
   */
  async restore(session: Session): Promise<unknown> {
    const retired = await this.retireRestoreRow(session);

    // An ordinary read, with no `includeDeleted`: the row is invisible.
    const hiddenStream = await session.query({
      table: "shipments",
      filter: eq(0, uint(restoreId)),
    });
    const hidden = (await hiddenStream.collect()).map((row) =>
      Number((row[0] as { value: bigint }).value),
    );

    // The restore. `restoredShipments` is generated from the catalog — it
    // clears whichever column the catalog names as the stamp — and the update
    // is ordinary, because there is no restore verb.
    await session.update("shipments", encodeShipments(restoredShipments(retired)));

    const backStream = await session.query({
      table: "shipments",
      filter: eq(0, uint(restoreId)),
    });
    const back = (await backStream.collect()).map((row) => decodeShipments(row));
    const answer = {
      retired_before: isRetiredShipments(retired),
      hidden_while_retired: hidden,
      visible_after: back.map((row) => Number(row.id)),
      retired_after: back.map((row) => isRetiredShipments(row)),
      // Every other column carried through, which is what separates a restore
      // from an insert of a fresh row at the same key.
      status_after: back.map((row) => row.status),
      book_id_after: back.map((row) => Number(row.book_id)),
    };
    await this.leaveShipmentsAsFound(session);
    return answer;
  }

  /**
   * Writes the retired row back exactly as `includeDeleted` gave it.
   *
   * The mistake anybody restoring by hand makes first, and the reason the
   * refusal is its own error rather than a row-level-security one: the
   * soft-delete column is the server's to write. Here so that the three
   * clients are compared on the reason token and the message, not only on the
   * happy path.
   *
   * The write is refused, so the row is left retired and the cleanup is the
   * same one the happy path does.
   */
  async restoreUnchanged(session: Session): Promise<unknown> {
    const retired = await this.retireRestoreRow(session);
    try {
      await session.update("shipments", encodeShipments(retired));
    } finally {
      await this.leaveShipmentsAsFound(session);
    }
    // Reached only if the server stopped refusing, which is a disagreement
    // worth failing loudly on rather than reporting as an answer.
    throw new Error("the server accepted a caller-supplied deleted_at");
  }

  /**
   * Sends two shipments an independent batch will refuse, one for two reasons
   * and one for a single reason.
   *
   * The gap this closes: a batch reports each failure as *data* inside a
   * successful response, so there are no trailers and no
   * `grpc-status-details-bin`. A caller submitting a form as a batch got the
   * reason token and the prose and nothing to put beside a field. The server
   * now carries the same blob in the message body.
   *
   * Both rows are refused, so nothing is written and there is nothing to undo
   * — and the two refusals differ, which is what makes the case say more than
   * "a batch can fail": 9498 breaks `status_known` and `id_is_seeded`, 9497
   * breaks only `id_is_seeded`, and an adapter reporting one list for both
   * would be caught here rather than looking plausible.
   */
  async badBatch(session: Session): Promise<unknown> {
    const result = await session.batch({
      atomicity: "independent",
      operations: [
        {
          kind: "insert",
          table: "shipments",
          rows: [[uint(9498n), uint(10n), str("teleported"), nullValue]],
        },
        {
          kind: "insert",
          table: "shipments",
          rows: [[uint(9497n), uint(10n), str("pending"), nullValue]],
        },
      ],
    });
    return {
      outcomes: result.outcomes.map((one) => {
        if (!one.error) {
          // Reached only if the server stopped enforcing a check, which is a
          // disagreement worth failing loudly on.
          throw new Error("the server accepted a row two checks refuse");
        }
        return {
          kind: one.error.kind,
          reason: one.error.reason,
          violations: one.error.violations.map((violation) => ({
            check: violation.check,
            column: violation.column,
          })),
        };
      }),
    };
  }

  /**
   * Reads two rows and decodes them with the *generated* decoders.
   *
   * The gap this closes, recorded when the decoders were first executed: every
   * test of them builds values by hand, so all three suites agree with their
   * own idea of what the server sends. A value arriving as `int` where the
   * schema says `uint` would pass every one of them and fail here — the only
   * failure the decoders exist to catch that a hand-built row cannot show.
   *
   * It is also the first thing that *calls* a generated decoder outside a
   * test. They were generated, compiled, typechecked and run against fixtures,
   * and no code path used one.
   *
   * `books` 10 covers uint, string, int, decimal and vector; `shipments` 600
   * covers the nullable column and the enumerated one. `rating` is left out on
   * purpose: a float's spelling is the one thing three languages will not
   * agree on without a shared formatter, the corpus pins it elsewhere, and
   * this case is about *decoding* rather than rendering.
   */
  async typed(session: Session): Promise<unknown> {
    const bookRow = await session.get("books", [uint(10n)]);
    if (!bookRow) throw new Error("the seeded book is not there");
    const book = decodeBooks(bookRow);

    const shipmentRow = await session.get("shipments", [uint(600n)]);
    if (!shipmentRow) throw new Error("the seeded shipment is not there");
    const shipment = decodeShipments(shipmentRow);

    // The other three tables, added because two of five decoders having a live
    // row meant "the decoders agree with the server" held for the two somebody
    // picked. They carry no value *shape* the first two do not — their point is
    // the column list, checked against the real catalog rather than a fixture
    // written from it.
    const authorRow = await session.get("authors", [uint(1n)]);
    if (!authorRow) throw new Error("the seeded author is not there");
    const author = decodeAuthors(authorRow);

    const saleRow = await session.get("sales", [uint(100n)]);
    if (!saleRow) throw new Error("the seeded sale is not there");
    const sale = decodeSales(saleRow);

    const editionRow = await session.get("editions", [uint(500n)]);
    if (!editionRow) throw new Error("the seeded edition is not there");
    const edition = decodeEditions(editionRow);

    // Every integer as a decimal string, because these are `bigint` here and
    // JSON numbers are doubles. The demo's other handlers agree.
    return {
      book: {
        id: String(book.id),
        author_id: String(book.author_id),
        title: book.title,
        year: String(book.year),
        // A decimal is a count of the smallest unit; the scale lives in the
        // schema and the row type does not know it.
        price: String(book.price),
        dimensions: book.embedding.length,
      },
      shipment: {
        id: String(shipment.id),
        book_id: String(shipment.book_id),
        status: shipment.status,
        deleted_at: shipment.deleted_at === null ? "null" : String(shipment.deleted_at),
        // Through the generated accessor; see the Go adapter for why.
        retired: isRetiredShipments(shipment),
      },
      author: {
        id: String(author.id),
        // `name` and `country` are both strings and adjacent, so a decoder one
        // ordinal out would read a plausible value. The seeded values differ,
        // which is what makes that visible here.
        name: author.name,
        country: author.country,
        born: String(author.born),
      },
      sale: {
        id: String(sale.id),
        book_id: String(sale.book_id),
        units: String(sale.units),
      },
      edition: {
        id: String(edition.id),
        book_id: String(edition.book_id),
        format: edition.format,
      },
    };
  }

  /**
   * Writes a shipment that breaks two of its table's checks at once.
   *
   * Two, not one, and that is the point: `violations` is a *list*, decoded by
   * counting up from a count, and reading one failure is different code from
   * reading several. `"teleported"` breaks `status_known` and `id` 9499 breaks
   * `id_is_seeded`, so the refusal carries both — in the order `head.toml`
   * declares them, which is not the order the row breaks them in.
   *
   * The row is never written, so there is nothing to clean up — the one
   * convenience a refusal case has over `purge` above.
   */
  async badStatus(session: Session): Promise<unknown> {
    await session.upsert("shipments", [
      uint(9499n),
      uint(10n),
      str("teleported"),
      nullValue,
    ]);
    // Reached only if the server stopped enforcing the check, which is a
    // disagreement worth failing loudly on rather than reporting as an answer.
    throw new Error("the server accepted a status no CHECK admits");
  }

  async conditionalDelete(
    session: Session,
    body: { stale?: boolean; gone?: boolean },
  ): Promise<unknown> {
    const id = 9301n;
    const key = [uint(id)];
    await session.upsert("books", bookRow(id, "Doomed"));
    const was = await session.get("books", key);
    if (!was) throw new Error("the seeded row is not there");

    if (body.stale) {
      const moved = [...was];
      moved[7] = units(1100n);
      await session.update("books", moved);
    }
    if (body.gone) {
      await session.delete("books", key);
    }

    let refused = "";
    let affected = 0n;
    try {
      const result = await session.deleteIfUnchanged("books", { key, was });
      affected = result.affected;
    } catch (error) {
      if (!(error instanceof SlateError)) throw error;
      refused = kindName(error);
    }

    // `left` is what the table says, beside `affected` and `refused`, which
    // are what the server said it did.
    const left = (await session.get("books", key)) !== undefined;
    return { refused, affected: Number(affected), left };
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
    "/api/window": (s, b) => adapter.window(s, b),
    "/api/search": (s, b) => adapter.search(s, b),
    "/api/join": (s, b) => adapter.join(s, b),
    "/api/aggregate": (s, b) => adapter.aggregate(s, b),
    "/api/explain": (s, b) => adapter.explain(s, b),
    "/api/explain-aggregate": (s, b) => adapter.explainAggregate(s, b),
    "/api/nearest": (s, b) => adapter.nearest(s, b),
    "/api/chain": (s, b) => adapter.chain(s, b),
    "/api/page": (s, b) => adapter.page(s, b),
    "/api/related": (s, b) => adapter.related(s, b),
    "/api/path": (s, b) => adapter.path(s, b),
    "/api/batch": (s, b) => adapter.batch(s, b),
    "/api/predicate-write": (s, b) => adapter.predicateWrite(s, b),
    "/api/conditional-update": (s, b) => adapter.conditionalUpdate(s, b),
    "/api/conditional-delete": (s, b) => adapter.conditionalDelete(s, b),
    "/api/purge": (s) => adapter.purge(s),
    "/api/restore": (s) => adapter.restore(s),
    "/api/restore-unchanged": (s) => adapter.restoreUnchanged(s),
    "/api/bad-status": (s) => adapter.badStatus(s),
    "/api/typed": (s) => adapter.typed(s),
    "/api/bad-batch": (s) => adapter.badBatch(s),
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
            // `reason` rides along with `kind` and is the stronger of the
            // two: a kind is this adapter's word for a status code, while the
            // token is the server's own and is finer than the code. Always
            // present, empty string included, because whether the server sent
            // a token is itself part of what the three SDKs must agree on — a
            // client that silently stopped decoding the details blob would
            // otherwise report the same body as one that decoded it and found
            // nothing.
            // `violations` is the same argument one level down. The token
            // says *that* a row broke a check; this says which ones, and it
            // is the part each client decodes by hand out of
            // `ErrorInfo.metadata`. Three hand-written decoders is exactly
            // the shape of thing that drifts, and each client's unit tests
            // decode a captured fixture — which proves each agrees with a
            // recording, not that they agree with each other against a live
            // server. This is where that is checked. Always present, `[]`
            // included, for the reason `reason` is.
            //
            // Spread into fresh objects rather than passed through, so the
            // JSON is this adapter's shape and not one client's: Python
            // spells an absent column `None` and this one spells it `""`,
            // which is each language's own idiom and not a disagreement
            // about what the server said. The flattening is what `kindName`
            // already does for status codes.
            error: {
              kind: kindName(error),
              message: error.message.replace(/^[a-z-]+: /, ""),
              reason: error.reason,
              violations: error.violations.map((one) => ({
                check: one.check,
                column: one.column,
                message: one.message,
              })),
            },
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
    // 10.00, at the column's declared scale of 2.
    units(1000n),
  ];
}
