import { type Expr, type Ordinal, type Direction } from "./query.js";
import { type Value } from "./value.js";
/**
 * One input's column: which input, and that input's own ordinal.
 *
 * Not a cumulative offset into a flattened joined row. The wire carries the
 * input index alongside the ordinal, so a column of the second table is
 * `at(1, 2)` — the third column of *that* table — rather than "first table's
 * width plus two". There is no arithmetic to do and no table width to know,
 * which is why this client needs no catalog.
 */
export interface Column {
    readonly input: number;
    readonly ordinal: Ordinal;
}
/** Names column `ordinal` of input `input`. */
export declare const at: (input: number, ordinal: Ordinal) => Column;
/**
 * Names column `ordinal` of the only input, for grouping one table.
 *
 * `at(0, ordinal)` says the same thing; this reads better where there is no
 * join and no second input to distinguish from.
 */
export declare const key0: (ordinal: Ordinal) => Column;
/** Which rows survive a join. */
export type JoinType = "inner" | "left" | "right" | "full";
/**
 * A join strategy forced in place of the planner's choice.
 *
 * A test aid and an escape hatch, not a tuning knob: the planner costs these
 * and is usually right. Forcing one is how an oracle checks that every
 * algorithm agrees, which is why it is reachable at all.
 */
export type Algorithm = "hash-build-left" | "hash-build-right" | "nested-loop";
/**
 * One equality of a join condition.
 *
 * `earlier` must name an input already read — the server refuses one that does
 * not, by position rather than by guessing. `own` is an ordinal of *this*
 * input's table and needs no input index: which input it belongs to is the
 * input it is written on.
 */
export interface On {
    readonly earlier: Column;
    readonly own: Ordinal;
}
/** One table entering a join. */
export interface JoinInput {
    readonly table: string;
    /** Admits rows of this input, before the join. */
    readonly filter?: Expr;
    /** Equates this input's columns with earlier ones. Empty for the first. */
    readonly on?: On[];
    /** How unmatched rows are treated. Ignored on the first input. */
    readonly type?: JoinType;
    /**
     * A condition evaluated after this input joins.
     *
     * May name any input read so far, which is what makes a chain more than
     * nested pairs.
     */
    readonly having?: Expr;
    /** Overrides the planner's algorithm choice. */
    readonly force?: Algorithm;
    /** This input's projection. Absent means every column. */
    readonly columns?: Ordinal[];
    /** Reads this input backwards where the access path allows it. */
    readonly descending?: boolean;
}
/**
 * A join of two or more tables.
 *
 * No sort on an input: the kernel documents input-level sorting as ignored and
 * the server refuses it, so there is nowhere here to set one and the refusal
 * is unreachable rather than a runtime surprise. The limit and offset that do
 * apply are here.
 */
export interface JoinQuery {
    readonly inputs: JoinInput[];
    readonly limit?: number | bigint;
    readonly offset?: number | bigint;
    /**
     * Caps rows held in a hash build side. The server clamps a value above its
     * own ceiling and says so in a warning.
     */
    readonly buildLimit?: number | bigint;
}
/**
 * Accumulates inputs and hands out their positions.
 *
 * An input's position is assigned in call order, which is the order the wire
 * declares them in. This exists so the position is never written by hand: a
 * literal `1` that goes stale when an input is inserted above it is a join
 * that silently answers a different question.
 */
export declare class JoinBuilder {
    #private;
    /** Appends an input and returns its position. */
    add(input: JoinInput): number;
    /** The join built so far. */
    query(extra?: Omit<JoinQuery, "inputs">): JoinQuery;
    /** How many inputs have been added. */
    get inputs(): number;
}
/** Starts a join. */
export declare const newJoin: () => JoinBuilder;
/**
 * A join in its wire form.
 *
 * `claim` supplies each input's declaration by table name, so a join checks
 * every table it reads rather than none of them.
 */
