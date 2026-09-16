import { fileURLToPath } from "node:url";
import path from "node:path";
import * as grpc from "@grpc/grpc-js";
import * as protoLoader from "@grpc/proto-loader";

import { directoryOf, findUpContaining } from "./paths.js";

import { fromServiceError, SlateError } from "./errors.js";
import { claimFor, type Schemas } from "./schema.js";
import {
  applyGrouping,
  type Group,
  type Grouping,
  type JoinQuery,
  joinToWire,
} from "./join.js";
import { type Query, queryToWire } from "./query.js";
import { type Value, valueFromWire, valueKey, valueToWire } from "./value.js";

/**
 * Who a request runs as.
 *
 * The head node does not authenticate: it reads three headers a proxy is
 * expected to have set. This only fills them in, and is not called
 * `Credentials` because nothing here proves anything.
 */
export interface Identity {
  /**
   * The caller's id, tagged: `"str:alice"`, `"u64:7"`, `"uuid:…"`. The tag is
   * required by the server, because 7 as a string and 7 as a number are
   * different principals to it.
   */
  readonly principal: string;
  /** The tenant the caller acts within, tagged the same way. */
  readonly tenant?: string;
  /** The roles the caller holds. */
  readonly roles?: string[];
}

/**
 * How far a read has to see.
 *
 * An opaque sequence from the writer. Pass one back to get a read that
 * includes an earlier write, which is the whole reason it exists.
 */
export type ReadToken = bigint;

/** Which store answered a read, and how far along it was. */
export interface ServedBy {
  /** The store's name, or empty for the writer. */
  readonly replica: string;
  /** How far that store had caught up. */
  readonly sequence: bigint;
}

/** What a write reports. */
export interface WriteResult {
  /** The writer's position after this write, if it said. */
  readonly sequence?: ReadToken;
  /** How many rows the write touched. */
  readonly affected: bigint;
}

/** The plan a query would run under. */
export interface Explanation {
  readonly table: string;
  readonly access: string;
  readonly residual: string;
  readonly descending: boolean;
  readonly estimatedRows: number;
  readonly estimatedCost: number;
  readonly sorts: boolean;
  readonly indexOnly: boolean;
  /**
   * The columns this plan decodes: the projection plus whatever the residual
   * reads. Often the only visible difference between a grouped read's plan and
   * the plan of the read it groups, since narrowing the projection need not
   * change the access path.
   */
  readonly decodes: number[];
  readonly display: string;
  readonly warnings: string[];
}

/** What a node says about who holds the writer lease. */
export interface Leadership {
  readonly leader: boolean;
  readonly generation?: bigint;
  readonly holder: string;
  readonly steppedDownBecause: string;
}

/**
 * Where the `.proto` files are.
 *
 * Found by looking for the file rather than counting `..` segments: this
 * module runs from `src/` in the repository, `dist/` once built, and a package
 * root once installed, and a fixed depth is correct in exactly one of those.
 */
const PROTO_ROOT = path.join(
  findUpContaining(
    directoryOf(import.meta.url),
    [path.join("proto", "slate", "v1", "records.proto")],
    "bundled proto directory",
  ),
  "proto",
);

// `longs: String` rather than Number: a u64 primary key above 2^53 would
// silently lose precision, and a primary key is where that shows up latest.
// The client turns them into bigint at the edge.
const LOADER_OPTIONS: protoLoader.Options = {
  keepCase: false,
  longs: String,
  enums: String,
  defaults: true,
  oneofs: true,
  includeDirs: [PROTO_ROOT],
};

type RawClient = grpc.Client & Record<string, Function>;

let cachedService: grpc.ServiceClientConstructor | undefined;

function service(): grpc.ServiceClientConstructor {
  if (!cachedService) {
    const definition = protoLoader.loadSync("slate/v1/records.proto", LOADER_OPTIONS);
    const loaded = grpc.loadPackageDefinition(definition) as unknown as {
      slate: { v1: { Records: grpc.ServiceClientConstructor } };
    };
    cachedService = loaded.slate.v1.Records;
  }
  return cachedService;
}

function rowToWire(values: Value[]): Record<string, unknown> {
  return { values: values.map(valueToWire) };
}

/**
 * A row's stored columns, as they arrived.
 *
 * Its computed values are left behind: a computed value is not a column, and a
 * caller indexing past the table's own columns would get one silently. They
 * come back through `computedFromWire`, on the `withComputed` iterators.
 *
 * This comment used to say the function *refused* such a row. It never did — it
 * read `values` and ignored `computed`, which is the right behaviour described
 * as a different one. Nothing depended on the wrong reading, because until now
 * no request this client could build produced a computed value at all.
 */
function rowFromWire(row: unknown): Value[] {
  if (!row || typeof row !== "object") return [];
  const values = (row as { values?: unknown[] }).values ?? [];
  return values.map(valueFromWire);
}

/** A row's computed values, which travel apart from its columns. */
function computedFromWire(row: unknown): Value[] {
  if (!row || typeof row !== "object") return [];
  const values = (row as { computed?: unknown[] }).computed ?? [];
  return values.map(valueFromWire);
}

