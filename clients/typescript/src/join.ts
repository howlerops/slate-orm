import { type Expr, type Ordinal, type Direction, queryToWire, type Query } from "./query.js";
import { type Scalar, scalarsToWire } from "./scalar.js";
import { type Value, valueToWire } from "./value.js";

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
  /**
   * The column's position in that input's own table — or, for a computed
   * value, which of them it is.
   */
  readonly ordinal: Ordinal;
  /**
   * Which of the wire's reference kinds this is. Absent means a stored column,
   * so an `{ input, ordinal }` literal written before computed values existed
   * still means what it meant.
   *
   * The wire distinguishes the kinds rather than carrying a flat ordinal,
   * because they live in different spaces and an ordinal that is in range in
   * the wrong one is a query about a different column.
   */
  readonly kind?: "column" | "computed" | "joined-computed";
}

/** Names column `ordinal` of input `input`. */
export const at = (input: number, ordinal: Ordinal): Column => ({ input, ordinal });

/**
 * Names the `n`th value input `input` computes.
 *
 * Legal wherever that input's own rows are read — its filter, its sort, and a
 * single-table grouping. Not across a join: an input's computed values are
 * appended to that input's row and a joined row is packed by declared table
 * width, so there is no slot for one and the server says so.
 * {@link joinComputed} is the kind that does have a slot.
 */
export const computedAt = (input: number, n: number): Column => ({
  input,
  ordinal: n,
  kind: "computed",
});

/**
 * Names the `n`th value the only input computes, for a single-table query.
 *
 * `computedAt(0, n)` says the same thing; this reads better where there is no
 * join, in the same way `key0` does.
 */
export const computed0 = (n: number): Column => computedAt(0, n);

/**
 * Names the `n`th value the **join itself** computes — the ones in
 * `JoinQuery.compute`, not any one input's.
 *
 * It is evaluated over the whole joined row, so it may read every input, and it
 * sits past every input's columns: the one place an ordinal can be added
 * without moving one that already exists. That is what makes it addressable
 * where an input's own computed value is not.
 *
 * No input index, because the value belongs to the request rather than to one
 * of its tables.
 */
export const joinComputed = (n: number): Column => ({
  input: 0,
  ordinal: n,
  kind: "joined-computed",
});

/**
 * Names column `ordinal` of the only input, for grouping one table.
 *
 * `at(0, ordinal)` says the same thing; this reads better where there is no
 * join and no second input to distinguish from.
 */
export const key0 = (ordinal: Ordinal): Column => at(0, ordinal);

/** A reference in its wire form, by kind. Exported for `scalar.ts`. */
export const columnWire = (c: Column): Record<string, unknown> => {
  switch (c.kind) {
    case "computed":
      return { input: c.input, computed: c.ordinal };
    case "joined-computed":
      // No input: the value belongs to the request rather than to one of its
      // tables, and the server refuses a non-zero one here.
      return { joinedComputed: c.ordinal };
    default:
      return { input: c.input, column: c.ordinal };
  }
};

/** Which rows survive a join. */
export type JoinType = "inner" | "left" | "right" | "full";

const JOIN_TYPES: Record<JoinType, string> = {
  inner: "JOIN_TYPE_INNER",
  left: "JOIN_TYPE_LEFT",
  right: "JOIN_TYPE_RIGHT",
  full: "JOIN_TYPE_FULL",
};

/**
 * A join strategy forced in place of the planner's choice.
 *
 * A test aid and an escape hatch, not a tuning knob: the planner costs these
 * and is usually right. Forcing one is how an oracle checks that every
 * algorithm agrees, which is why it is reachable at all.
 */
export type Algorithm = "hash-build-left" | "hash-build-right" | "nested-loop";

