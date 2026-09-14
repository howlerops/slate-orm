import { fileURLToPath } from "node:url";
import path from "node:path";
import { existsSync } from "node:fs";
import * as grpc from "@grpc/grpc-js";
import * as protoLoader from "@grpc/proto-loader";
import { fromServiceError, SlateError } from "./errors.js";
import { claimFor } from "./schema.js";
import { applyGrouping, joinToWire, } from "./join.js";
import { queryToWire } from "./query.js";
import { valueFromWire, valueToWire } from "./value.js";
/**
 * Where the `.proto` files are, found by walking up.
 *
 * Not a fixed number of `..` segments: this module runs from `src/` in the
 * repository, from `dist/src/` once compiled, and from a package root once
 * installed, and a relative depth is correct in exactly one of those. Getting
 * it wrong fails at the first call rather than at import, which is why this
 * looks for the file rather than assuming a layout.
 */
function findProtoRoot() {
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
const LOADER_OPTIONS = {
    keepCase: false,
    longs: String,
    enums: String,
    defaults: true,
    oneofs: true,
    includeDirs: [PROTO_ROOT],
};
let cachedService;
function service() {
    if (!cachedService) {
        const definition = protoLoader.loadSync("slate/v1/records.proto", LOADER_OPTIONS);
        const loaded = grpc.loadPackageDefinition(definition);
        cachedService = loaded.slate.v1.Records;
    }
    return cachedService;
}
function rowToWire(values) {
    return { values: values.map(valueToWire) };
}
/**
 * A row as it arrived.
 *
 * Refuses one carrying computed values: a computed value is not a column, and
 * a caller indexing past the table's own columns would get one silently.
 */
function rowFromWire(row) {
    if (!row || typeof row !== "object")
        return [];
    const values = row.values ?? [];
    return values.map(valueFromWire);
}
/** A connection to a head node. Safe to share; a [Session] is not. */
export class Client {
    #raw;
    #identity;
    #schemas;
    constructor(raw, identity) {
        this.#raw = raw;
        this.#identity = identity;
    }
    /**
     * Attach table declarations, so every request naming one carries a schema
     * check.
     *
     * Optional and per-table: a table with no declaration sends no claim and is
     * served as before. Worth doing for any table whose column *order* this
     * client hard-codes, which is all of them — see `TableDef`.
     */
    declaring(schemas) {
        this.#schemas = schemas;
        return this;
    }
    /** @internal */
    claim(table) {
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
    static connect(target, identity, credentials = grpc.credentials.createInsecure()) {
        const Records = service();
        const raw = new Records(target, credentials);
        return new Client(raw, identity);
    }
    /** Release the connection. */
    close() {
        this.#raw.close();
    }
    /** Start a session. Monotonic reads are on, which is the safe default. */
    session() {
        return new Session(this, true);
    }
    /**
     * Start one that does not carry its watermark.
     *
     * For a caller that wants the cheapest read available and has decided going
     * backwards in time is acceptable. Named at length so it is a decision.
     */
    sessionWithoutMonotonicReads() {
        return new Session(this, false);
    }
    /**
     * Ask a node about the writer lease.
     *
     * Answered by any node, leader or not, which is the point: a follower's
     * answer is how a caller finds the leader.
     */
    async leadership() {
        const response = await this.call("Leadership", {});
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
    metadata() {
        const md = new grpc.Metadata();
        md.set("slate-principal", this.#identity.principal);
        if (this.#identity.tenant)
            md.set("slate-tenant", this.#identity.tenant);
        if (this.#identity.roles?.length) {
            md.set("slate-roles", this.#identity.roles.join(","));
        }
        return md;
    }
    /** @internal */
    call(method, request) {
        return new Promise((resolve, reject) => {
            const fn = this.#raw[method];
            if (!fn) {
                reject(new Error(`slate: this server has no ${method} method`));
                return;
            }
            fn.call(this.#raw, request, this.metadata(), (error, response) => {
                if (error)
                    reject(fromServiceError(error));
                else
                    resolve(response);
            });
        });
    }
    /** @internal */
    stream(method, request) {
        const fn = this.#raw[method];
        if (!fn)
            throw new Error(`slate: this server has no ${method} method`);
        return fn.call(this.#raw, request, this.metadata());
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
    #client;
    #monotonic;
    #watermark;
    /** @internal */
    constructor(client, monotonic) {
        this.#client = client;
        this.#monotonic = monotonic;
    }
    /** The furthest this session has read or written. */
    get watermark() {
        return this.#watermark;
    }
    /** Fold an external token in, for carrying a position between sessions. */
    observe(token) {
        if (this.#watermark === undefined || token > this.#watermark) {
            this.#watermark = token;
        }
    }
    #freshness() {
        if (!this.#monotonic || this.#watermark === undefined)
            return undefined;
        return { atLeast: this.#watermark.toString() };
    }
    #observeServedBy(servedBy) {
        if (!servedBy || typeof servedBy !== "object")
            return;
        const seq = servedBy.sequence;
        if (seq !== undefined && seq !== null)
            this.observe(BigInt(seq));
    }
    async #write(method, request) {
        const response = await this.#client.call(method, request);
        const affected = BigInt(response.affected ?? 0);
        if (response.sequence !== undefined && response.sequence !== null) {
            const token = BigInt(response.sequence);
            this.observe(token);
            return { sequence: token, affected };
        }
        return { affected };
    }
    /** Add rows, refusing a primary key that is taken. */
    insert(table, ...rows) {
        return this.#write("Insert", {
            table,
            rows: rows.map(rowToWire),
            schema: this.#client.claim(table),
        });
    }
    /** Add rows, replacing any whose primary key is taken. */
    upsert(table, ...rows) {
        return this.#write("Insert", {
            table,
            rows: rows.map(rowToWire),
            upsert: true,
            schema: this.#client.claim(table),
        });
    }
    /** Replace rows, refusing one whose primary key is not there. */
    update(table, ...rows) {
        return this.#write("Update", {
            table,
            rows: rows.map(rowToWire),
            schema: this.#client.claim(table),
        });
    }
    /** Remove rows by primary key. */
    delete(table, ...keys) {
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
    async get(table, key) {
        const response = await this.#client.call("Get", {
            table,
            primaryKey: rowToWire(key),
            freshness: this.#freshness(),
            schema: this.#client.claim(table),
        });
        this.#observeServedBy(response.servedBy);
        return response.found ? rowFromWire(response.row) : undefined;
    }
    /** Read rows. */
    query(query) {
        const stream = this.#client.stream("Query", {
            query: queryToWire(query, this.#client.claim(query.table)),
            freshness: this.#freshness(),
        });
        return new RowStream(stream, (sb) => this.#observeServedBy(sb));
    }
    /** Read joined rows. */
    join(join) {
        const stream = this.#client.stream("Join", {
            join: joinToWire(join, (table) => this.#client.claim(table)),
            freshness: this.#freshness(),
        });
        return new JoinStream(stream, (sb) => this.#observeServedBy(sb));
    }
    /** Group one table. */
    aggregate(over, grouping) {
        const query = applyGrouping({ input: queryToWire(over, this.#client.claim(over.table)) }, grouping);
        return this.#aggregate(query, undefined);
    }
    /**
     * Group a join.
     *
     * Two inputs exactly: the kernel groups a two-table join and does not group
     * a chain. A third is refused by the server with that as the reason, rather
     * than counted here where the count could drift from the kernel's.
     */
    aggregateJoin(over, grouping) {
        const query = applyGrouping({ join: joinToWire(over, (table) => this.#client.claim(table)) }, grouping);
        return this.#aggregate(query, undefined);
    }
    /** @internal */
    aggregateIn(query, transaction) {
        return this.#aggregate(query, transaction);
    }
    #aggregate(query, transaction) {
        const request = { aggregate: query };
        if (transaction !== undefined)
            request["transaction"] = transaction;
        // A transaction's reads go to the writer and need no freshness floor.
        else
            request["freshness"] = this.#freshness();
        const stream = this.#client.stream("Aggregate", request);
        return new GroupStream(stream, (sb) => this.#observeServedBy(sb));
    }
    /**
     * Ask for a join's plan without running it.
     *
     * Needs the `explain` action on every table involved, not just one.
     */
    async explainJoin(join) {
        const r = await this.#client.call("ExplainJoin", {
            join: joinToWire(join, (table) => this.#client.claim(table)),
            freshness: this.#freshness(),
        });
        this.#observeServedBy(r["servedBy"]);
        const inputs = (r["inputs"] ?? []).map((input) => {
            const algorithm = input["algorithm"];
            return {
                plan: explanationFromWire(input["plan"] ?? {}),
                type: String(input["joinType"] ?? ""),
                algorithm: algorithm ? String(algorithm["algorithm"] ?? "") : "",
                estimatedRows: Number(input["estimatedRows"] ?? 0),
                estimatedCost: Number(input["estimatedCost"] ?? 0),
            };
        });
        return {
            inputs,
            estimatedRows: Number(r["estimatedRows"] ?? 0),
            estimatedCost: Number(r["estimatedCost"] ?? 0),
            display: String(r["display"] ?? ""),
            warnings: r["warnings"] ?? [],
        };
    }
    /**
     * Ask for a plan without running it.
     *
     * Requires the `explain` action on every table involved, which a `read`
     * grant does not carry: a plan is costed against statistics covering rows
     * the caller's policy may hide.
     */
    async explain(query) {
        const r = await this.#client.call("Explain", {
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
    async begin() {
        const response = await this.#client.call("Begin", {});
        return new Transaction(this, this.#client, response.transaction);
    }
    /** @internal */
    get client() {
        return this.#client;
    }
    /** @internal */
    writeThrough(method, request) {
        return this.#write(method, request);
    }
}
/** A set of operations that commit or roll back together. */
export class Transaction {
    #session;
    #client;
    #id;
    #done = false;
    /** @internal */
    constructor(session, client, id) {
        this.#session = session;
        this.#client = client;
        this.#id = id;
    }
    /** The server's handle for this transaction. */
    get id() {
        return this.#id;
    }
    /** Make the transaction's writes visible. */
    async commit() {
        if (this.#done)
            throw new Error("slate: this transaction is already finished");
        this.#done = true;
        const response = await this.#client.call("Commit", {
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
    async rollback() {
        if (this.#done)
            return;
        this.#done = true;
        await this.#client.call("Rollback", { transaction: this.#id });
    }
    /** Add rows inside the transaction. */
    insert(table, ...rows) {
        return this.#session.writeThrough("Insert", {
            transaction: this.#id,
            table,
            rows: rows.map(rowToWire),
            schema: this.#client.claim(table),
        });
    }
    /** Add or replace rows inside the transaction. */
    upsert(table, ...rows) {
        return this.#session.writeThrough("Insert", {
            transaction: this.#id,
            table,
            rows: rows.map(rowToWire),
            upsert: true,
            schema: this.#client.claim(table),
        });
    }
    /** Replace rows inside the transaction. */
    update(table, ...rows) {
        return this.#session.writeThrough("Update", {
            transaction: this.#id,
            table,
            rows: rows.map(rowToWire),
            schema: this.#client.claim(table),
        });
    }
    /** Remove rows by primary key inside the transaction. */
    delete(table, ...keys) {
        return this.#session.writeThrough("Delete", {
            transaction: this.#id,
            table,
            primaryKeys: keys.map(rowToWire),
            schema: this.#client.claim(table),
        });
    }
    /** Read one row inside the transaction, seeing its uncommitted writes. */
    async get(table, key) {
        const response = await this.#client.call("Get", {
            transaction: this.#id,
            table,
            primaryKey: rowToWire(key),
            schema: this.#client.claim(table),
        });
        return response.found ? rowFromWire(response.row) : undefined;
    }
    /** Read joined rows inside the transaction. */
    join(join) {
        const stream = this.#client.stream("Join", {
            transaction: this.#id,
            join: joinToWire(join, (table) => this.#client.claim(table)),
        });
        return new JoinStream(stream, () => { });
    }
    /** Group one table inside the transaction. */
    aggregate(over, grouping) {
        return this.#session.aggregateIn(applyGrouping({ input: queryToWire(over, this.#client.claim(over.table)) }, grouping), this.#id);
    }
    /** Group a join inside the transaction. */
    aggregateJoin(over, grouping) {
        return this.#session.aggregateIn(applyGrouping({ join: joinToWire(over, (table) => this.#client.claim(table)) }, grouping), this.#id);
    }
    /** Read rows inside the transaction. */
    query(query) {
        const stream = this.#client.stream("Query", {
            transaction: this.#id,
            query: queryToWire(query, this.#client.claim(query.table)),
        });
        return new RowStream(stream, () => { });
    }
}
/**
 * Rows arriving in batches.
 *
 * Async-iterable. Break out of the loop and the call is cancelled; a stream
 * abandoned without that keeps the server producing rows nobody will read.
 */
