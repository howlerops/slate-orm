import { type Value } from "./value.js";
/**
 * A column's position in its table, counting from zero.
 *
 * Positions rather than names, because that is what the wire carries.
 * Resolving a name would mean this client holding a second copy of the schema
 * that can disagree with the server's.
 */
export type Ordinal = number;
/** A predicate over one table's rows. */
export interface Expr {
    readonly wire: Record<string, unknown>;
}
/** Admits every row. */
export declare const alwaysTrue: () => Expr;
/** Admits none. */
export declare const alwaysFalse: () => Expr;
/** `column = value`. */
export declare const eq: (c: Ordinal, v: Value) => Expr;
/** `column <> value`. */
export declare const ne: (c: Ordinal, v: Value) => Expr;
/** `column < value`. */
export declare const lt: (c: Ordinal, v: Value) => Expr;
/** `column <= value`. */
export declare const le: (c: Ordinal, v: Value) => Expr;
/** `column > value`. */
export declare const gt: (c: Ordinal, v: Value) => Expr;
/** `column >= value`. */
export declare const ge: (c: Ordinal, v: Value) => Expr;
/** `column IS NULL`. */
export declare const isNull: (column: Ordinal) => Expr;
/** `column IS NOT NULL`. */
export declare const isNotNull: (column: Ordinal) => Expr;
/** `column IN (values)`. */
export declare const isIn: (column: Ordinal, values: Value[]) => Expr;
/** `column LIKE pattern`, with `%` and `_` as the wildcards. */
export declare const like: (c: Ordinal, p: string) => Expr;
/** `column ILIKE pattern`, matching without regard to case. */
export declare const ilike: (c: Ordinal, p: string) => Expr;
/** `column NOT LIKE pattern`. */
export declare const notLike: (c: Ordinal, p: string) => Expr;
/**
 * The conjunction of every part.
 *
 * With no parts this is [alwaysTrue], the identity for `AND`, so a filter
 * built up in a loop that adds nothing does not become `false` by accident.
 */
export declare const and: (...parts: Expr[]) => Expr;
/** The disjunction. With no parts this is [alwaysFalse], the identity for `OR`. */
export declare const or: (...parts: Expr[]) => Expr;
/** Inverts a predicate. */
export declare const not: (inner: Expr) => Expr;
/** Which way a sort key orders. */
export type Direction = "asc" | "desc";
/** One column of an ordering. */
export interface SortKey {
    readonly column: Ordinal;
    readonly direction?: Direction;
}
/** Selects rows from one table. */
export interface Query {
    /** The table's name, as the server's catalog spells it. */
    readonly table: string;
    /** Admits rows. Absent means every row. */
    readonly filter?: Expr;
    /** Orders the answer. Absent is the access path's own order. */
    readonly sort?: SortKey[];
    /** Caps the rows returned. */
    readonly limit?: number | bigint;
    /** Discards rows before the limit applies. */
    readonly offset?: number | bigint;
    /**
     * The projection. Absent means every column.
     *
     * Naming fewer is what lets an index answer without reading a row, so it is
     * worth naming them where a caller knows. Columns outside it come back null
     * rather than absent, so an ordinal still means what it meant.
     */
    readonly columns?: Ordinal[];
    /** Reads the table backwards where the access path allows it. */
    readonly descending?: boolean;
}
/** A query in its wire form. */
export declare function queryToWire(query: Query): Record<string, unknown>;