function algorithmWire(a: Algorithm): Record<string, unknown> {
  switch (a) {
    case "hash-build-left":
      return { hashBuild: "SIDE_LEFT" };
    case "hash-build-right":
      return { hashBuild: "SIDE_RIGHT" };
    case "nested-loop":
      return { nestedLoop: "UNIT" };
  }
}

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
  /**
   * Values this input computes from its own rows, named with `computedAt`.
   *
   * Readable by this input's own filter. Not readable across the join and not
   * groupable — see `computedAt` — for which `JoinQuery.compute` is the answer.
   */
  readonly compute?: Scalar[];
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
  /**
   * Values computed per *joined* row, appended after every input's columns and
   * named with `joinComputed`.
   *
   * The arrangement `Query.compute` uses on one table, lifted one level. What
   * is new is that the expression is evaluated over the joined row, so it may
   * read both sides at once — which is the thing no input's own `compute` can
   * express, and the reason this field is here rather than there.
   *
   * Each may read every input's columns and the values *before* it, so
   * `compute[1]` may read `joinComputed(0)` and not the other way round.
   *
   * An ungrouped join returns them on each row's `computed`; a grouped one
   * exposes them to `groupBy` and the aggregates.
   */
  readonly compute?: Scalar[];
}

/**
 * Accumulates inputs and hands out their positions.
 *
 * An input's position is assigned in call order, which is the order the wire
 * declares them in. This exists so the position is never written by hand: a
 * literal `1` that goes stale when an input is inserted above it is a join
 * that silently answers a different question.
 */
export class JoinBuilder {
  readonly #inputs: JoinInput[] = [];

  /** Appends an input and returns its position. */
  add(input: JoinInput): number {
    this.#inputs.push(input);
    return this.#inputs.length - 1;
  }

