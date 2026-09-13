import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  bool,
  bytes,
  float,
  ge,
  int,
  isKind,
  nullValue,
  SlateError,
  str,
  uint,
  uuid,
  type Value,
} from "../src/index.js";

const servers: Serving[] = [];

async function serving(extra = ""): Promise<Serving> {
  const s = await start(extra);
  servers.push(s);
  return s;
}

after(() => {
  for (const s of servers) s.stop();
});

function expectUint(value: Value | undefined, want: bigint, what: string): void {
  assert.ok(value, `${what} is missing`);
  assert.equal(value.kind, "uint", `${what} should be a uint`);
  assert.equal((value as { value: bigint }).value, want, what);
}

test("insert then get", async () => {
  const session = (await serving()).client().session();

  const result = await session.insert("docs", [uint(1), str("note"), int(10)]);
  assert.equal(result.affected, 1n);
  // The writer's position, which is what a caller carries to another session
  // to read its own write there.
  assert.ok(result.sequence !== undefined, "a write should report its sequence");

  const row = await session.get("docs", [uint(1)]);
  assert.ok(row, "the row just inserted was not found");
  assert.equal(row[1]?.kind, "string");
  assert.equal((row[1] as { value: string }).value, "note");
  assert.equal((row[2] as { value: bigint }).value, 10n);
});

// A missing row is `undefined`, not a throw: checking existence should not
// mean catching an exception.
test("a missing row is undefined rather than an error", async () => {
  const session = (await serving()).client().session();
  assert.equal(await session.get("docs", [uint(404)]), undefined);
});

test("insert refuses a duplicate key", async () => {
  const session = (await serving()).client().session();
  const row = [uint(1), str("a"), int(1)];
  await session.insert("docs", row);

  await assert.rejects(
    () => session.insert("docs", row),
    (error: unknown) => {
      assert.ok(isKind(error, "already-exists"), `kind was ${(error as SlateError).kind}`);
      assert.equal((error as SlateError).retryable, false, "a taken key is not retryable");
      return true;
    },
  );
});

// Upsert is the same call with a flag, so it is worth pinning that the flag
// reaches the server rather than the two behaving identically.
test("upsert replaces where insert refuses", async () => {
  const session = (await serving()).client().session();
  await session.insert("docs", [uint(1), str("first"), int(1)]);
  await session.upsert("docs", [uint(1), str("second"), int(2)]);

  const row = await session.get("docs", [uint(1)]);
  assert.equal((row?.[1] as { value: string }).value, "second");
});

test("query filters and orders", async () => {
  const session = (await serving()).client().session();
  for (let i = 1n; i <= 5n; i++) {
    await session.insert("docs", [uint(i), str("k"), int(i * 10n)]);
  }

  const rows = await session
    .query({
      table: "docs",
      filter: ge(2, int(30)),
      sort: [{ column: 2, direction: "desc" }],
    })
    .collect();

  assert.equal(rows.length, 3, "sizes 30, 40 and 50 match");
  assert.equal((rows[0]?.[2] as { value: bigint }).value, 50n);
  assert.equal((rows[2]?.[2] as { value: bigint }).value, 30n);
});

test("limit and projection", async () => {
  const session = (await serving()).client().session();
  for (let i = 1n; i <= 4n; i++) {
    await session.insert("docs", [uint(i), str("k"), int(i)]);
  }

  const rows = await session
    .query({ table: "docs", limit: 2, columns: [0] })
    .collect();

  assert.equal(rows.length, 2);
  // Columns outside the projection come back null rather than absent, so the
  // row keeps its shape and an ordinal still means what it meant.
  assert.equal(rows[0]?.[1]?.kind, "null");
});

test("a rolled-back transaction discards its writes", async () => {
  const session = (await serving()).client().session();
  const tx = await session.begin();

  await tx.insert("docs", [uint(7), str("x"), int(1)]);
  assert.ok(await tx.get("docs", [uint(7)]), "a transaction must see its own writes");

  await tx.rollback();
  assert.equal(await session.get("docs", [uint(7)]), undefined);
});

test("a committed transaction lands", async () => {
  const session = (await serving()).client().session();
  const tx = await session.begin();
  await tx.insert("docs", [uint(8), str("y"), int(2)]);
  await tx.commit();

  assert.ok(await session.get("docs", [uint(8)]));
});

// `finally { await tx.rollback() }` is the shape every transaction should
// use, so rolling back after a commit has to be quiet.
test("rollback after commit is quiet", async () => {
  const session = (await serving()).client().session();
  const tx = await session.begin();
  await tx.commit();
  await tx.rollback();
});

test("delete", async () => {
  const session = (await serving()).client().session();
  await session.insert("docs", [uint(3), str("z"), int(1)]);

  const result = await session.delete("docs", [uint(3)]);
  assert.equal(result.affected, 1n);
  assert.equal(await session.get("docs", [uint(3)]), undefined);
});

