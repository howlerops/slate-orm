import { valueToWire } from "./value.js";
const columnRef = (column) => ({ column });
/** Admits every row. */
export const alwaysTrue = () => ({ wire: { literal: true } });
/** Admits none. */
export const alwaysFalse = () => ({ wire: { literal: false } });
const compare = (column, op, value) => ({
    wire: { compare: { column: columnRef(column), op, value: valueToWire(value) } },
});
/** `column = value`. */
export const eq = (c, v) => compare(c, "CMP_OP_EQ", v);
/** `column <> value`. */
export const ne = (c, v) => compare(c, "CMP_OP_NE", v);
/** `column < value`. */
export const lt = (c, v) => compare(c, "CMP_OP_LT", v);
/** `column <= value`. */
export const le = (c, v) => compare(c, "CMP_OP_LE", v);
/** `column > value`. */
export const gt = (c, v) => compare(c, "CMP_OP_GT", v);
/** `column >= value`. */
export const ge = (c, v) => compare(c, "CMP_OP_GE", v);
/** `column IS NULL`. */
export const isNull = (column) => ({
    wire: { isNull: { column: columnRef(column), negated: false } },
});
/** `column IS NOT NULL`. */
export const isNotNull = (column) => ({
    wire: { isNull: { column: columnRef(column), negated: true } },
});
/** `column IN (values)`. */
export const isIn = (column, values) => ({
    wire: { inList: { column: columnRef(column), values: values.map(valueToWire) } },
});
const likeExpr = (column, pattern, negated, insensitive) => ({
    wire: { like: { column: columnRef(column), pattern, negated, insensitive } },
});
/** `column LIKE pattern`, with `%` and `_` as the wildcards. */
export const like = (c, p) => likeExpr(c, p, false, false);
/** `column ILIKE pattern`, matching without regard to case. */
export const ilike = (c, p) => likeExpr(c, p, false, true);
/** `column NOT LIKE pattern`. */
export const notLike = (c, p) => likeExpr(c, p, true, false);
/**
 * The conjunction of every part.
 *
 * With no parts this is [alwaysTrue], the identity for `AND`, so a filter
 * built up in a loop that adds nothing does not become `false` by accident.
 */
export const and = (...parts) => parts.length === 0
    ? alwaysTrue()
    : { wire: { conjunction: { exprs: parts.map((p) => p.wire) } } };
/** The disjunction. With no parts this is [alwaysFalse], the identity for `OR`. */
export const or = (...parts) => parts.length === 0
    ? alwaysFalse()
    : { wire: { disjunction: { exprs: parts.map((p) => p.wire) } } };
/** Inverts a predicate. */
export const not = (inner) => ({ wire: { negation: inner.wire } });
/** A query in its wire form. */
export function queryToWire(query) {
    const out = { table: query.table };
    if (query.filter)
        out["filter"] = query.filter.wire;
    if (query.descending)
        out["order"] = "SCAN_ORDER_DESCENDING";
    if (query.limit !== undefined)
        out["limit"] = String(query.limit);
    if (query.offset !== undefined)
        out["offset"] = String(query.offset);
    if (query.columns && query.columns.length > 0) {
        out["projection"] = { columns: query.columns.map(columnRef) };
    }
    if (query.sort && query.sort.length > 0) {
        out["sort"] = query.sort.map((key) => ({
            column: columnRef(key.column),
            direction: key.direction === "desc" ? "SORT_DIRECTION_DESC" : "SORT_DIRECTION_ASC",
        }));
    }
    return out;
}