/** A row and what the query computed for it. See `RowStream.withComputed`. */
export interface ComputedRow {
  /** The stored columns, in table order. */
  readonly values: Value[];
  /** What `Query.compute` produced, in declaration order. */
  readonly computed: Value[];
}

/** A joined row and what the join computed for it. See `JoinStream.withComputed`. */
export interface ComputedJoinedRow {
  /** One array per input, `undefined` where an outer join found no match. */
  readonly inputs: (Value[] | undefined)[];
  /**
   * What `JoinQuery.compute` produced, in declaration order.
   *
   * Beside the inputs rather than inside one of them, because a value that may
   * read every input belongs to none of them.
   */
  readonly computed: Value[];
  /**
   * What each input's *own* `JoinInput.compute` produced, one array per input
   * in the same order as `inputs`.
   *
   * A different kind of value from `computed`: an input's computed value reads
   * only that input's table, so it travels in that input's row and is that
   * input's. `undefined` where an outer join found no match — that input
   * produced no row, so it computed nothing for this one.
   *
   * This was missing while `computed` existed, so a caller could declare an
   * input-level computed value, have the server evaluate it, and have no way
   * to read it back: it arrived in that input's `row.computed` and
   * `rowFromWire` dropped it.
   */
  readonly inputComputed: (Value[] | undefined)[];
}

/** A connection to a head node. Safe to share; a [Session] is not. */
export class Client {
  readonly #raw: RawClient;
  readonly #identity: Identity;
  readonly #timeoutMs: number | undefined;
  #schemas: Schemas | undefined;

  private constructor(
    raw: RawClient,
    identity: Identity,
    timeoutMs?: number,
    schemas?: Schemas,
  ) {
    this.#raw = raw;
    this.#identity = identity;
    this.#timeoutMs = timeoutMs;
    this.#schemas = schemas;
  }

