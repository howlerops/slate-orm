import { type Column, columnWire } from "./join.js";
import { type Scalar, scalarsToWire } from "./scalar.js";
import { rowToWire, type Value, valueToWire } from "./value.js";

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

const compareColumn = (column: Ordinal, op: string, value: Value): Expr => ({
  wire: { compare: { column: columnRef(column), op, value: valueToWire(value) } },
});

/** `column = value`. */
export const eq = (c: Ordinal, v: Value): Expr => compareColumn(c, "CMP_OP_EQ", v);
/** `column <> value`. */
export const ne = (c: Ordinal, v: Value): Expr => compareColumn(c, "CMP_OP_NE", v);
/** `column < value`. */
export const lt = (c: Ordinal, v: Value): Expr => compareColumn(c, "CMP_OP_LT", v);
/** `column <= value`. */
export const le = (c: Ordinal, v: Value): Expr => compareColumn(c, "CMP_OP_LE", v);
/** `column > value`. */
export const gt = (c: Ordinal, v: Value): Expr => compareColumn(c, "CMP_OP_GT", v);
/** `column >= value`. */
export const ge = (c: Ordinal, v: Value): Expr => compareColumn(c, "CMP_OP_GE", v);

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

/**
 * A comparison operator, for {@link compare}.
 *
 * The `eq` family covers the common case of a stored column of the query's own
 * table and takes a bare {@link Ordinal}. This type exists so the same six
 * comparisons can be written against any reference — a computed value, a column
 * of another input, a value the join computed — without six more exported
 * names each.
 */
export type Operator = "eq" | "ne" | "lt" | "le" | "gt" | "ge";

const OPERATORS: Record<Operator, string> = {
  eq: "CMP_OP_EQ",
  ne: "CMP_OP_NE",
  lt: "CMP_OP_LT",
  le: "CMP_OP_LE",
  gt: "CMP_OP_GT",
  ge: "CMP_OP_GE",
};

/**
 * A comparison naming any reference: `at`, `computedAt`, `computed0` or
 * `joinComputed`.
 *
 * `compare(computed0(0), "gt", int(10))` filters on a query's first computed
 * value; `gt(2, ...)` remains the short way to say "column 2 of this table".
 */
export const compare = (column: Column, op: Operator, value: Value): Expr => ({
  wire: { compare: { column: columnWire(column), op: OPERATORS[op], value: valueToWire(value) } },
});

/** `IS NULL` over any reference. See {@link compare}. */
export const isNullAt = (column: Column): Expr => ({
  wire: { isNull: { column: columnWire(column), negated: false } },
});

/** `IS NOT NULL` over any reference. */
export const isNotNullAt = (column: Column): Expr => ({
  wire: { isNull: { column: columnWire(column), negated: true } },
});

/** Which way a sort key orders. */
export type Direction = "asc" | "desc";

/**
 * One column of an ordering.
 *
 * `column` is an ordinal of the query's own table. `ref`, when set, names any
 * reference instead — a computed value, say — and wins over `column`.
 */
export interface SortKey {
  readonly column: Ordinal;
  readonly direction?: Direction;
  /** Overrides `column` with a qualified reference. See `computed0`. */
  readonly ref?: Column;
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
  /**
   * Resumes at the first row strictly after this primary key — keyset
   * pagination. Absent or empty is the first page.
   *
   * Not `offset`, which counts rows and is only correct while nothing changes:
   * delete a row ahead of the cursor between two pages and the reader silently
   * skips one, insert one and they see a row twice, and nothing reports either.
   * A key does not move when its neighbours change. It is also cheaper —
   * `offset n` reads and discards `n` rows, where a key lets the range start
   * after the cursor, so every page costs what the first one costs.
   *
   * Use {@link Session.page}, which sets `paged` and hands back the cursor.
   *
   * Explicitly `| undefined`, unlike the other optional fields here: a caller
   * paging in a loop holds `Value[] | undefined` and passes it straight back,
   * and under `exactOptionalPropertyTypes` a bare `?:` would reject the first
   * iteration — the one where there is no cursor yet.
   */
  readonly after?: Value[] | undefined;
  /**
   * Asks the server for the cursor to the next page.
   *
   * Separate from `after`, because the first page has no cursor to resume from
   * and still wants one back. Separate from `limit`, because `LIMIT 10` and
   * "the first page of ten" are the same request and different intentions —
   * and the server refuses a page it cannot build a cursor for (no limit, or a
   * projection dropping a key column) rather than serving it without one,
   * which it can only do if it knows one was wanted.
   */
  readonly paged?: boolean;
  /**
   * Values computed per row, appended after the table's own columns and named
   * with `computed0`.
   *
   * A filter, a sort key, a GROUP BY key or an aggregate names one the same way
   * it names a column, so none of them has to learn what an expression is. The
   * `n`th may read the `n` before it and not itself or a later one.
   *
   * They come back in each row's `computed`, beside the row rather than as a
   * tail of it, so an ordinal still means a column.
   */
  readonly compute?: Scalar[];
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
      column: key.ref ? columnWire(key.ref) : columnRef(key.column),
      direction:
        key.direction === "desc" ? "SORT_DIRECTION_DESC" : "SORT_DIRECTION_ASC",
    }));
  }
  if (query.compute && query.compute.length > 0) {
    out["compute"] = scalarsToWire(query.compute);
  }
  if (query.after && query.after.length > 0) {
    out["after"] = query.after.map(valueToWire);
  }
  if (query.paged) out["paged"] = true;
  return out;
}

