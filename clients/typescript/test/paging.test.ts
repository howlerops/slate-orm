import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import { isKind, str, uint, valueKey, type Session, type Value } from "../src/index.js";

const PAGING_TABLES = `
[[tables]]
name = "notes"
id = 30
columns = [
  { name = "id",   type = "u64" },
  { name = "body", type = "str" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["notes"]
actions = ["everything"]
`;

const COUNT = 25;

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

/** A node holding twenty-five notes, ids 1..25. */
async function noted(): Promise<Session> {
  const server = await start(PAGING_TABLES);
  servers.push(server);
  const session = server.client().session();
  const rows: Value[][] = [];
  for (let id = 1; id <= COUNT; id++) rows.push([uint(id), str("note")]);
  await session.insert("notes", ...rows);
  return session;
}

function ids(rows: Value[][]): bigint[] {
  return rows.map((row) => {
    const id = row[0]!;
    assert.equal(id.kind, "uint", `the first column is ${id.kind}`);
    return (id as { value: bigint }).value;
  });
}

const upTo = (n: number) => Array.from({ length: n }, (_, i) => BigInt(i + 1));

test("a full page carries a cursor", async () => {
  const session = await noted();
  const page = await session.page({ table: "notes", limit: 10 });
  assert.deepEqual(ids(page.rows), upTo(10));
  assert.ok(page.cursor, "a full page should carry a cursor");
  assert.equal(page.isLast, false);
});

test("a short page ends the sequence", async () => {
  const session = await noted();
  const page = await session.page({ table: "notes", limit: 1000 });
  assert.equal(page.rows.length, COUNT);
  assert.equal(page.cursor, undefined);
  assert.equal(page.isLast, true);
});

test("the cursor resumes strictly after the last row", async () => {
  const session = await noted();
  const first = await session.page({ table: "notes", limit: 10 });
  const second = await session.page({
    table: "notes",
    limit: 10,
    after: first.cursor,
  });
  assert.deepEqual(
    ids(second.rows),
    Array.from({ length: 10 }, (_, i) => BigInt(i + 11)),
  );
  // Strictly after: nothing appears on both pages.
  const seen = new Set(ids(first.rows));
  for (const id of ids(second.rows)) {
    assert.ok(!seen.has(id), `row ${id} appeared on both pages`);
  }
});

/**
 * The property `offset` cannot offer.
 *
 * Between pages, a row *behind* the cursor is deleted. Under offset-based
 * paging that shifts the window and the reader silently skips a row it has not
 * seen. A key does not move when its neighbours change.
 */
test("paging visits every row exactly once under concurrent writes", async () => {
  const session = await noted();
  const seen: bigint[] = [];
  let cursor: Value[] | undefined;
  let deleted = 0;
  // Bounded, because a bug in the cursor is exactly a loop that never ends: a
  // server returning one for a short page would page forever and this test
  // would hang rather than fail. Twenty-five notes in pages of five is six
  // requests; twenty is room for a different fixture and not for a loop.
  for (let round = 0; ; round++) {
    assert.ok(round < 20, `paging did not terminate: ${seen}`);
    const page = await session.page({ table: "notes", limit: 5, after: cursor });
    seen.push(...ids(page.rows));
    if (page.isLast) break;
    cursor = page.cursor;
    // A *different* row each round, all of them behind the cursor and already
    // in `seen`, so anything re-read shows up as a duplicate below. Deleting
    // the same row twice would refuse rather than churn.
    await session.delete("notes", [uint(seen[deleted]!)]);
    deleted++;
  }
  assert.ok(deleted > 0, "the churn never happened, so this asserts nothing");
  assert.equal(new Set(seen).size, seen.length, `a row was visited twice: ${seen}`);
  assert.deepEqual(seen, upTo(COUNT));
});

test("a page with no limit is refused", async () => {
  const session = await noted();
  await assert.rejects(
    () => session.page({ table: "notes" }),
    (error: unknown) => {
      assert.ok(isKind(error, "invalid-request"), String(error));
      assert.match(String(error), /limit/);
      return true;
    },
  );
});

/**
 * Refused rather than served without a cursor, which would be the silent
 * version: the caller loops until the cursor is absent, gets none on the first
 * page, and reads five rows of the table as the whole answer.
 */
test("a projection that drops the key is refused when paged", async () => {
  const session = await noted();
  await assert.rejects(
    () => session.page({ table: "notes", limit: 5, columns: [1] }),
    (error: unknown) => {
      assert.ok(isKind(error, "invalid-request"), String(error));
      assert.match(String(error), /id/);
      return true;
    },
  );

  // The same projection reads fine when nothing is paging.
  const rows: Value[][] = [];
  for await (const row of session.query({ table: "notes", limit: 5, columns: [1] })) {
    rows.push(row);
  }
  assert.equal(rows.length, 5);
});

/**
 * `after` without `paged`, for a caller keeping its own key. `query` is
 * unchanged by any of this: it sends no `paged`, gets no cursor, and honours
 * `after` exactly as the kernel does.
 */
test("a cursor alone pages without asking for one back", async () => {
  const session = await noted();
  const rows: Value[][] = [];
  for await (const row of session.query({
    table: "notes",
    limit: 3,
    after: [uint(10)],
  })) {
    rows.push(row);
  }
  assert.deepEqual(ids(rows), [11n, 12n, 13n]);
});

/** The cursor is the key, typed as the key: a `uint`, not a bare number. */
test("the cursor is the last row's primary key", async () => {
  const session = await noted();
  const page = await session.page({ table: "notes", limit: 4 });
  assert.deepEqual(page.cursor?.map(valueKey), [valueKey(uint(4))]);
});