export class RowStream {
    #stream;
    #onServedBy;
    #servedBy;
    #warnings = [];
    /** @internal */
    constructor(stream, onServedBy) {
        this.#stream = stream;
        this.#onServedBy = onServedBy;
    }
    /** Which store answered, known once the first message has arrived. */
    get servedBy() {
        return this.#servedBy;
    }
    /** What the server said about the request it served. First message only. */
    get warnings() {
        return this.#warnings;
    }
    /** Cancel the call. */
    cancel() {
        this.#stream.cancel();
    }
    async *[Symbol.asyncIterator]() {
        try {
            for await (const message of this.#stream) {
                const servedBy = message["servedBy"];
                if (servedBy && !this.#servedBy) {
                    const sb = servedBy;
                    this.#servedBy = {
                        replica: sb.replica ?? "",
                        sequence: BigInt(sb.sequence ?? 0),
                    };
                    this.#onServedBy(servedBy);
                }
                const warnings = message["warnings"];
                if (warnings?.length)
                    this.#warnings.push(...warnings);
                for (const row of message["rows"] ?? []) {
                    yield rowFromWire(row);
                }
            }
        }
        catch (error) {
            if (isServiceError(error))
                throw fromServiceError(error);
            throw error;
        }
    }
    /**
     * Drain into an array.
     *
     * For an answer known to be small. Iterate a large table instead.
     */
    async collect() {
        const out = [];
        for await (const row of this)
            out.push(row);
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
export class JoinStream {
    #stream;
    #onServedBy;
    #servedBy;
    #warnings = [];
    /** @internal */
    constructor(stream, onServedBy) {
        this.#stream = stream;
        this.#onServedBy = onServedBy;
    }
    /** Which store answered, known once the first message has arrived. */
    get servedBy() {
        return this.#servedBy;
    }
    /** What the server said about the request it served. */
    get warnings() {
        return this.#warnings;
    }
    /** Cancel the call. */
    cancel() {
        this.#stream.cancel();
    }
    async *[Symbol.asyncIterator]() {
        try {
            for await (const message of this.#stream) {
                this.#note(message);
                for (const joined of message["rows"] ?? []) {
                    yield (joined.inputs ?? []).map((input) => {
                        const row = input.row;
                        return row ? rowFromWire(row) : undefined;
                    });
                }
            }
        }
        catch (error) {
            if (isServiceError(error))
                throw fromServiceError(error);
            throw error;
        }
    }
    #note(message) {
        const servedBy = message["servedBy"];
        if (servedBy && !this.#servedBy) {
            const sb = servedBy;
            this.#servedBy = { replica: sb.replica ?? "", sequence: BigInt(sb.sequence ?? 0) };
            this.#onServedBy(servedBy);
        }
        const warnings = message["warnings"];
        if (warnings?.length)
            this.#warnings.push(...warnings);
    }
    /** Drain into an array. */
    async collect() {
        const out = [];
        for await (const row of this)
            out.push(row);
        return out;
    }
}
/** Groups arriving in batches. */
export class GroupStream {
    #stream;
    #onServedBy;
    #servedBy;
    #warnings = [];
    /** @internal */
    constructor(stream, onServedBy) {
        this.#stream = stream;
        this.#onServedBy = onServedBy;
    }
    /** Which store answered. */
    get servedBy() {
        return this.#servedBy;
    }
    /** What the server said about the request it served. */
    get warnings() {
        return this.#warnings;
    }
    /** Cancel the call. */
    cancel() {
        this.#stream.cancel();
    }
    async *[Symbol.asyncIterator]() {
        try {
            for await (const message of this.#stream) {
                const servedBy = message["servedBy"];
                if (servedBy && !this.#servedBy) {
                    const sb = servedBy;
                    this.#servedBy = { replica: sb.replica ?? "", sequence: BigInt(sb.sequence ?? 0) };
                    this.#onServedBy(servedBy);
                }
                const warnings = message["warnings"];
                if (warnings?.length)
                    this.#warnings.push(...warnings);
                for (const group of message["groups"] ?? []) {
                    yield {
                        key: (group["key"] ?? []).map(valueFromWire),
                        values: (group["values"] ?? []).map(valueFromWire),
                    };
                }
            }
        }
        catch (error) {
            if (isServiceError(error))
                throw fromServiceError(error);
            throw error;
        }
    }
    /** Drain into an array. */
    async collect() {
        const out = [];
        for await (const group of this)
            out.push(group);
        return out;
    }
}
function explanationFromWire(r) {
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
        warnings: r["warnings"] ?? [],
    };
}
function isServiceError(error) {
    return (typeof error === "object" &&
        error !== null &&
        "code" in error &&
        typeof error.code === "number");
}
export { SlateError };