/**
 * Delete every row a predicate selects, in one statement.
 *
 * A plain object like `Query`, so the fields the server accepts are the fields
 * there are — rather than a builder offering a projection and a limit that a
 * predicate write would have to refuse.
 */
export interface DeleteWhere {
  /** The table's name, as the server's catalog spells it. */
  readonly table: string;
  /**
   * Admits rows. Absent means every row the caller can see — a `DELETE FROM t`
   * with no `WHERE`, which is a real statement and is allowed. Neither this
   * nor the server can tell it from the mistake it resembles.
   */
  readonly filter?: Expr;
  /**
   * Ask for the rows back, as they were before removal.
   *
   * Off by default because the rows are the whole cost: a delete that matched
   * a million rows would send a million of them back.
   */
  readonly returning?: boolean;
}

/** One column and the value to store in it, for {@link UpdateWhere}. */
export interface Assignment {
  /** The column to write, by ordinal within the table. */
  readonly column: Ordinal;
  /**
   * Evaluated over the row **as it was read**, so `add(col(views), lit(...))`
   * is one write rather than a read, a decision and a write — and two
   * concurrent increments make two.
   *
   * Every assignment in one request reads the original row, so they apply
   * together: assigning `a` from `b` and `b` from `a` swaps them rather than
   * making both `b`. Left-to-right is the other reading and it is the one that
   * surprises people; SQL takes this one and so does this.
   */
  readonly value: Scalar;
}

/** Assign to columns of every row a predicate selects. */
export interface UpdateWhere {
  /** The table's name, as the server's catalog spells it. */
  readonly table: string;
  /** Admits rows. Absent means every row the caller can see. */
  readonly filter?: Expr;
  /**
   * The assignments, in order. At least one is required: a request with none
   * is refused rather than reported as zero rows written, because zero is what
   * a predicate that matched nothing reports and the two are different
   * mistakes.
   */
  readonly set: Assignment[];
  /** Ask for the rows back, as written. */
  readonly returning?: boolean;
}

export function deleteWhereToWire(
  write: DeleteWhere,
  claim?: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = { table: write.table };
  if (claim) out["schema"] = claim;
  if (write.filter) out["filter"] = write.filter.wire;
  if (write.returning) out["returning"] = true;
  return out;
}

export function updateWhereToWire(
  write: UpdateWhere,
  claim?: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = { table: write.table };
  if (claim) out["schema"] = claim;
  if (write.filter) out["filter"] = write.filter.wire;
  if (write.returning) out["returning"] = true;
  out["assignments"] = write.set.map((a) => ({
    column: columnRef(a.column),
    value: a.value.wire,
  }));
  return out;
}

/**
 * Whether a batch's operations stand alone or land together.
 *
 * There is no default. The two guarantees differ *only when something fails*,
 * so a caller who never chose finds out on the day a write in the middle is
 * rejected — and discovers then whether the ones before it stayed.
 */
export type Atomicity = "independent" | "all-or-nothing";

/** One write in a {@link Batch}. */
export type BatchOperation =
  | { readonly kind: "insert"; readonly table: string; readonly rows: Value[][]; readonly upsert?: boolean }
  | { readonly kind: "update"; readonly table: string; readonly rows: Value[][] }
  | { readonly kind: "delete"; readonly table: string; readonly keys: Value[][] }
  | { readonly kind: "deleteWhere"; readonly write: DeleteWhere }
  | { readonly kind: "updateWhere"; readonly write: UpdateWhere };

/**
 * Several writes in one round trip.
 *
 * `atomicity` is required rather than defaulted, which is the client-side half
 * of the server refusing an unspecified one.
 */
export interface Batch {
  readonly atomicity: Atomicity;
  /** Applied in order. At least one; an empty batch is refused. */
  readonly operations: BatchOperation[];
}

const ATOMICITY_WIRE: Record<Atomicity, string> = {
  independent: "ATOMICITY_INDEPENDENT",
  "all-or-nothing": "ATOMICITY_ALL_OR_NOTHING",
};

/** The table an operation names, for decoding its returned rows. */
export function operationTable(operation: BatchOperation): string {
  return operation.kind === "deleteWhere" || operation.kind === "updateWhere"
    ? operation.write.table
    : operation.table;
}

export function batchToWire(
  batch: Batch,
  transaction: string,
  claim: (table: string) => Record<string, unknown> | undefined,
): Record<string, unknown> {
  const operations = batch.operations.map((operation) => {
    const schema = claim(operationTable(operation));
    switch (operation.kind) {
      case "insert":
        return {
          insert: {
            table: operation.table,
            rows: operation.rows.map(rowToWire),
            ...(operation.upsert ? { upsert: true } : {}),
            ...(schema ? { schema } : {}),
          },
        };
      case "update":
        return {
          update: {
            table: operation.table,
            rows: operation.rows.map(rowToWire),
            ...(schema ? { schema } : {}),
          },
        };
      case "delete":
        return {
          delete: {
            table: operation.table,
            primaryKeys: operation.keys.map(rowToWire),
            ...(schema ? { schema } : {}),
          },
        };
      case "deleteWhere":
        return { deleteWhere: deleteWhereToWire(operation.write, schema) };
      case "updateWhere":
        return { updateWhere: updateWhereToWire(operation.write, schema) };
    }
  });
  return {
    operations,
    atomicity: ATOMICITY_WIRE[batch.atomicity],
    ...(transaction ? { transaction } : {}),
  };
}
