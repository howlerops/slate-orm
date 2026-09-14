/**
 * Table declarations, and the fingerprint the server checks them by.
 *
 * Optional. Everything in this package works without one — the wire carries
 * ordinals and this client resolves nothing. What a declaration buys is the
 * *check*: a request carrying one is refused if the server's catalog disagrees,
 * instead of being answered with the wrong column.
 *
 * That matters most for the mistake ordinals make easy. Declaring
 * `{id, title, year}` for a table that is really `{id, year, title}` produces a
 * client that reads titles as years, silently and forever. With a declaration
 * the first request says so.
 */
/** A column's declared type, as the catalog spells it. */
export type ColumnType = "bool" | "bytes" | "string" | "i64" | "u64" | "f64" | "uuid" | "vector";
/** One column of a [TableDef]. */
export interface ColumnDef {
    readonly name: string;
    readonly type: ColumnType;
}
/** This client's declaration of a table. */
export interface TableDef {
    /** The name, as the catalog spells it. */
    readonly name: string;
    /** Columns in ordinal order. `columns[2]` *is* ordinal 2. */
    readonly columns: ColumnDef[];
    /** The key columns, in key order. */
    readonly primaryKey: string[];
}
/** Declarations by table name. A table with no entry sends no claim. */
export type Schemas = Record<string, TableDef>;
/** The position of a named column, or -1. */
export declare function ordinalOf(table: TableDef, name: string): number;
/**
 * This client's claim about the table, in the server's canonical form.
 *
 * Only what a client can address and can be wrong about: the table's name, and
 * per ordinal the column's name and declared type, plus the primary key and the
 * column count. Nullability, `DEFAULT`, `CHECK`, foreign keys and indexes
 * address no column and are deliberately absent — hashing them would make an
 * unrelated migration break every client, which is the failure mode that makes
 * a fingerprint worse than none.
 */
export declare function fingerprint(table: TableDef): bigint;
/** The wire form of a claim, or undefined for an undeclared table. */
export declare function claimFor(schemas: Schemas | undefined, table: string): Record<string, unknown> | undefined;
