import * as grpc from "@grpc/grpc-js";
import { SlateError } from "./errors.js";
import { type Group, type Grouping, type JoinQuery } from "./join.js";
import { type Query } from "./query.js";
import { type Value } from "./value.js";
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
/** A connection to a head node. Safe to share; a [Session] is not. */
export declare class Client {
    #private;
    private constructor();
    /**
     * Connect to a head node.
     *
     * Insecure by default and deliberately: this client is meant to sit behind
     * the same proxy that sets its identity headers, and offering TLS here would
     * suggest the identity was protected by something. Pass `credentials` to
     * override.
     */
    static connect(target: string, identity: Identity, credentials?: grpc.ChannelCredentials): Client;
    /** Release the connection. */
    close(): void;
    /** Start a session. Monotonic reads are on, which is the safe default. */
    session(): Session;
    /**
     * Start one that does not carry its watermark.
     *
     * For a caller that wants the cheapest read available and has decided going
     * backwards in time is acceptable. Named at length so it is a decision.
     */
    sessionWithoutMonotonicReads(): Session;
    /**
     * Ask a node about the writer lease.
     *
     * Answered by any node, leader or not, which is the point: a follower's
     * answer is how a caller finds the leader.
     */
    leadership(): Promise<Leadership>;
    /** @internal */
    metadata(): grpc.Metadata;
    /** @internal */
    call<T>(method: string, request: unknown): Promise<T>;
    /** @internal */
    stream(method: string, request: unknown): grpc.ClientReadableStream<unknown>;
}
/**
 * A sequence of operations that keeps its own read position.
 *
 * Reads through one session never go backwards in time: a read after a write
 * sees that write. Without it a replica read can be served by a node that has
 * not caught up — correct, and surprising.
 */
export declare class Session {
    #private;
    /** @internal */
    constructor(client: Client, monotonic: boolean);
    /** The furthest this session has read or written. */
    get watermark(): ReadToken | undefined;
    /** Fold an external token in, for carrying a position between sessions. */
    observe(token: ReadToken): void;
    /** Add rows, refusing a primary key that is taken. */
    insert(table: string, ...rows: Value[][]): Promise<WriteResult>;
    /** Add rows, replacing any whose primary key is taken. */
    upsert(table: string, ...rows: Value[][]): Promise<WriteResult>;
    /** Replace rows, refusing one whose primary key is not there. */
    update(table: string, ...rows: Value[][]): Promise<WriteResult>;
    /** Remove rows by primary key. */
    delete(table: string, ...keys: Value[][]): Promise<WriteResult>;
    /**
     * Read one row by primary key.
     *
     * `undefined` for a row that is not there rather than an error: checking
     * existence should not mean catching an exception.
     */
    get(table: string, key: Value[]): Promise<Value[] | undefined>;
    /** Read rows. */
    query(query: Query): RowStream;
    /** Read joined rows. */
    join(join: JoinQuery): JoinStream;
    /** Group one table. */
    aggregate(over: Query, grouping: Grouping): GroupStream;
    /**
     * Group a join.
     *
     * Two inputs exactly: the kernel groups a two-table join and does not group
     * a chain. A third is refused by the server with that as the reason, rather
     * than counted here where the count could drift from the kernel's.
     */
    aggregateJoin(over: JoinQuery, grouping: Grouping): GroupStream;
    /** @internal */
    aggregateIn(query: Record<string, unknown>, transaction: string): GroupStream;
    /**
     * Ask for a join's plan without running it.
     *
     * Needs the `explain` action on every table involved, not just one.
     */
    explainJoin(join: JoinQuery): Promise<JoinExplanation>;
    /**
     * Ask for a plan without running it.
     *
     * Requires the `explain` action on every table involved, which a `read`
     * grant does not carry: a plan is costed against statistics covering rows
     * the caller's policy may hide.
     */
    explain(query: Query): Promise<Explanation>;
    /**
     * Open a transaction.
     *
     * Every operation on it goes to the writer, and its reads see its own
     * uncommitted writes. Commit or roll back: an abandoned transaction holds a
     * slot until the node's idle timeout collects it.
     */
    begin(): Promise<Transaction>;
    /** @internal */
    get client(): Client;
    /** @internal */
    writeThrough(method: string, request: Record<string, unknown>): Promise<WriteResult>;
}
/** A set of operations that commit or roll back together. */
export declare class Transaction {
    #private;
    /** @internal */
    constructor(session: Session, client: Client, id: string);
    /** The server's handle for this transaction. */
    get id(): string;
    /** Make the transaction's writes visible. */
    commit(): Promise<void>;
    /**
     * Discard the transaction's writes.
     *
     * Quiet after a commit, which is what makes a `finally { await tx.rollback() }`
     * the right shape for every transaction.
     */
    rollback(): Promise<void>;
    /** Add rows inside the transaction. */
    insert(table: string, ...rows: Value[][]): Promise<WriteResult>;
    /** Add or replace rows inside the transaction. */
    upsert(table: string, ...rows: Value[][]): Promise<WriteResult>;
    /** Replace rows inside the transaction. */
    update(table: string, ...rows: Value[][]): Promise<WriteResult>;
    /** Remove rows by primary key inside the transaction. */
    delete(table: string, ...keys: Value[][]): Promise<WriteResult>;
    /** Read one row inside the transaction, seeing its uncommitted writes. */
    get(table: string, key: Value[]): Promise<Value[] | undefined>;
    /** Read joined rows inside the transaction. */
    join(join: JoinQuery): JoinStream;
    /** Group one table inside the transaction. */
    aggregate(over: Query, grouping: Grouping): GroupStream;
    /** Group a join inside the transaction. */
    aggregateJoin(over: JoinQuery, grouping: Grouping): GroupStream;
    /** Read rows inside the transaction. */
    query(query: Query): RowStream;
}
/**
 * Rows arriving in batches.
 *
 * Async-iterable. Break out of the loop and the call is cancelled; a stream
 * abandoned without that keeps the server producing rows nobody will read.
 */
