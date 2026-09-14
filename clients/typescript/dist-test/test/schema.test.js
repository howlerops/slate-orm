import assert from "node:assert/strict";
import { after, test } from "node:test";
import { start } from "./harness.js";
import { fingerprint, int, isKind, str, uint, } from "../src/index.js";
const servers = [];
after(() => {
    for (const s of servers)
        s.stop();
});
/** `docs`, as the harness's fixture actually declares it. */
const DOCS = {
    name: "docs",
    columns: [
        { name: "id", type: "u64" },
        { name: "kind", type: "string" },
        { name: "size", type: "i64" },
    ],
    primaryKey: ["id"],
};
async function declaring(schemas) {
    const server = await start("");
    servers.push(server);
    return server.client().declaring(schemas).session();
}
// A correct declaration is accepted, on every path that carries one.
//
// All four rather than one: the claim is attached at ten call sites, and a
// missed one is invisible — the request is simply not checked.
test("a correct declaration is accepted", async () => {
    const session = await declaring({ docs: DOCS });
    await session.insert("docs", [uint(1), str("note"), int(10)]);
    await session.get("docs", [uint(1)]);
    await session.update("docs", [uint(1), str("edited"), int(11)]);
    await session.delete("docs", [uint(1)]);
});
// The mistake ordinals make easy, and the reason this exists.
//
// A client that swaps two columns reads `kind` where the table has `size`.
// Undeclared that is silent and permanent; declared, the first request is
// refused.
test("a swapped column order is refused", async () => {
    const swapped = {
        ...DOCS,
        columns: [DOCS.columns[0], DOCS.columns[2], DOCS.columns[1]],
    };
    const session = await declaring({ docs: swapped });
    await assert.rejects(() => session.insert("docs", [uint(1), int(10), str("note")]), (error) => {
        assert.ok(isKind(error, "invalid-request"), `kind was ${error.kind}`);
        return true;
    });
});
test("a wrong column name is refused", async () => {
    const renamed = {
        ...DOCS,
        columns: [DOCS.columns[0], { name: "category", type: "string" }, DOCS.columns[2]],
    };
    const session = await declaring({ docs: renamed });
    await assert.rejects(() => session.insert("docs", [uint(1), str("note"), int(10)]));
});
test("a wrong column type is refused", async () => {
    const retyped = {
        ...DOCS,
        columns: [DOCS.columns[0], DOCS.columns[1], { name: "size", type: "u64" }],
    };
    const session = await declaring({ docs: retyped });
    await assert.rejects(() => session.insert("docs", [uint(1), str("note"), int(10)]));
});
// The check is opt-in per table, so declaring one must not start refusing
// requests for a table the caller said nothing about.
test("an undeclared table is unaffected", async () => {
    const server = await start(`
[[tables]]
name = "other"
id = 7
columns = [{ name = "id", type = "u64" }]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["other"]
actions = ["everything"]
`);
    servers.push(server);
    const session = server.client().declaring({ docs: DOCS }).session();
    await session.insert("other", [uint(1)]);
});
/**
 * The fingerprint must be the *server's*, not merely self-consistent.
 *
 * A client whose hash is internally consistent and different from the server's
 * refuses every request, and the tests above cannot see that — they assert
 * that a wrong declaration is refused, and a wrong hash refuses everything.
 *
 * The pinned value is not this implementation's own output written down: it is
 * what the Python client — a separate port, already checked against a live
 * server — computes for the same table. Two independent ports agreeing on a
 * constant is evidence; one port agreeing with itself is not.
 */
test("the fingerprint matches the canonical form", () => {
    //	>>> from slate.schema import fingerprint_of
    //	>>> hex(fingerprint_of(DOCS))
    //	'0x97c3c1256af4cfdb'
    assert.equal(fingerprint(DOCS), 0x97c3c1256af4cfdbn);
});
/**
 * A key naming a column the declaration does not have must not collide with a
 * correct declaration whose key is the first column.
 *
 * The obvious implementation hashes the miss as ordinal 0, which makes
 * `primaryKey: ["nope"]` hash identically to `primaryKey: ["id"]`.
 */
test("a key naming a missing column does not collide", () => {
    assert.notEqual(fingerprint(DOCS), fingerprint({ ...DOCS, primaryKey: ["nope"] }));
});
/**
 * A name outside the BMP hashes by its UTF-8 byte length, not by JavaScript's
 * UTF-16 `.length`.
 *
 * This is the one place a JavaScript port can silently disagree with every
 * other client: `"𝕏".length` is 2 and its UTF-8 length is 4, so a `.length`
 * here produces a fingerprint no server accepts — for exactly the tables whose
 * column names are not ASCII, and no others.
 *
 * Pinned against the Python port rather than compared against a second
 * TypeScript fingerprint. The first version of this test did the latter, using
 * two names of equal UTF-16 length, and it *passed under the bug*: swapping the
 * prefix changed both hashes and they stayed different from each other. Only a
 * value computed by a different implementation can catch a length prefix that
 * is consistently wrong.
 */
test("a non-BMP column name hashes by its byte length", () => {
    //	>>> fingerprint_of(Table("docs", [Column("𝕏", U64)], primary_key=["𝕏"]))
    //	'0x8763d37fb927fd16'
    const wide = {
        name: "docs",
        columns: [{ name: "\u{1D54F}", type: "u64" }],
        primaryKey: ["\u{1D54F}"],
    };
    assert.equal(fingerprint(wide), 0x8763d37fb927fd16n);
});
/**
 * A *read* against a misdeclared table is refused too.
 *
 * The first version of this checked only the five requests with a top-level
 * `SchemaCheck` field and left every read unchecked — the larger half of the
 * exposure, since a client reading a transposed table gets transposed rows on
 * every query. The claim rides on the `Query` message, so it covers `query`,
 * `explain`, `join` and `aggregate` alike.
 */
test("a misdeclared table is refused on read", async () => {
    const swapped = {
        ...DOCS,
        columns: [DOCS.columns[0], DOCS.columns[2], DOCS.columns[1]],
    };
    const session = await declaring({ docs: swapped });
    await assert.rejects(() => session.query({ table: "docs" }).collect(), (error) => {
        assert.ok(isKind(error, "invalid-request"), `kind was ${error.kind}`);
        return true;
    });
    await assert.rejects(() => session.explain({ table: "docs" }));
});
test("a correct declaration still reads", async () => {
    const session = await declaring({ docs: DOCS });
    await session.insert("docs", [uint(1), str("note"), int(10)]);
    const rows = await session.query({ table: "docs" }).collect();
    assert.equal(rows.length, 1);
});