  /**
   * The same connection, under a per-call deadline.
   *
   * **Milliseconds**, which is what every other duration in JavaScript is; the
   * Python client's `with_timeout` takes *seconds*, and both name their unit in
   * the parameter because a caller reading one and writing the other would
   * otherwise be off by a thousand.
   *
   * A new `Client` rather than a setting, so a short deadline cannot be left
   * switched on by a caller who forgot to restore it, and so one view can be
   * handed to a background task while another is in use. The underlying
   * connection, identity and schema declarations are shared; `close()` on
   * either closes it.
   *
   * The deadline covers a whole streaming call, not each message: a scan that
   * returns rows steadily for longer than this is cancelled part-way. That is
   * what a gRPC deadline means, and it is why this is per call rather than one
   * number for the connection.
   */
  withTimeout(milliseconds: number | undefined): Client {
    return new Client(this.#raw, this.#identity, milliseconds, this.#schemas);
  }

  /**
   * Attach table declarations, so every request naming one carries a schema
   * check.
   *
   * Optional and per-table: a table with no declaration sends no claim and is
   * served as before. Worth doing for any table whose column *order* this
   * client hard-codes, which is all of them — see `TableDef`.
   */
  declaring(schemas: Schemas): this {
    this.#schemas = schemas;
    return this;
  }

  /** @internal */
  claim(table: string): Record<string, unknown> | undefined {
    return claimFor(this.#schemas, table);
  }

  /**
   * Connect to a head node.
   *
   * Insecure by default and deliberately: this client is meant to sit behind
   * the same proxy that sets its identity headers, and offering TLS here would
   * suggest the identity was protected by something. Pass `credentials` to
   * override.
   */
  static connect(
    target: string,
    identity: Identity,
    credentials: grpc.ChannelCredentials = grpc.credentials.createInsecure(),
  ): Client {
    const Records = service();
    const raw = new Records(target, credentials) as RawClient;
    return new Client(raw, identity);
  }

  /** @internal The call options every RPC carries, deadline included. */
  #options(): grpc.CallOptions {
    // An absolute instant, because that is what grpc-js wants: a relative
    // number here would be read as a Unix timestamp in 1970 and expire every
    // call immediately.
    return this.#timeoutMs === undefined ? {} : { deadline: Date.now() + this.#timeoutMs };
  }

  /** Release the connection. */
  close(): void {
    this.#raw.close();
  }

  /** Start a session. Monotonic reads are on, which is the safe default. */
  session(): Session {
    return new Session(this, true);
  }

  /**
   * Start one that does not carry its watermark.
   *
   * For a caller that wants the cheapest read available and has decided going
   * backwards in time is acceptable. Named at length so it is a decision.
   */
  sessionWithoutMonotonicReads(): Session {
    return new Session(this, false);
  }

  /**
   * Ask a node about the writer lease.
   *
   * Answered by any node, leader or not, which is the point: a follower's
   * answer is how a caller finds the leader.
   */
  async leadership(): Promise<Leadership> {
    const response = await this.call<{
      standing: string;
      generation?: string;
      holder: string;
      steppedDownBecause: string;
    }>("Leadership", {});
    return {
      leader: response.standing === "STANDING_LEADER",
      ...(response.generation !== undefined && response.generation !== null
        ? { generation: BigInt(response.generation) }
        : {}),
      holder: response.holder,
      steppedDownBecause: response.steppedDownBecause,
    };
  }

  /** @internal */
  metadata(): grpc.Metadata {
    const md = new grpc.Metadata();
    md.set("slate-principal", this.#identity.principal);
    if (this.#identity.tenant) md.set("slate-tenant", this.#identity.tenant);
    if (this.#identity.roles?.length) {
      md.set("slate-roles", this.#identity.roles.join(","));
    }
    return md;
  }

  /** @internal */
  call<T>(method: string, request: unknown): Promise<T> {
    return new Promise((resolve, reject) => {
      const fn = this.#raw[method];
      if (!fn) {
        reject(new Error(`slate: this server has no ${method} method`));
        return;
      }
      fn.call(
        this.#raw,
        request,
        this.metadata(),
        this.#options(),
        (error: grpc.ServiceError | null, response: T) => {
          if (error) reject(fromServiceError(error));
          else resolve(response);
        },
      );
    });
  }

  /** @internal */
  stream(method: string, request: unknown): grpc.ClientReadableStream<unknown> {
    const fn = this.#raw[method];
    if (!fn) throw new Error(`slate: this server has no ${method} method`);
    return fn.call(
      this.#raw,
      request,
      this.metadata(),
      this.#options(),
    ) as grpc.ClientReadableStream<unknown>;
  }
}

/**
 * A sequence of operations that keeps its own read position.
 *
 * Reads through one session never go backwards in time: a read after a write
 * sees that write. Without it a replica read can be served by a node that has
 * not caught up — correct, and surprising.
 */
/**
 * Which way a relationship is read.
 *
 * Both name the *same* foreign key, because a foreign key is the
 * relationship: `shelves.library_id -> libraries` read forwards is a shelf's
 * library, and read backwards is a library's shelves.
 */
export type Way = "children" | "parents";

/**
 * One relationship, named by the foreign key that already declares it.
 *
 * Nothing about the relationship is described here — a table and a key name go
 * to the server, which resolves them against its catalog. A client that
 * described it instead ("relate these two ordinals") could describe it
 * differently from the next client and both be right, which is the divergence
 * the conformance runner exists to catch.
 */
export interface Relation {
  /** The table holding the foreign key. Always the child, either way. */
  readonly on: string;
  /** That key's name, as the catalog spells it. */
  readonly through: string;
  /** `"children"` for a library's shelves, `"parents"` for a shelf's library. */
  readonly way?: Way;
}

function relationToWire(relation: Relation): Record<string, unknown> {
  return {
    table: relation.on,
    foreignKey: relation.through,
    direction: relation.way === "parents" ? "PARENTS" : "CHILDREN",
  };
}

/** The response, grouped back onto the caller's own key order. */
function relatedFromWire(response: unknown, keys: Value[]): Value[][][] {
  const groups =
    ((response as { groups?: unknown[] }).groups ?? []) as {
      key?: unknown;
      rows?: unknown[];
    }[];

  // Keyed on the decoded value rather than matched pairwise: a caller
  // resolving a page of parents sends hundreds of keys, and a scan per key
  // would make the one saved round trip quadratic on the way back out.
  const byKey = new Map<string, Value[][]>();
  for (const group of groups) {
    byKey.set(
      valueKey(valueFromWire(group.key)),
      (group.rows ?? []).map(rowFromWire),
    );
  }
  // Empty, not missing: the caller indexes this by its own loop counter.
  return keys.map((key) => byKey.get(valueKey(key)) ?? []);
}

export class Session {
  readonly #client: Client;
  readonly #monotonic: boolean;
  #watermark: ReadToken | undefined;

  /** @internal */
  constructor(client: Client, monotonic: boolean) {
    this.#client = client;
    this.#monotonic = monotonic;
  }

  /** The furthest this session has read or written. */
  get watermark(): ReadToken | undefined {
    return this.#watermark;
  }

  /** Fold an external token in, for carrying a position between sessions. */
  observe(token: ReadToken): void {
    if (this.#watermark === undefined || token > this.#watermark) {
      this.#watermark = token;
    }
  }

  #freshness(): Record<string, unknown> | undefined {
    if (!this.#monotonic || this.#watermark === undefined) return undefined;
    return { atLeast: this.#watermark.toString() };
  }

  #observeServedBy(servedBy: unknown): void {
    if (!servedBy || typeof servedBy !== "object") return;
    const seq = (servedBy as { sequence?: string }).sequence;
    if (seq !== undefined && seq !== null) this.observe(BigInt(seq));
  }

  async #write(method: string, request: Record<string, unknown>): Promise<WriteResult> {
    const response = await this.#client.call<{ sequence?: string; affected: string }>(
      method,
      request,
    );
    const affected = BigInt(response.affected ?? 0);
    if (response.sequence !== undefined && response.sequence !== null) {
      const token = BigInt(response.sequence);
      this.observe(token);
      return { sequence: token, affected };
    }
    return { affected };
  }

  /** Add rows, refusing a primary key that is taken. */
  insert(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#write("Insert", {
      table,
      rows: rows.map(rowToWire),
      schema: this.#client.claim(table),
    });
  }

