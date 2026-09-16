import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  add,
  col,
  eq,
  ge,
  lit,
  lt,
  type Session,
  type Value,
  int,
  str,
  uint,
} from "../src/index.js";

/**
 * Predicate writes over the wire, and `returning`.
 *
 * `returning` is offered on these two and on nothing else. Over this wire an
 * insert cannot produce anything a caller does not already have: the proto's
 * row is full width and its nulls are values a caller meant, so there is no
 * unset column for a `DEFAULT` to fill, and there is no auto-increment, no
 * trigger and no generated column. The row written is the row sent. A
 * predicate write is the other case — the caller named a *condition*, and
 * which rows matched is a fact it does not have; for a delete it is a fact
 * that stops existing the moment the write lands.
 */
const PREDICATE_TABLES = `
[[tables]]
name = "papers"
id = 41
columns = [
  { name = "id",   type = "u64" },
  { name = "kind", type = "str" },
  { name = "size", type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["papers"]
actions = ["everything"]
`;

const ID = 0;
const KIND = 1;
const SIZE = 2;
const COUNT = 12;

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

/** A node holding twelve papers: ids 0..11, size = id, kind-0..kind-3. */
async function seeded(): Promise<Session> {
  const server = await start(PREDICATE_TABLES);
  servers.push(server);
  const session = server.client().session();
  const rows: Value[][] = [];
  for (let id = 0; id < COUNT; id++) {
    rows.push([uint(id), str(`kind-${id % 4}`), int(id)]);
  }
  await session.insert("papers", ...rows);
  return session;
}

async function remaining(session: Session): Promise<number> {
  let seen = 0;
  for await (const _ of session.query({ table: "papers" })) seen++;
  return seen;
}

function ids(rows: Value[][]): bigint[] {
  return rows.map((row) => {
    const id = row[0];
    assert.equal(id?.kind, "uint", `the first column was ${id?.kind}`);
    return (id as { value: bigint }).value;
  });
}

function sizes(rows: Value[][]): bigint[] {
  return rows.map((row) => {
    const size = row[2];
    assert.equal(size?.kind, "int", `size was ${size?.kind}`);
    return (size as { value: bigint }).value;
  });
}

test("a predicate delete removes every row it selects", async () => {
  const session = await seeded();
  const result = await session.deleteWhere({
    table: "papers",
    filter: ge(SIZE, int(9)),
  });
  assert.equal(result.affected, 3n);
  assert.deepEqual(result.rows, [], "no rows were asked for");
  assert.equal(await remaining(session), 9);
});

test("a predicate delete returns the rows it destroyed", async () => {
  // The one that could not be written any other way: after the delete the rows
  // are gone, so this response is the only record of what they were.
  const session = await seeded();
  const result = await session.deleteWhere({
    table: "papers",
    filter: ge(SIZE, int(9)),
    returning: true,
  });
  assert.equal(result.affected, 3n);
  assert.deepEqual(ids(result.rows), [9n, 10n, 11n]);
  assert.deepEqual(sizes(result.rows), [9n, 10n, 11n]);
  assert.equal(await remaining(session), 9);
});

test("a predicate update returns the rows as written", async () => {
  const session = await seeded();
  const result = await session.updateWhere({
    table: "papers",
    filter: lt(SIZE, int(3)),
    // size = size + 100: one write, not a read, a decision and a write.
    set: [{ column: SIZE, value: add(col(SIZE), lit(int(100))) }],
    returning: true,
  });
  assert.equal(result.affected, 3n);
  assert.deepEqual(ids(result.rows), [0n, 1n, 2n]);
  assert.deepEqual(
    sizes(result.rows),
    [100n, 101n, 102n],
    "the rows come back as written, not as they were",
  );
});

test("every assignment reads the original row", async () => {
  const session = await seeded();
  const result = await session.updateWhere({
    table: "papers",
    filter: eq(ID, uint(3)),
    set: [
      { column: SIZE, value: add(col(SIZE), lit(int(1))) },
      { column: KIND, value: col(KIND) },
    ],
    returning: true,
  });
  assert.equal(result.affected, 1n);
  assert.deepEqual(sizes(result.rows), [4n]);
});

test("an absent filter is every row", async () => {
  const session = await seeded();
  const result = await session.deleteWhere({ table: "papers" });
  assert.equal(result.affected, BigInt(COUNT));
  assert.equal(await remaining(session), 0);
});

test("a predicate that matches nothing writes nothing", async () => {
  const session = await seeded();
  const result = await session.deleteWhere({
    table: "papers",
    filter: eq(KIND, str("no-such-kind")),
    returning: true,
  });
  assert.equal(result.affected, 0n);
  assert.deepEqual(result.rows, []);
  assert.equal(await remaining(session), COUNT);
});

test("an update with no assignments is refused", async () => {
  // Refused rather than reported as zero rows written: zero is what a
  // predicate that matched nothing reports, and the two are different
  // mistakes.
  const session = await seeded();
  await assert.rejects(
    () => session.updateWhere({ table: "papers", set: [] }),
    (error: Error) => {
      assert.match(error.message, /assignment/);
      return true;
    },
  );
  assert.equal(await remaining(session), COUNT);
});

test("a column assigned twice is refused", async () => {
  const session = await seeded();
  await assert.rejects(() =>
    session.updateWhere({
      table: "papers",
      set: [
        { column: SIZE, value: lit(int(1)) },
        { column: SIZE, value: lit(int(2)) },
      ],
    }),
  );
  assert.equal(await remaining(session), COUNT);
});