export declare class RowStream implements AsyncIterable<Value[]> {
    #private;
    /** @internal */
    constructor(stream: grpc.ClientReadableStream<unknown>, onServedBy: (servedBy: unknown) => void);
    /** Which store answered, known once the first message has arrived. */
    get servedBy(): ServedBy | undefined;
    /** What the server said about the request it served. First message only. */
    get warnings(): readonly string[];
    /** Cancel the call. */
    cancel(): void;
    [Symbol.asyncIterator](): AsyncIterator<Value[]>;
    /**
     * Drain into an array.
     *
     * For an answer known to be small. Iterate a large table instead.
     */
    collect(): Promise<Value[][]>;
}
/**
 * Joined rows arriving in batches.
 *
 * A joined row is one array of values **per input**, `undefined` where an
 * outer join found no match — kept separate rather than concatenated, because
 * a flat row cannot tell "the right side had no match" from "the right side
 * matched and its columns are null".
 */
export declare class JoinStream implements AsyncIterable<(Value[] | undefined)[]> {
    #private;
    /** @internal */
    constructor(stream: grpc.ClientReadableStream<unknown>, onServedBy: (servedBy: unknown) => void);
    /** Which store answered, known once the first message has arrived. */
    get servedBy(): ServedBy | undefined;
    /** What the server said about the request it served. */
    get warnings(): readonly string[];
    /** Cancel the call. */
    cancel(): void;
    [Symbol.asyncIterator](): AsyncIterator<(Value[] | undefined)[]>;
    /** Drain into an array. */
    collect(): Promise<(Value[] | undefined)[][]>;
}
/** Groups arriving in batches. */
export declare class GroupStream implements AsyncIterable<Group> {
    #private;
    /** @internal */
    constructor(stream: grpc.ClientReadableStream<unknown>, onServedBy: (servedBy: unknown) => void);
    /** Which store answered. */
    get servedBy(): ServedBy | undefined;
    /** What the server said about the request it served. */
    get warnings(): readonly string[];
    /** Cancel the call. */
    cancel(): void;
    [Symbol.asyncIterator](): AsyncIterator<Group>;
    /** Drain into an array. */
    collect(): Promise<Group[]>;
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
export { SlateError };