  /** Add rows, replacing any whose primary key is taken. */
  upsert(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#write("Insert", {
      table,
      rows: rows.map(rowToWire),
      upsert: true,
      schema: this.#client.claim(table),
    });
  }

  /** Replace rows, refusing one whose primary key is not there. */
  update(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#write("Update", {
      table,
      rows: rows.map(rowToWire),
      schema: this.#client.claim(table),
    });
  }

  /** Remove rows by primary key. */
  delete(table: string, ...keys: Value[][]): Promise<WriteResult> {
    return this.#write("Delete", {
      table,
      primaryKeys: keys.map(rowToWire),
      schema: this.#client.claim(table),
    });
  }

  /**
   * Read one row by primary key.
   *
   * `undefined` for a row that is not there rather than an error: checking
   * existence should not mean catching an exception.
   */
  async get(table: string, key: Value[]): Promise<Value[] | undefined> {
    const response = await this.#client.call<{
      row?: unknown;
      found: boolean;
      servedBy?: unknown;
    }>("Get", {
      table,
      primaryKey: rowToWire(key),
      freshness: this.#freshness(),
      schema: this.#client.claim(table),
    });
    this.#observeServedBy(response.servedBy);
    return response.found ? rowFromWire(response.row) : undefined;
  }

  /** Read rows. */
  query(query: Query): RowStream {
    const stream = this.#client.stream("Query", {
      query: queryToWire(query, this.#client.claim(query.table)),
      freshness: this.#freshness(),
    });
    return new RowStream(stream, (sb) => this.#observeServedBy(sb));
  }

  /**
   * Load one relationship for many parents, in a single read.
   *
   * Returns an array the same length as `keys`: entry `i` is the rows related
   * to `keys[i]`, and a key with nothing related to it gets `[]` rather than
   * being left out, so the result is indexable by the caller's own loop
   * counter.
   *
   * Duplicate keys are expected and are the point: two shelves in one library
   * send the same value twice, the server reads it once, and both map onto the
   * one group that comes back. So this is one round trip whatever the number
   * of parents, which is the reason it exists rather than a loop over `get`.
   *
   * `table` is the table the rows come back as — `relation.on` for children,
   * and the key's parent for parents. It is named separately because the
   * client decodes against it and does not hold the catalog.
   */
  related(table: string, relation: Relation, keys: Value[]): Promise<Value[][][]> {
    return this.relatedIn(undefined, table, relation, keys);
  }

  /**
   * @internal
   *
   * Both paths through one request rather than two nearly identical ones: the
   * schema claim is attached by hand at every call site, and a second site
   * that forgot it would simply not be checked. A mutation dropping it from a
   * duplicated transaction path survived the whole suite, which is why there
   * is no longer a duplicated transaction path.
   */
  async relatedIn(
    transaction: string | undefined,
    table: string,
    relation: Relation,
    keys: Value[],
  ): Promise<Value[][][]> {
    const request: Record<string, unknown> = {
      relation: relationToWire(relation),
      keys: keys.map(valueToWire),
      schema: this.#client.claim(table),
    };
    // A transaction's reads go to the writer, which is already ahead of any
    // floor this session could name.
    if (transaction !== undefined) request["transaction"] = transaction;
    else request["freshness"] = this.#freshness();

    const response = await this.#client.call<unknown>("Related", request);
    if (transaction === undefined) {
      this.#observeServedBy((response as { servedBy?: unknown }).servedBy);
    }
    return relatedFromWire(response, keys);
  }

  /** Read joined rows. */
  join(join: JoinQuery): JoinStream {
    const stream = this.#client.stream("Join", {
      join: joinToWire(join, (table) => this.#client.claim(table)),
      freshness: this.#freshness(),
    });
    return new JoinStream(stream, (sb) => this.#observeServedBy(sb));
  }

  /** Group one table. */
  aggregate(over: Query, grouping: Grouping): GroupStream {
    const query = applyGrouping(
      { input: queryToWire(over, this.#client.claim(over.table)) },
      grouping,
    );
    return this.#aggregate(query, undefined);
  }

  /**
   * Group a join, or a chain of any length.
   *
   * It was two inputs exactly when the kernel grouped only a two-table join
   * and the server refused a third; it groups a chain now, and the refusal
   * went with it. Nothing counts inputs here, where the count could drift from
   * the kernel's.
   */
  aggregateJoin(over: JoinQuery, grouping: Grouping): GroupStream {
    const query = applyGrouping(
      { join: joinToWire(over, (table) => this.#client.claim(table)) },
      grouping,
    );
    return this.#aggregate(query, undefined);
  }

  /** @internal */
  aggregateIn(
    query: Record<string, unknown>,
    transaction: string,
  ): GroupStream {
    return this.#aggregate(query, transaction);
  }

  #aggregate(query: Record<string, unknown>, transaction: string | undefined): GroupStream {
    const request: Record<string, unknown> = { aggregate: query };
    if (transaction !== undefined) request["transaction"] = transaction;
    // A transaction's reads go to the writer and need no freshness floor.
    else request["freshness"] = this.#freshness();
    const stream = this.#client.stream("Aggregate", request);
    return new GroupStream(stream, (sb) => this.#observeServedBy(sb));
  }

  /**
   * Ask for a join's plan without running it.
   *
   * Needs the `explain` action on every table involved, not just one.
   */
  async explainJoin(join: JoinQuery): Promise<JoinExplanation> {
    const r = await this.#client.call<Record<string, unknown>>("ExplainJoin", {
      join: joinToWire(join, (table) => this.#client.claim(table)),
      freshness: this.#freshness(),
    });
    this.#observeServedBy(r["servedBy"]);
    return joinExplanationFromWire(r);
  }

  /**
   * Ask for a grouped read's plan without running it.
   *
   * Not `explain` or `explainJoin` on the underlying read: grouping narrows
   * each input's projection to the group keys and the aggregates' columns,
   * which is what lets an index answer a `COUNT(*)` without touching a row.
   * Comparing `decodes` between the two is how to see it where the access path
   * is unchanged.
   *
   * Exactly one of `input` and `join` comes back, matching the request.
   */
  async explainAggregate(
    over: Query,
    grouping: Grouping,
  ): Promise<AggregateExplanation> {
    return this.#explainAggregate(
      applyGrouping({ input: queryToWire(over, this.#client.claim(over.table)) }, grouping),
    );
  }

  /** Ask for a grouped join's or grouped chain's plan. */
  async explainAggregateJoin(
    over: JoinQuery,
    grouping: Grouping,
  ): Promise<AggregateExplanation> {
    return this.#explainAggregate(
      applyGrouping(
        { join: joinToWire(over, (table) => this.#client.claim(table)) },
        grouping,
      ),
    );
  }

  /** @internal */
  explainAggregateIn(
    aggregate: Record<string, unknown>,
    transaction: string,
  ): Promise<AggregateExplanation> {
    return this.#explainAggregate(aggregate, transaction);
  }

  async #explainAggregate(
    aggregate: Record<string, unknown>,
    transaction?: string,
  ): Promise<AggregateExplanation> {
    const request: Record<string, unknown> = { aggregate };
    // A transaction's reads go to the writer and need no freshness floor —
    // the same rule `#aggregate` follows, and for the same reason.
    if (transaction !== undefined) request["transaction"] = transaction;
    else request["freshness"] = this.#freshness();
    const r = await this.#client.call<Record<string, unknown>>(
      "ExplainAggregate",
      request,
    );
    this.#observeServedBy(r["servedBy"]);
    // `@grpc/proto-loader` leaves an unset message field undefined rather than
    // an empty object, so presence here is the wire's own oneof-in-spirit and
    // not a guess about which fields came back populated.
    const input = r["input"] as Record<string, unknown> | undefined;
    const join = r["join"] as Record<string, unknown> | undefined;
    return {
      input: input ? explanationFromWire(input) : undefined,
      join: join ? joinExplanationFromWire(join) : undefined,
      display: String(r["display"] ?? ""),
      warnings: (r["warnings"] as string[]) ?? [],
    };
  }

  /**
   * Ask for a plan without running it.
   *
   * Requires the `explain` action on every table involved, which a `read`
   * grant does not carry: a plan is costed against statistics covering rows
   * the caller's policy may hide.
   */
  async explain(query: Query): Promise<Explanation> {
    const r = await this.#client.call<Record<string, unknown>>("Explain", {
      query: queryToWire(query, this.#client.claim(query.table)),
      freshness: this.#freshness(),
    });
    this.#observeServedBy(r["servedBy"]);
    return explanationFromWire(r);
  }

  /**
   * Open a transaction.
   *
   * Every operation on it goes to the writer, and its reads see its own
   * uncommitted writes. Commit or roll back: an abandoned transaction holds a
   * slot until the node's idle timeout collects it.
   */
  async begin(): Promise<Transaction> {
    const response = await this.#client.call<{ transaction: string }>("Begin", {});
    return new Transaction(this, this.#client, response.transaction);
  }

  /** @internal */
  get client(): Client {
    return this.#client;
  }

  /** @internal */
  writeThrough(method: string, request: Record<string, unknown>): Promise<WriteResult> {
    return this.#write(method, request);
  }
}

/** A set of operations that commit or roll back together. */
export class Transaction {
  readonly #session: Session;
  readonly #client: Client;
  readonly #id: string;
  #done = false;

  /** @internal */
  constructor(session: Session, client: Client, id: string) {
    this.#session = session;
    this.#client = client;
    this.#id = id;
  }

  /** The server's handle for this transaction. */
  get id(): string {
    return this.#id;
  }

  /** Make the transaction's writes visible. */
  async commit(): Promise<void> {
    if (this.#done) throw new Error("slate: this transaction is already finished");
    this.#done = true;
    const response = await this.#client.call<{ sequence?: string }>("Commit", {
      transaction: this.#id,
    });
    if (response.sequence !== undefined && response.sequence !== null) {
      this.#session.observe(BigInt(response.sequence));
    }
  }

  /**
   * Discard the transaction's writes.
   *
   * Quiet after a commit, which is what makes a `finally { await tx.rollback() }`
   * the right shape for every transaction.
   */
  async rollback(): Promise<void> {
    if (this.#done) return;
    this.#done = true;
    await this.#client.call("Rollback", { transaction: this.#id });
  }

  /** Add rows inside the transaction. */
  insert(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#session.writeThrough("Insert", {
      transaction: this.#id,
      table,
      rows: rows.map(rowToWire),
      schema: this.#client.claim(table),
    });
  }

  /** Add or replace rows inside the transaction. */
  upsert(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#session.writeThrough("Insert", {
      transaction: this.#id,
      table,
      rows: rows.map(rowToWire),
      upsert: true,
      schema: this.#client.claim(table),
    });
  }

  /** Replace rows inside the transaction. */
  update(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#session.writeThrough("Update", {
      transaction: this.#id,
      table,
      rows: rows.map(rowToWire),
      schema: this.#client.claim(table),
    });
  }

  /** Remove rows by primary key inside the transaction. */
  delete(table: string, ...keys: Value[][]): Promise<WriteResult> {
    return this.#session.writeThrough("Delete", {
      transaction: this.#id,
      table,
      primaryKeys: keys.map(rowToWire),
      schema: this.#client.claim(table),
    });
  }

  /** Read one row inside the transaction, seeing its uncommitted writes. */
  async get(table: string, key: Value[]): Promise<Value[] | undefined> {
    const response = await this.#client.call<{ row?: unknown; found: boolean }>("Get", {
      transaction: this.#id,
      table,
      primaryKey: rowToWire(key),
      schema: this.#client.claim(table),
    });
    return response.found ? rowFromWire(response.row) : undefined;
  }

  /** Read joined rows inside the transaction. */
  join(join: JoinQuery): JoinStream {
    const stream = this.#client.stream("Join", {
      transaction: this.#id,
      join: joinToWire(join, (table) => this.#client.claim(table)),
    });
    return new JoinStream(stream, () => {});
  }

  /** Group one table inside the transaction. */
  aggregate(over: Query, grouping: Grouping): GroupStream {
    return this.#session.aggregateIn(
      applyGrouping({ input: queryToWire(over, this.#client.claim(over.table)) }, grouping),
      this.#id,
    );
  }

  /** Group a join inside the transaction. */
  aggregateJoin(over: JoinQuery, grouping: Grouping): GroupStream {
    return this.#session.aggregateIn(
      applyGrouping(
        { join: joinToWire(over, (table) => this.#client.claim(table)) },
        grouping,
      ),
      this.#id,
    );
  }

  /** The plan a grouped read would run under, inside the transaction. */
  explainAggregate(over: Query, grouping: Grouping): Promise<AggregateExplanation> {
    return this.#session.explainAggregateIn(
      applyGrouping({ input: queryToWire(over, this.#client.claim(over.table)) }, grouping),
      this.#id,
    );
  }

  /** The plan a grouped join or chain would run under, inside the transaction. */
  explainAggregateJoin(over: JoinQuery, grouping: Grouping): Promise<AggregateExplanation> {
    return this.#session.explainAggregateIn(
      applyGrouping(
        { join: joinToWire(over, (table) => this.#client.claim(table)) },
        grouping,
      ),
      this.#id,
    );
  }

  /** Read rows inside the transaction. */
  query(query: Query): RowStream {
    const stream = this.#client.stream("Query", {
      transaction: this.#id,
      query: queryToWire(query, this.#client.claim(query.table)),
    });
    return new RowStream(stream, () => {});
  }

  /**
   * Load one relationship inside the transaction, seeing its uncommitted
   * writes. Otherwise exactly {@link Session.related}.
   */
  related(table: string, relation: Relation, keys: Value[]): Promise<Value[][][]> {
    return this.#session.relatedIn(this.#id, table, relation, keys);
  }
}

/**
 * Rows arriving in batches.
 *
 * Async-iterable. Break out of the loop and the call is cancelled; a stream
 * abandoned without that keeps the server producing rows nobody will read.
 */
export class RowStream implements AsyncIterable<Value[]> {
  readonly #stream: grpc.ClientReadableStream<unknown>;
  readonly #onServedBy: (servedBy: unknown) => void;
  #servedBy: ServedBy | undefined;
  #warnings: string[] = [];

  /** @internal */
  constructor(
    stream: grpc.ClientReadableStream<unknown>,
    onServedBy: (servedBy: unknown) => void,
  ) {
    this.#stream = stream;
    this.#onServedBy = onServedBy;
  }

  /** Which store answered, known once the first message has arrived. */
  get servedBy(): ServedBy | undefined {
    return this.#servedBy;
  }

  /** What the server said about the request it served. First message only. */
  get warnings(): readonly string[] {
    return this.#warnings;
  }

  /** Cancel the call. */
  cancel(): void {
    this.#stream.cancel();
  }

  async *[Symbol.asyncIterator](): AsyncIterator<Value[]> {
    try {
      for await (const message of this.#stream as AsyncIterable<Record<string, unknown>>) {
        const servedBy = message["servedBy"];
        if (servedBy && !this.#servedBy) {
          const sb = servedBy as { replica?: string; sequence?: string };
          this.#servedBy = {
            replica: sb.replica ?? "",
            sequence: BigInt(sb.sequence ?? 0),
          };
          this.#onServedBy(servedBy);
        }
        const warnings = message["warnings"] as string[] | undefined;
        if (warnings?.length) this.#warnings.push(...warnings);
        for (const row of (message["rows"] as unknown[]) ?? []) {
          yield rowFromWire(row);
        }
      }
    } catch (error) {
      if (isServiceError(error)) throw fromServiceError(error);
      throw error;
    }
  }

  /**
   * The same rows, each paired with what `Query.compute` produced for it.
   *
   * An *alternative* to iterating this stream directly, not an addition: a gRPC
   * stream is consumed once, so a caller uses one or the other. The plain
   * iterator stays the default because most queries compute nothing and
   * `for await (const row of stream)` should keep yielding a row.
   */
  async *withComputed(): AsyncIterableIterator<ComputedRow> {
    try {
      for await (const message of this.#stream as AsyncIterable<Record<string, unknown>>) {
        const servedBy = message["servedBy"];
        if (servedBy && !this.#servedBy) {
          const sb = servedBy as { replica?: string; sequence?: string };
          this.#servedBy = {
            replica: sb.replica ?? "",
            sequence: BigInt(sb.sequence ?? 0),
          };
          this.#onServedBy(servedBy);
        }
        const warnings = message["warnings"] as string[] | undefined;
        if (warnings?.length) this.#warnings.push(...warnings);
        for (const row of (message["rows"] as unknown[]) ?? []) {
          yield { values: rowFromWire(row), computed: computedFromWire(row) };
        }
      }
    } catch (error) {
      if (isServiceError(error)) throw fromServiceError(error);
      throw error;
    }
  }

  /**
   * Drain into an array.
   *
   * For an answer known to be small. Iterate a large table instead.
   */
  async collect(): Promise<Value[][]> {
    const out: Value[][] = [];
    for await (const row of this) out.push(row);
    return out;
  }
}

/**
 * Joined rows arriving in batches.
 *
 * A joined row is one array of values **per input**, `undefined` where an
 * outer join found no match — kept separate rather than concatenated, because
 * a flat row cannot tell "the right side had no match" from "the right side
 * matched and its columns are null".
 */
export class JoinStream implements AsyncIterable<(Value[] | undefined)[]> {
  readonly #stream: grpc.ClientReadableStream<unknown>;
  readonly #onServedBy: (servedBy: unknown) => void;
  #servedBy: ServedBy | undefined;
  #warnings: string[] = [];

  /** @internal */
  constructor(
    stream: grpc.ClientReadableStream<unknown>,
    onServedBy: (servedBy: unknown) => void,
  ) {
    this.#stream = stream;
    this.#onServedBy = onServedBy;
  }

  /** Which store answered, known once the first message has arrived. */
  get servedBy(): ServedBy | undefined {
    return this.#servedBy;
  }

  /** What the server said about the request it served. */
  get warnings(): readonly string[] {
    return this.#warnings;
  }

  /** Cancel the call. */
  cancel(): void {
    this.#stream.cancel();
  }

  async *[Symbol.asyncIterator](): AsyncIterator<(Value[] | undefined)[]> {
    try {
      for await (const message of this.#stream as AsyncIterable<Record<string, unknown>>) {
        this.#note(message);
        for (const joined of (message["rows"] as { inputs?: unknown[] }[]) ?? []) {
          yield (joined.inputs ?? []).map((input) => {
            const row = (input as { row?: unknown }).row;
            return row ? rowFromWire(row) : undefined;
          });
        }
      }
    } catch (error) {
      if (isServiceError(error)) throw fromServiceError(error);
      throw error;
    }
  }

  /**
   * The same rows, each paired with what `JoinQuery.compute` produced for it.
   *
   * Beside the inputs rather than inside one of them, because a value that may
   * read every input belongs to none of them.
   *
   * An *alternative* to iterating this stream directly, for the reason
   * `RowStream.withComputed` gives: a gRPC stream is consumed once.
   */
  async *withComputed(): AsyncIterableIterator<ComputedJoinedRow> {
    try {
      for await (const message of this.#stream as AsyncIterable<Record<string, unknown>>) {
        this.#note(message);
        const rows = (message["rows"] as { inputs?: unknown[]; computed?: unknown[] }[]) ?? [];
        for (const joined of rows) {
          const inputs = joined.inputs ?? [];
          yield {
            inputs: inputs.map((input) => {
              const row = (input as { row?: unknown }).row;
              return row ? rowFromWire(row) : undefined;
            }),
            computed: (joined.computed ?? []).map(valueFromWire),
            inputComputed: inputs.map((input) => {
              const row = (input as { row?: unknown }).row;
              return row ? computedFromWire(row) : undefined;
            }),
          };
        }
      }
    } catch (error) {
      if (isServiceError(error)) throw fromServiceError(error);
      throw error;
    }
  }

  #note(message: Record<string, unknown>): void {
    const servedBy = message["servedBy"];
    if (servedBy && !this.#servedBy) {
      const sb = servedBy as { replica?: string; sequence?: string };
      this.#servedBy = { replica: sb.replica ?? "", sequence: BigInt(sb.sequence ?? 0) };
      this.#onServedBy(servedBy);
    }
    const warnings = message["warnings"] as string[] | undefined;
    if (warnings?.length) this.#warnings.push(...warnings);
  }

  /** Drain into an array. */
  async collect(): Promise<(Value[] | undefined)[][]> {
    const out: (Value[] | undefined)[][] = [];
    for await (const row of this) out.push(row);
    return out;
  }
}

/** Groups arriving in batches. */
export class GroupStream implements AsyncIterable<Group> {
  readonly #stream: grpc.ClientReadableStream<unknown>;
  readonly #onServedBy: (servedBy: unknown) => void;
  #servedBy: ServedBy | undefined;
  #warnings: string[] = [];

  /** @internal */
  constructor(
    stream: grpc.ClientReadableStream<unknown>,
    onServedBy: (servedBy: unknown) => void,
  ) {
    this.#stream = stream;
    this.#onServedBy = onServedBy;
  }

  /** Which store answered. */
  get servedBy(): ServedBy | undefined {
    return this.#servedBy;
  }

  /** What the server said about the request it served. */
  get warnings(): readonly string[] {
    return this.#warnings;
  }

  /** Cancel the call. */
  cancel(): void {
    this.#stream.cancel();
  }

  async *[Symbol.asyncIterator](): AsyncIterator<Group> {
    try {
      for await (const message of this.#stream as AsyncIterable<Record<string, unknown>>) {
        const servedBy = message["servedBy"];
        if (servedBy && !this.#servedBy) {
          const sb = servedBy as { replica?: string; sequence?: string };
          this.#servedBy = { replica: sb.replica ?? "", sequence: BigInt(sb.sequence ?? 0) };
          this.#onServedBy(servedBy);
        }
        const warnings = message["warnings"] as string[] | undefined;
        if (warnings?.length) this.#warnings.push(...warnings);
        for (const group of (message["groups"] as Record<string, unknown>[]) ?? []) {
          yield {
            key: ((group["key"] as unknown[]) ?? []).map(valueFromWire),
            values: ((group["values"] as unknown[]) ?? []).map(valueFromWire),
          };
        }
      }
    } catch (error) {
      if (isServiceError(error)) throw fromServiceError(error);
      throw error;
    }
  }

  /** Drain into an array. */
  async collect(): Promise<Group[]> {
    const out: Group[] = [];
    for await (const group of this) out.push(group);
    return out;
  }
}

/** How one input of a join is planned. */
export interface JoinInputPlan {
  readonly plan: Explanation;
  readonly type: string;
  /** The algorithm the planner chose, or empty where it named none. */
  readonly algorithm: string;
  readonly estimatedRows: number;
  readonly estimatedCost: number;
}

/** The plan a join would run under. */
export interface JoinExplanation {
  readonly inputs: JoinInputPlan[];
  readonly estimatedRows: number;
  readonly estimatedCost: number;
  readonly display: string;
  readonly warnings: string[];
}

/** The plan a grouped read would run under. */
export interface AggregateExplanation {
  /** Set when the aggregate reads one table; undefined otherwise. */
  readonly input: Explanation | undefined;
  /** Set when it aggregates a join or a chain; undefined otherwise. */
  readonly join: JoinExplanation | undefined;
  readonly display: string;
  readonly warnings: string[];
}

/**
 * The join algorithm as a name, or `""` where the wire named none.
 *
 * These exact strings, because the three clients have to spell it the same.
 * This used to return whichever key `proto-loader` gave the `oneof` —
 * `"hashBuild"` — while Go returned `"hash"` and Python handed back the raw
 * message. Three answers to one question, and nothing had asked.
 */
function algorithmName(algorithm: Record<string, unknown> | undefined): string {
  if (!algorithm) return "";
  if ("hashBuild" in algorithm) return "hash";
  if ("nestedLoop" in algorithm) return "nested loop";
  return "";
}

function joinExplanationFromWire(r: Record<string, unknown>): JoinExplanation {
  const inputs = ((r["inputs"] as Record<string, unknown>[]) ?? []).map((input) => {
    const algorithm = input["algorithm"] as Record<string, unknown> | undefined;
    return {
      plan: explanationFromWire((input["plan"] as Record<string, unknown>) ?? {}),
      type: String(input["joinType"] ?? ""),
      algorithm: algorithmName(algorithm),
      estimatedRows: Number(input["estimatedRows"] ?? 0),
      estimatedCost: Number(input["estimatedCost"] ?? 0),
    };
  });
  return {
    inputs,
    estimatedRows: Number(r["estimatedRows"] ?? 0),
    estimatedCost: Number(r["estimatedCost"] ?? 0),
    display: String(r["display"] ?? ""),
    warnings: (r["warnings"] as string[]) ?? [],
  };
}

function explanationFromWire(r: Record<string, unknown>): Explanation {
  return {
    table: String(r["table"] ?? ""),
    access: String(r["access"] ?? ""),
    residual: String(r["residual"] ?? ""),
    descending: Boolean(r["descending"]),
    estimatedRows: Number(r["estimatedRows"] ?? 0),
    estimatedCost: Number(r["estimatedCost"] ?? 0),
    sorts: Boolean(r["sorts"]),
    indexOnly: Boolean(r["indexOnly"]),
    decodes: ((r["decodes"] as unknown[]) ?? []).map(Number),
    display: String(r["display"] ?? ""),
    warnings: (r["warnings"] as string[]) ?? [],
  };
}

function isServiceError(error: unknown): error is grpc.ServiceError {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    typeof (error as { code: unknown }).code === "number"
  );
}

export { SlateError };
