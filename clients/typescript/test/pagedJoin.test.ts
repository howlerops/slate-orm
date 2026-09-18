import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  at,
  isKind,
  newJoin,
  str,
  uint,
  type JoinQuery,
  type Session,
  type Value,
} from "../src/index.js";

/**
 * Writers and works with uneven fan-out, so a page of N writers is more than N
 * rows and a test that confused the two would fail rather than pass by luck.
 *
 * "region" sits in front of "id" for the reason `relatedPath.test.ts` gives: a
 * fixture where the right answer is ordinal 0 cannot tell "read the key" from
 * "assume the first column". Here it also makes the cursor a *two-column* key,
 * so a client that sent one value would be refused rather than accidentally
 * right.
 */
const PAGED_TABLES = `
[[tables]]
name = "writers"
id = 40
columns = [
  { name = "region", type = "str" },
  { name = "id",     type = "u64" },
  { name = "name",   type = "str" },
]
primary_key = ["region", "id"]

[[tables]]
name = "works"
id = 41
columns = [
  { name = "id",        type = "u64" },
  { name = "writer_id", type = "u64" },
  { name = "title",     type = "str" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["writers", "works"]
actions = ["everything"]
`;

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

const WRITERS = 6;

/** Writers 0..5, writer n having n works — so writer 0 matches nothing. */
async function seeded(): Promise<Session> {
  const server = await start(PAGED_TABLES);
  servers.push(server);
  const session = server.client().session();
  for (let id = 0; id < WRITERS; id += 1) {
    await session.insert("writers", [str("north"), uint(id), str(`w-${id}`)]);
  }
  let work = 0;
  for (let id = 0; id < WRITERS; id += 1) {
    for (let n = 0; n < id; n += 1) {
      work += 1;
      await session.insert("works", [uint(work), uint(id), str("t")]);
    }
  }
  return session;
}

function paged(cursor: Value[] | undefined, limit: number | undefined): JoinQuery {
  const b = newJoin();
  const writers = b.add({ table: "writers" });
  b.add({ table: "works", on: [{ earlier: at(writers, 1), own: 1 }] });
  const query = b.query();
  return {
    ...query,
    ...(limit !== undefined ? { limit } : {}),
    ...(cursor !== undefined ? { after: cursor } : {}),
  };
}

/** Each input's id, asserting an inner join produced both. */
function ids(row: { inputs: (Value[] | undefined)[] }): [bigint, bigint] {
  const [writer, work] = row.inputs;
  assert.ok(writer && work, "an inner join returned an absent input");
  // Asserted rather than cast: a `Value` is a tagged union, and a cast would
  // make a schema change read as a passing test over the wrong column.
  const [a, b] = [writer[1], work[0]];
  assert.ok(a?.kind === "uint" && b?.kind === "uint", "both ids are u64");
  return [a.value, b.value];
}

test("paging a join reproduces the whole join, and nothing twice", async () => {
  const session = await seeded();
  const whole: string[] = [];
  for await (const row of session.join(paged(undefined, undefined)).withComputed()) {
    whole.push(ids(row).join("/"));
  }

  for (const size of [1, 2, 3]) {
    const seen: string[] = [];
    let cursor: Value[] | undefined;
    for (let guard = 0; ; guard += 1) {
      assert.ok(guard < 40, `pages of ${size} did not terminate: the cursor is stuck`);
      const page = await session.pageJoin(paged(cursor, size));
      for (const row of page.rows) seen.push(ids(row).join("/"));
      if (page.isLast) break;
      cursor = page.cursor;
    }
    assert.deepEqual(
      seen.slice().sort(),
      whole.slice().sort(),
      `pages of ${size} did not reproduce the join`,
    );
    assert.equal(new Set(seen).size, seen.length, "a row was visited twice");
  }
});

test("a page is bounded in input-0 rows, not in returned rows", async () => {
  const session = await seeded();
  // Four writers is 0, 1, 2 and 3; writer 0 has no works, so three appear.
  const page = await session.pageJoin(paged(undefined, 4));
  const writers = new Set(page.rows.map((row) => ids(row)[0]));
  assert.equal(writers.size, 3, "three writers with works in a page of four");
  assert.equal(page.rows.length, 1 + 2 + 3, "and all of their works");
});

test("a page whose writers have no works still advances", async () => {
  const session = await seeded();
  const page = await session.pageJoin(paged(undefined, 1));
  assert.equal(page.rows.length, 0, "writer 0 has no works");
  assert.ok(!page.isLast, "a full page — one writer read — still carries a cursor");
  assert.equal(page.cursor?.length, 2, "the cursor is writers' whole two-column key");
});

test("a right outer join refuses a cursor, on its first page", async () => {
  const session = await seeded();
  const b = newJoin();
  const writers = b.add({ table: "writers" });
  b.add({ table: "works", type: "right", on: [{ earlier: at(writers, 1), own: 1 }] });
  await assert.rejects(
    () => session.pageJoin({ ...b.query(), limit: 2 }),
    (error: unknown) =>
      isKind(error, "invalid-request") && String(error).includes("belong to no page"),
  );
});

test("an offset beside a cursor is refused", async () => {
  const session = await seeded();
  await assert.rejects(
    () => session.pageJoin({ ...paged(undefined, 2), offset: 3 }),
    (error: unknown) =>
      isKind(error, "invalid-request") && String(error).includes("Drop the offset"),
  );
});

test("a page with no size is refused", async () => {
  const session = await seeded();
  await assert.rejects(
    () => session.pageJoin(paged(undefined, undefined)),
    (error: unknown) =>
      isKind(error, "invalid-request") && String(error).includes("a page needs a size"),
  );
});
