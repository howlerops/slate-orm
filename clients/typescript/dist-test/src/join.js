import { queryToWire } from "./query.js";
import { valueToWire } from "./value.js";
/** Names column `ordinal` of input `input`. */
export const at = (input, ordinal) => ({ input, ordinal });
/**
 * Names column `ordinal` of the only input, for grouping one table.
 *
 * `at(0, ordinal)` says the same thing; this reads better where there is no
 * join and no second input to distinguish from.
 */
export const key0 = (ordinal) => at(0, ordinal);
const columnWire = (c) => ({ input: c.input, column: c.ordinal });
const JOIN_TYPES = {
    inner: "JOIN_TYPE_INNER",
    left: "JOIN_TYPE_LEFT",
    right: "JOIN_TYPE_RIGHT",
    full: "JOIN_TYPE_FULL",
};
function algorithmWire(a) {
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
 * Accumulates inputs and hands out their positions.
 *
 * An input's position is assigned in call order, which is the order the wire
 * declares them in. This exists so the position is never written by hand: a
 * literal `1` that goes stale when an input is inserted above it is a join
 * that silently answers a different question.
 */
export class JoinBuilder {
    #inputs = [];
    /** Appends an input and returns its position. */
    add(input) {
        this.#inputs.push(input);
        return this.#inputs.length - 1;
    }
    /** The join built so far. */
    query(extra = {}) {
        return { inputs: [...this.#inputs], ...extra };
    }
    /** How many inputs have been added. */
    get inputs() {
        return this.#inputs.length;
    }
}
/** Starts a join. */
export const newJoin = () => new JoinBuilder();
/**
 * A join in its wire form.
 *
 * `claim` supplies each input's declaration by table name, so a join checks
 * every table it reads rather than none of them.
 */
export function joinToWire(join, claim = () => undefined) {
    const out = {
        inputs: join.inputs.map((input, position) => {
            const query = {
                table: input.table,
                ...(input.filter ? { filter: input.filter } : {}),
                ...(input.columns ? { columns: input.columns } : {}),
                ...(input.descending ? { descending: input.descending } : {}),
            };
            const wire = {
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
            if (input.having)
                wire["having"] = input.having.wire;
            if (input.force)
                wire["force"] = algorithmWire(input.force);
            return wire;
        }),
    };
    if (join.limit !== undefined)
        out["limit"] = String(join.limit);
    if (join.offset !== undefined)
        out["offset"] = String(join.offset);
    if (join.buildLimit !== undefined)
        out["buildLimit"] = String(join.buildLimit);
    return out;
}
const FUNCTIONS = {
    count: "AGGREGATE_FUNCTION_COUNT",
    "count-column": "AGGREGATE_FUNCTION_COUNT_COLUMN",
    "count-distinct": "AGGREGATE_FUNCTION_COUNT_DISTINCT",
    min: "AGGREGATE_FUNCTION_MIN",
    max: "AGGREGATE_FUNCTION_MAX",
    sum: "AGGREGATE_FUNCTION_SUM",
    avg: "AGGREGATE_FUNCTION_AVG",
};
/** `COUNT(*)`: every row, nulls included. */
export const count = () => ({ function: "count" });
/** `COUNT(column)`, which skips nulls. */
export const countOf = (column) => ({ function: "count-column", column });
/** `COUNT(DISTINCT column)`. */
export const countDistinctOf = (column) => ({
    function: "count-distinct",
    column,
});
/** `MIN(column)`. */
export const minOf = (column) => ({ function: "min", column });
/** `MAX(column)`. */
export const maxOf = (column) => ({ function: "max", column });
/** `SUM(column)`. */
export const sumOf = (column) => ({ function: "sum", column });
/** `AVG(column)`. */
export const avgOf = (column) => ({ function: "avg", column });
function aggregateWire(a) {
    const out = { function: FUNCTIONS[a.function] };
    // `COUNT(*)` reads no column, and sending one would be a different
    // aggregate. Every other function requires it.
    if (a.function !== "count" && a.column)
        out["column"] = columnWire(a.column);
    return out;
}
/** The `index`th GROUP BY key. */
export const groupKey = (index) => ({ wire: { input: 0, groupKey: index } });
/** The `index`th aggregate. */
export const agg = (index) => ({ wire: { input: 0, aggregate: index } });
const groupCompare = (ref, op, value) => ({
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
export const groupEq = (r, v) => groupCompare(r, "CMP_OP_EQ", v);
/** `<> value` over a group. */
export const groupNe = (r, v) => groupCompare(r, "CMP_OP_NE", v);
/** `< value` over a group. */
export const groupLt = (r, v) => groupCompare(r, "CMP_OP_LT", v);
/** `<= value` over a group. */
export const groupLe = (r, v) => groupCompare(r, "CMP_OP_LE", v);
/** `> value` over a group. */
export const groupGt = (r, v) => groupCompare(r, "CMP_OP_GT", v);
/** `>= value` over a group. */
export const groupGe = (r, v) => groupCompare(r, "CMP_OP_GE", v);
/** Applies a grouping onto an `AggregateQuery` in its wire form. */
export function applyGrouping(out, grouping) {
    out["groupBy"] = (grouping.groupBy ?? []).map(columnWire);
    out["aggregates"] = grouping.aggregates.map(aggregateWire);
    out["sort"] = (grouping.sort ?? []).map((key) => ({
        column: key.column.wire,
        direction: key.direction === "desc" ? "SORT_DIRECTION_DESC" : "SORT_DIRECTION_ASC",
    }));
    if (grouping.having)
        out["having"] = grouping.having.wire;
    if (grouping.limit !== undefined)
        out["limit"] = String(grouping.limit);
    if (grouping.offset !== undefined)
        out["offset"] = String(grouping.offset);
    return out;
}
