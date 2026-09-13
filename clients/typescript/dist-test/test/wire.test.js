import assert from "node:assert/strict";
import { test } from "node:test";
import { applyGrouping, at, count } from "../src/join.js";
/**
 * Wire-shape tests, with no server.
 *
 * Everything else in this suite goes through a real node, which is the right
 * default. These cases are here because they assert on what is *not* sent, and
 * a server that ignores a field it should never have received cannot tell the
 * two apart.
 */
test("COUNT(*) sends no column, even when one is set", () => {
    // `count()` never sets a column, so the guard against sending one is only
    // reachable through a hand-built aggregate — which the type permits, since
    // `column` is optional on every function. Sending it would make this a
    // COUNT(column), which skips nulls and is a different question.
    const handBuilt = { function: "count", column: at(0, 3) };
    const wire = applyGrouping({}, { aggregates: [handBuilt] });
    const aggregates = wire["aggregates"];
    assert.equal(aggregates.length, 1);
    assert.equal(aggregates[0]["function"], "AGGREGATE_FUNCTION_COUNT");
    assert.ok(!("column" in aggregates[0]), `COUNT(*) must send no column, got ${JSON.stringify(aggregates[0])}`);
});
test("COUNT(column) does send its column", () => {
    const wire = applyGrouping({}, {
        aggregates: [{ function: "count-column", column: at(1, 2) }],
    });
    const aggregates = wire["aggregates"];
    assert.deepEqual(aggregates[0]["column"], { input: 1, column: 2 });
});
test("an aggregate with no group by sends an empty key list", () => {
    // Not an absent field: the server distinguishes "group by nothing" — one
    // group over every row — from a malformed request, and an omitted list
    // would be relying on proto3's default rather than saying so.
    const wire = applyGrouping({}, { aggregates: [count()] });
    assert.deepEqual(wire["groupBy"], []);
});