export declare function joinToWire(join: JoinQuery, claim?: (table: string) => Record<string, unknown> | undefined): Record<string, unknown>;
/** What an aggregate computes. */
export type AggregateFunction = "count" | "count-column" | "count-distinct" | "min" | "max" | "sum" | "avg";
/** One computed value over a group. */
export interface Aggregate {
    readonly function: AggregateFunction;
    /** What it reads. Absent for `count`, required by the rest. */
    readonly column?: Column;
}
/** `COUNT(*)`: every row, nulls included. */
export declare const count: () => Aggregate;
/** `COUNT(column)`, which skips nulls. */
export declare const countOf: (column: Column) => Aggregate;
/** `COUNT(DISTINCT column)`. */
export declare const countDistinctOf: (column: Column) => Aggregate;
/** `MIN(column)`. */
export declare const minOf: (column: Column) => Aggregate;
/** `MAX(column)`. */
export declare const maxOf: (column: Column) => Aggregate;
/** `SUM(column)`. */
export declare const sumOf: (column: Column) => Aggregate;
/** `AVG(column)`. */
export declare const avgOf: (column: Column) => Aggregate;
/**
 * Names a column of the *grouped result* — a key or an aggregate — for a
 * `having` and for the ordering over groups.
 *
 * A raw table column there is a kind mismatch the server refuses by name:
 * SQL's "column must appear in the GROUP BY clause", made decidable by the
 * wire carrying the kind rather than inferring it.
 */
export interface GroupRef {
    readonly wire: Record<string, unknown>;
}
/** The `index`th GROUP BY key. */
export declare const groupKey: (index: number) => GroupRef;
/** The `index`th aggregate. */
export declare const agg: (index: number) => GroupRef;
/** One column of an ordering over groups. */
export interface GroupSortKey {
    readonly column: GroupRef;
    readonly direction?: Direction;
}
/**
 * Comparisons over a grouped result, for `having`.
 *
 * Separate from the row-level `eq` and friends because they take a [GroupRef]
 * rather than an [Ordinal]. Two families rather than one overloaded family
 * means the wrong one does not typecheck, instead of being refused at runtime
 * by a server the caller has already deployed against.
 */
export declare const groupEq: (r: GroupRef, v: Value) => Expr;
/** `<> value` over a group. */
export declare const groupNe: (r: GroupRef, v: Value) => Expr;
/** `< value` over a group. */
export declare const groupLt: (r: GroupRef, v: Value) => Expr;
/** `<= value` over a group. */
export declare const groupLe: (r: GroupRef, v: Value) => Expr;
/** `> value` over a group. */
export declare const groupGt: (r: GroupRef, v: Value) => Expr;
/** `>= value` over a group. */
export declare const groupGe: (r: GroupRef, v: Value) => Expr;
/**
 * A set of aggregates, optionally per group.
 *
 * `sort`, `limit` and `offset` are over *groups*, not the rows going into
 * them: ordering the input rows would change nothing and cutting them would
 * change the answer in a way nobody means.
 */
export interface Grouping {
    /** The key columns. Empty means one group over every row. */
    readonly groupBy?: Column[];
    /** What to compute. At least one. */
    readonly aggregates: Aggregate[];
    /** Keeps groups. Names keys and aggregates, not columns. */
    readonly having?: Expr;
    /** Orders the groups. */
    readonly sort?: GroupSortKey[];
    /** Caps the groups returned. */
    readonly limit?: number | bigint;
    /** Discards groups before the limit applies. */
    readonly offset?: number | bigint;
}
/** Applies a grouping onto an `AggregateQuery` in its wire form. */
export declare function applyGrouping(out: Record<string, unknown>, grouping: Grouping): Record<string, unknown>;
/** One row of a grouped answer. */
export interface Group {
    /** The group's key values, in `groupBy` order. */
    readonly key: Value[];
    /** The aggregates, in `aggregates` order. */
    readonly values: Value[];
}