  /** The join built so far. */
  query(extra: Omit<JoinQuery, "inputs"> = {}): JoinQuery {
    return { inputs: [...this.#inputs], ...extra };
  }

  /** How many inputs have been added. */
  get inputs(): number {
    return this.#inputs.length;
  }
}

/** Starts a join. */
export const newJoin = (): JoinBuilder => new JoinBuilder();

/**
 * A join in its wire form.
 *
 * `claim` supplies each input's declaration by table name, so a join checks
 * every table it reads rather than none of them.
 */
export function joinToWire(
  join: JoinQuery,
  claim: (table: string) => Record<string, unknown> | undefined = () => undefined,
): Record<string, unknown> {
  const out: Record<string, unknown> = {
    inputs: join.inputs.map((input, position) => {
      const query: Query = {
        table: input.table,
        ...(input.filter ? { filter: input.filter } : {}),
        ...(input.columns ? { columns: input.columns } : {}),
        ...(input.descending ? { descending: input.descending } : {}),
        ...(input.compute ? { compute: input.compute } : {}),
      };
      const wire: Record<string, unknown> = {
        query: queryToWire(query, claim(input.table)),
        joinType: JOIN_TYPES[input.type ?? "inner"],
        on: (input.on ?? []).map((on) => ({
          earlier: columnWire(on.earlier),
          // `own` carries this input's own position: the server checks that
          // the two sides name different inputs, and an ordinal defaulting to
          // input 0 is refused on every input but the first.
          own: columnWire(at(position, on.own)),
        })),
      };
      if (input.having) wire["having"] = input.having.wire;
      if (input.force) wire["force"] = algorithmWire(input.force);
      return wire;
    }),
  };
  if (join.limit !== undefined) out["limit"] = String(join.limit);
  if (join.offset !== undefined) out["offset"] = String(join.offset);
  if (join.buildLimit !== undefined) out["buildLimit"] = String(join.buildLimit);
  if (join.compute && join.compute.length > 0) {
    out["compute"] = scalarsToWire(join.compute);
  }
  return out;
}

/** What an aggregate computes. */
export type AggregateFunction =
  | "count"
  | "count-column"
  | "count-distinct"
  | "min"
  | "max"
  | "sum"
  | "avg";

const FUNCTIONS: Record<AggregateFunction, string> = {
  count: "AGGREGATE_FUNCTION_COUNT",
  "count-column": "AGGREGATE_FUNCTION_COUNT_COLUMN",
  "count-distinct": "AGGREGATE_FUNCTION_COUNT_DISTINCT",
  min: "AGGREGATE_FUNCTION_MIN",
  max: "AGGREGATE_FUNCTION_MAX",
  sum: "AGGREGATE_FUNCTION_SUM",
  avg: "AGGREGATE_FUNCTION_AVG",
};

/** One computed value over a group. */
export interface Aggregate {
  readonly function: AggregateFunction;
  /** What it reads. Absent for `count`, required by the rest. */
  readonly column?: Column;
}

/** `COUNT(*)`: every row, nulls included. */
export const count = (): Aggregate => ({ function: "count" });
/** `COUNT(column)`, which skips nulls. */
export const countOf = (column: Column): Aggregate => ({ function: "count-column", column });
/** `COUNT(DISTINCT column)`. */
export const countDistinctOf = (column: Column): Aggregate => ({
  function: "count-distinct",
  column,
});
/** `MIN(column)`. */
export const minOf = (column: Column): Aggregate => ({ function: "min", column });
/** `MAX(column)`. */
export const maxOf = (column: Column): Aggregate => ({ function: "max", column });
/** `SUM(column)`. */
export const sumOf = (column: Column): Aggregate => ({ function: "sum", column });
/** `AVG(column)`. */
export const avgOf = (column: Column): Aggregate => ({ function: "avg", column });

function aggregateWire(a: Aggregate): Record<string, unknown> {
  const out: Record<string, unknown> = { function: FUNCTIONS[a.function] };
  // `COUNT(*)` reads no column, and sending one would be a different
  // aggregate. Every other function requires it.
  if (a.function !== "count" && a.column) out["column"] = columnWire(a.column);
  return out;
}

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
export const groupKey = (index: number): GroupRef => ({ wire: { input: 0, groupKey: index } });

/** The `index`th aggregate. */
export const agg = (index: number): GroupRef => ({ wire: { input: 0, aggregate: index } });

/** One column of an ordering over groups. */
export interface GroupSortKey {
  readonly column: GroupRef;
  readonly direction?: Direction;
}

const groupCompare = (ref: GroupRef, op: string, value: Value): Expr => ({
  wire: { compare: { column: ref.wire, op, value: valueToWire(value) } },
});

/**
 * Comparisons over a grouped result, for `having`.
 *
 * Separate from the row-level `eq` and friends because they take a [GroupRef]
 * rather than an [Ordinal]. Two families rather than one overloaded family
 * means the wrong one does not typecheck, instead of being refused at runtime
 * by a server the caller has already deployed against.
 */
export const groupEq = (r: GroupRef, v: Value): Expr => groupCompare(r, "CMP_OP_EQ", v);
/** `<> value` over a group. */
export const groupNe = (r: GroupRef, v: Value): Expr => groupCompare(r, "CMP_OP_NE", v);
/** `< value` over a group. */
export const groupLt = (r: GroupRef, v: Value): Expr => groupCompare(r, "CMP_OP_LT", v);
/** `<= value` over a group. */
export const groupLe = (r: GroupRef, v: Value): Expr => groupCompare(r, "CMP_OP_LE", v);
/** `> value` over a group. */
export const groupGt = (r: GroupRef, v: Value): Expr => groupCompare(r, "CMP_OP_GT", v);
/** `>= value` over a group. */
export const groupGe = (r: GroupRef, v: Value): Expr => groupCompare(r, "CMP_OP_GE", v);

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
  /**
   * What to compute per group.
   *
   * May be empty: keys with no aggregates are the distinct combinations of
   * those keys — `SELECT DISTINCT`. What the server refuses is neither, which
   * asks for one group with nothing in it.
   */
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
export function applyGrouping(
  out: Record<string, unknown>,
  grouping: Grouping,
): Record<string, unknown> {
  out["groupBy"] = (grouping.groupBy ?? []).map(columnWire);
  out["aggregates"] = grouping.aggregates.map(aggregateWire);
  out["sort"] = (grouping.sort ?? []).map((key) => ({
    column: key.column.wire,
    direction: key.direction === "desc" ? "SORT_DIRECTION_DESC" : "SORT_DIRECTION_ASC",
  }));
  if (grouping.having) out["having"] = grouping.having.wire;
  if (grouping.limit !== undefined) out["limit"] = String(grouping.limit);
  if (grouping.offset !== undefined) out["offset"] = String(grouping.offset);
  return out;
}

/** One row of a grouped answer. */
export interface Group {
  /** The group's key values, in `groupBy` order. */
  readonly key: Value[];
  /** The aggregates, in `aggregates` order. */
  readonly values: Value[];
}
