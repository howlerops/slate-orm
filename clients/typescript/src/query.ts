import { type Value, valueToWire } from "./value.js";

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

const columnRef = (column: Ordinal) => ({ column });

/** Admits every row. */
export const alwaysTrue = (): Expr => ({ wire: { literal: true } });

/** Admits none. */
export const alwaysFalse = (): Expr => ({ wire: { literal: false } });

const compare = (column: Ordinal, op: string, value: Value): Expr => ({
  wire: { compare: { column: columnRef(column), op, value: valueToWire(value) } },
});

/** `column = value`. */
export const eq = (c: Ordinal, v: Value): Expr => compare(c, "CMP_OP_EQ", v);
/** `column <> value`. */
export const ne = (c: Ordinal, v: Value): Expr => compare(c, "CMP_OP_NE", v);
/** `column < value`. */
export const lt = (c: Ordinal, v: Value): Expr => compare(c, "CMP_OP_LT", v);
/** `column <= value`. */
export const le = (c: Ordinal, v: Value): Expr => compare(c, "CMP_OP_LE", v);
/** `column > value`. */
export const gt = (c: Ordinal, v: Value): Expr => compare(c, "CMP_OP_GT", v);
/** `column >= value`. */
export const ge = (c: Ordinal, v: Value): Expr => compare(c, "CMP_OP_GE", v);

/** `column IS NULL`. */
export const isNull = (column: Ordinal): Expr => ({
  wire: { isNull: { column: columnRef(column), negated: false } },
});

/** `column IS NOT NULL`. */
export const isNotNull = (column: Ordinal): Expr => ({
  wire: { isNull: { column: columnRef(column), negated: true } },
});

/** `column IN (values)`. */
export const isIn = (column: Ordinal, values: Value[]): Expr => ({
  wire: { inList: { column: columnRef(column), values: values.map(valueToWire) } },
});

const likeExpr = (
  column: Ordinal,
  pattern: string,
  negated: boolean,
  insensitive: boolean,
): Expr => ({
  wire: { like: { column: columnRef(column), pattern, negated, insensitive } },
});

/** `column LIKE pattern`, with `%` and `_` as the wildcards. */
export const like = (c: Ordinal, p: string): Expr => likeExpr(c, p, false, false);
/** `column ILIKE pattern`, matching without regard to case. */
export const ilike = (c: Ordinal, p: string): Expr => likeExpr(c, p, false, true);
/** `column NOT LIKE pattern`. */
export const notLike = (c: Ordinal, p: string): Expr => likeExpr(c, p, true, false);

/**
 * The conjunction of every part.
 *
 * With no parts this is [alwaysTrue], the identity for `AND`, so a filter
 * built up in a loop that adds nothing does not become `false` by accident.
 */
export const and = (...parts: Expr[]): Expr =>
  parts.length === 0
    ? alwaysTrue()
    : { wire: { conjunction: { exprs: parts.map((p) => p.wire) } } };

/** The disjunction. With no parts this is [alwaysFalse], the identity for `OR`. */
export const or = (...parts: Expr[]): Expr =>
  parts.length === 0
    ? alwaysFalse()
    : { wire: { disjunction: { exprs: parts.map((p) => p.wire) } } };

/** Inverts a predicate. */
export const not = (inner: Expr): Expr => ({ wire: { negation: inner.wire } });

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

/**
 * A query in its wire form.
 *
 * `claim` is the caller's declaration of the table, or undefined where it has
 * none — attached here rather than by each call site, because a read that
 * forgets it is a read that is silently unchecked.
 */
export function queryToWire(
  query: Query,
  claim?: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = { table: query.table };
  if (claim) out["schema"] = claim;
  if (query.filter) out["filter"] = query.filter.wire;
  if (query.descending) out["order"] = "SCAN_ORDER_DESCENDING";
  if (query.limit !== undefined) out["limit"] = String(query.limit);
  if (query.offset !== undefined) out["offset"] = String(query.offset);
  if (query.columns && query.columns.length > 0) {
    out["projection"] = { columns: query.columns.map(columnRef) };
  }
  if (query.sort && query.sort.length > 0) {
    out["sort"] = query.sort.map((key) => ({
      column: columnRef(key.column),
      direction:
        key.direction === "desc" ? "SORT_DIRECTION_DESC" : "SORT_DIRECTION_ASC",
    }));
  }
  return out;
}