// Every value kind must survive the round trip. A client that encodes a u64 as
// an i64 gets a different value back from this server, because its ordering is
// type-first, so this is not a formality.
test("every value kind round-trips", async () => {
  const server = await serving(`
[[tables]]
name = "kinds"
id = 2
columns = [
  { name = "id",  type = "u64" },
  { name = "b",   type = "bool" },
  { name = "by",  type = "bytes" },
  { name = "s",   type = "str" },
  { name = "i",   type = "i64" },
  { name = "u",   type = "u64" },
  { name = "f",   type = "f64" },
  { name = "uu",  type = "uuid" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["kinds"]
actions = ["everything"]
`);
  const session = server.client().session();
  const id = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);

  await session.insert("kinds", [
    uint(1),
    bool(true),
    bytes(new Uint8Array([0xde, 0xad])),
    str("hello"),
    int(-5),
    uint(5),
    float(1.5),
    uuid(id),
  ]);

  const row = await session.get("kinds", [uint(1)]);
  assert.ok(row);
  assert.equal((row[1] as { value: boolean }).value, true);
  assert.deepEqual(Array.from((row[2] as { value: Uint8Array }).value), [0xde, 0xad]);
  assert.equal((row[3] as { value: string }).value, "hello");
  assert.equal((row[4] as { value: bigint }).value, -5n);
  expectUint(row[5], 5n, "the u64 column");
  assert.equal((row[6] as { value: number }).value, 1.5);
  assert.deepEqual(Array.from((row[7] as { value: Uint8Array }).value), Array.from(id));

  // An i64 and a u64 must not decode to the same value even at the same
  // magnitude — this server orders type-first.
  assert.notEqual(row[4]?.kind, row[5]?.kind);
});

// A u64 above 2^53 must survive, which is the whole reason values are bigint
// rather than number. A `number` would lose the low bits silently, and a
// primary key is where that shows up latest.
test("a u64 past the double's precision survives", async () => {
  const session = (await serving()).client().session();
  const big = 9_007_199_254_740_993n; // 2^53 + 1

  await session.insert("docs", [uint(big), str("big"), int(1)]);
  const row = await session.get("docs", [uint(big)]);
  assert.ok(row, "a large key must be findable again");
  expectUint(row[0], big, "the large primary key");
});

test("explain needs the explain grant", async () => {
  const server = await serving(`
[[tables]]
name = "plain"
id = 3
columns = [{ name = "id", type = "u64" }]
primary_key = ["id"]

[[security.grants]]
role = "reader"
tables = ["plain"]
actions = ["all"]
`);
  const session = server
    .client({ principal: "u64:2", tenant: "u64:1", roles: ["reader"] })
    .session();

  await assert.rejects(
    () => session.explain({ table: "plain" }),
    (error: unknown) => {
      assert.ok(
        isKind(error, "permission-denied"),
        `a read grant must not carry EXPLAIN, got ${(error as SlateError).kind}`,
      );
      return true;
    },
  );
});

test("explain with the grant", async () => {
  const session = (await serving()).client().session();
  await session.insert("docs", [uint(1), str("k"), int(1)]);

  const plan = await session.explain({ table: "docs" });
  assert.equal(plan.table, "docs");
  assert.notEqual(plan.access, "", "a plan should name its access path");
});

test("leadership", async () => {
  const status = await (await serving()).client().leadership();
  assert.equal(status.leader, true, "a lone node should hold the lease");
});

// A session's watermark must advance past its own writes, which is what makes
// a later read see them.
test("the session watermark advances on write", async () => {
  const session = (await serving()).client().session();
  assert.equal(session.watermark, undefined, "a fresh session has no watermark");

  await session.insert("docs", [uint(1), str("k"), int(1)]);
  const first = session.watermark;
  assert.ok(first !== undefined && first > 0n, "a write must advance the watermark");

  await session.insert("docs", [uint(2), str("k"), int(2)]);
  const second = session.watermark;
  assert.ok(
    second !== undefined && second > first,
    "a second write must advance it further, not merely set it once",
  );
});

test("an unknown table is not-found", async () => {
  const session = (await serving()).client().session();
  await assert.rejects(
    () => session.get("nosuchtable", [uint(1)]),
    (error: unknown) => {
      assert.ok(isKind(error, "not-found"), `kind was ${(error as SlateError).kind}`);
      return true;
    },
  );
});

// A null is not a zero and not an absent column, and the round trip has to
// keep all three apart. `docs.kind` is not nullable — the server refuses one
// there, correctly — so this needs a column that is.
test("a null value is not a zero", async () => {
  const server = await serving(`
[[tables]]
name = "maybe"
id = 4
columns = [
  { name = "id",   type = "u64" },
  { name = "note", type = "str", nullable = true },
  { name = "n",    type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["maybe"]
actions = ["everything"]
`);
  const session = server.client().session();
  await session.insert("maybe", [uint(1), nullValue, int(0)]);

  const row = await session.get("maybe", [uint(1)]);
  assert.equal(row?.[1]?.kind, "null", "a written null must come back null");
  assert.equal(row?.[2]?.kind, "int", "and a zero must stay an int");
  assert.equal((row?.[2] as { value: bigint }).value, 0n);
});

// The server refuses a null in a column that is not nullable, and the client
// must surface that as an ordinary refusal rather than swallowing it.
test("a null in a non-nullable column is refused", async () => {
  const session = (await serving()).client().session();
  await assert.rejects(
    () => session.insert("docs", [uint(1), nullValue, int(0)]),
    (error: unknown) => {
      assert.ok(isKind(error, "invalid-request"), `kind was ${(error as SlateError).kind}`);
      assert.match((error as SlateError).message, /nullable/);
      return true;
    },
  );
});
