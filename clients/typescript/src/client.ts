import { fileURLToPath } from "node:url";
import path from "node:path";
import { existsSync } from "node:fs";
import * as grpc from "@grpc/grpc-js";
import * as protoLoader from "@grpc/proto-loader";

import { fromServiceError, SlateError } from "./errors.js";
import { type Query, queryToWire } from "./query.js";
import { type Value, valueFromWire, valueToWire } from "./value.js";

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
 * Where the `.proto` files are, found by walking up.
 *
 * Not a fixed number of `..` segments: this module runs from `src/` in the
 * repository, from `dist/src/` once compiled, and from a package root once
 * installed, and a relative depth is correct in exactly one of those. Getting
 * it wrong fails at the first call rather than at import, which is why this
 * looks for the file rather than assuming a layout.
 */
function findProtoRoot(): string {
  let dir = path.dirname(fileURLToPath(import.meta.url));
  for (;;) {
    const candidate = path.join(dir, "proto");
    if (existsSync(path.join(candidate, "slate", "v1", "records.proto"))) {
      return candidate;
    }
    const parent = path.dirname(dir);
    if (parent === dir) {
      throw new Error("slate: cannot find the bundled proto/slate/v1/records.proto");
    }
    dir = parent;
  }
}

const PROTO_ROOT = findProtoRoot();

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
 * A row as it arrived.
 *
 * Refuses one carrying computed values: a computed value is not a column, and
 * a caller indexing past the table's own columns would get one silently.
 */
function rowFromWire(row: unknown): Value[] {
  if (!row || typeof row !== "object") return [];
  const values = (row as { values?: unknown[] }).values ?? [];
  return values.map(valueFromWire);
}

/** A connection to a head node. Safe to share; a [Session] is not. */
export class Client {
  readonly #raw: RawClient;
  readonly #identity: Identity;

  private constructor(raw: RawClient, identity: Identity) {
    this.#raw = raw;
    this.#identity = identity;
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
    return fn.call(this.#raw, request, this.metadata()) as grpc.ClientReadableStream<unknown>;
  }
}

/**
 * A sequence of operations that keeps its own read position.
 *
 * Reads through one session never go backwards in time: a read after a write
 * sees that write. Without it a replica read can be served by a node that has
 * not caught up — correct, and surprising.
 */
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
    return this.#write("Insert", { table, rows: rows.map(rowToWire) });
  }

  /** Add rows, replacing any whose primary key is taken. */
  upsert(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#write("Insert", { table, rows: rows.map(rowToWire), upsert: true });
  }

  /** Replace rows, refusing one whose primary key is not there. */
  update(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#write("Update", { table, rows: rows.map(rowToWire) });
  }

  /** Remove rows by primary key. */
  delete(table: string, ...keys: Value[][]): Promise<WriteResult> {
    return this.#write("Delete", { table, primaryKeys: keys.map(rowToWire) });
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
    });
    this.#observeServedBy(response.servedBy);
    return response.found ? rowFromWire(response.row) : undefined;
  }

  /** Read rows. */
  query(query: Query): RowStream {
    const stream = this.#client.stream("Query", {
      query: queryToWire(query),
      freshness: this.#freshness(),
    });
    return new RowStream(stream, (sb) => this.#observeServedBy(sb));
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
      query: queryToWire(query),
      freshness: this.#freshness(),
    });
    this.#observeServedBy(r["servedBy"]);
    return {
      table: String(r["table"] ?? ""),
      access: String(r["access"] ?? ""),
      residual: String(r["residual"] ?? ""),
      descending: Boolean(r["descending"]),
      estimatedRows: Number(r["estimatedRows"] ?? 0),
      estimatedCost: Number(r["estimatedCost"] ?? 0),
      sorts: Boolean(r["sorts"]),
      indexOnly: Boolean(r["indexOnly"]),
      display: String(r["display"] ?? ""),
      warnings: (r["warnings"] as string[]) ?? [],
    };
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
    });
  }

  /** Add or replace rows inside the transaction. */
  upsert(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#session.writeThrough("Insert", {
      transaction: this.#id,
      table,
      rows: rows.map(rowToWire),
      upsert: true,
    });
  }

  /** Replace rows inside the transaction. */
  update(table: string, ...rows: Value[][]): Promise<WriteResult> {
    return this.#session.writeThrough("Update", {
      transaction: this.#id,
      table,
      rows: rows.map(rowToWire),
    });
  }

  /** Remove rows by primary key inside the transaction. */
  delete(table: string, ...keys: Value[][]): Promise<WriteResult> {
    return this.#session.writeThrough("Delete", {
      transaction: this.#id,
      table,
      primaryKeys: keys.map(rowToWire),
    });
  }

  /** Read one row inside the transaction, seeing its uncommitted writes. */
  async get(table: string, key: Value[]): Promise<Value[] | undefined> {
    const response = await this.#client.call<{ row?: unknown; found: boolean }>("Get", {
      transaction: this.#id,
      table,
      primaryKey: rowToWire(key),
    });
    return response.found ? rowFromWire(response.row) : undefined;
  }

  /** Read rows inside the transaction. */
  query(query: Query): RowStream {
    const stream = this.#client.stream("Query", {
      transaction: this.#id,
      query: queryToWire(query),
    });
    return new RowStream(stream, () => {});
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

function isServiceError(error: unknown): error is grpc.ServiceError {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    typeof (error as { code: unknown }).code === "number"
  );
}

export { SlateError };
