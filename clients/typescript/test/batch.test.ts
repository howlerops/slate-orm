import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  ge,
  lit,
  SlateError,
  type Batch,
  type Session,
  type Value,
  str,
  uint,
} from "../src/index.js";

/**
 * Several writes in one round trip, and the distinction that makes it safe.
 *
 * A batch is a network optimisation; a transaction is an atomicity guarantee.
 * They differ *only when something fails*, which is why `atomicity` has no
 * default here or on the wire. The two tests named `...carries on past a
 * failure` and `...undoes everything before the failure` are the same three
 * operations under the two guarantees, so the difference reads in one screen.
 */
const BATCH_TABLES = `
[[tables]]
name = "notes"
id = 43
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

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

async function batched(): Promise<Session> {
  const server = await start(BATCH_TABLES);
  servers.push(server);
  return server.client().session();
}

function note(id: number): Value[] {
  return [uint(id), str("note")];
}

async function present(session: Session): Promise<number> {
  let seen = 0;
  for await (const _ of session.query({ table: "notes" })) seen++;
  return seen;
}

test("an empty batch is refused before it is sent", async () => {
  const session = await batched();
  await assert.rejects(
    () => session.batch({ atomicity: "independent", operations: [] }),
    (error: Error) => {
      assert.match(error.message, /at least one/);
      return true;
    },
  );
});

test("an independent batch applies every operation", async () => {
  const session = await batched();
  const batch: Batch = {
    atomicity: "independent",
    operations: [1, 2, 3, 4, 5].map((id) => ({
      kind: "insert" as const,
      table: "notes",
      rows: [note(id)],
    })),
  };
  const result = await session.batch(batch);

  assert.equal(result.outcomes.length, 5, "one outcome per operation");
  for (const one of result.outcomes) {
    assert.equal(one.error, undefined);
    assert.equal(one.written?.affected, 1n);
  }
  assert.notEqual(result.sequence, undefined);
  assert.equal(await present(session), 5);
});

test("an independent batch carries on past a failure", async () => {
  const session = await batched();
  await session.insert("notes", note(2));

  const result = await session.batch({
    atomicity: "independent",
    operations: [
      { kind: "insert", table: "notes", rows: [note(1)] },
      { kind: "insert", table: "notes", rows: [note(2)] }, // already there
      { kind: "insert", table: "notes", rows: [note(3)] },
    ],
  });

  assert.equal(result.outcomes[0]?.error, undefined);
  assert.equal(result.outcomes[2]?.error, undefined);
  const failed = result.outcomes[1]?.error;
  assert.ok(failed instanceof SlateError, `expected a SlateError, got ${failed}`);
  assert.equal(failed.kind, "already-exists");
  // The stable token survives the trip through a message body, which is the
  // half a lone call gets from its trailers.
  assert.equal(failed.reason, "DUPLICATE_PRIMARY_KEY");

  // 1 and 3 landed although 2 failed between them. That is independence, and
  // what a caller who wanted a transaction would be horrified by.
  assert.equal(await present(session), 3);
});

test("an atomic batch undoes everything before the failure", async () => {
  const session = await batched();
  await session.insert("notes", note(2));

  await assert.rejects(() =>
    session.batch({
      atomicity: "all-or-nothing",
      operations: [
        { kind: "insert", table: "notes", rows: [note(1)] },
        { kind: "insert", table: "notes", rows: [note(2)] }, // already there
        { kind: "insert", table: "notes", rows: [note(3)] },
      ],
    }),
  );

  // Only the seeded row: the same three operations as the test above.
  assert.equal(await present(session), 1);
});

test("an atomic batch reports no per-operation outcomes", async () => {
  const session = await batched();
  const result = await session.batch({
    atomicity: "all-or-nothing",
    operations: [1, 2, 3].map((id) => ({
      kind: "insert" as const,
      table: "notes",
      rows: [note(id)],
    })),
  });
  assert.deepEqual(result.outcomes, [], "they all happened; nothing to report");
  assert.notEqual(result.sequence, undefined);
  assert.equal(await present(session), 3);
});

test("a batch carries every kind of write", async () => {
  const session = await batched();
  await session.insert("notes", ...[1, 2, 3, 4, 5, 6].map(note));

  const result = await session.batch({
    atomicity: "independent",
    operations: [
      { kind: "insert", table: "notes", rows: [note(7)] },
      { kind: "insert", table: "notes", rows: [[uint(7), str("replaced")]], upsert: true },
      { kind: "delete", table: "notes", keys: [[uint(1)]] },
      {
        kind: "deleteWhere",
        write: { table: "notes", filter: ge(0, uint(6)), returning: true },
      },
      {
        kind: "updateWhere",
        write: { table: "notes", set: [{ column: 1, value: lit(str("touched")) }] },
      },
    ],
  });

  for (const [at, one] of result.outcomes.entries()) {
    assert.equal(one.error, undefined, `operation ${at} failed: ${one.error}`);
  }
  // The deleteWhere asked for its rows: 6 and 7.
  assert.equal(result.outcomes[3]?.written?.rows.length, 2);
  // The updateWhere did not ask, so it returns none.
  assert.equal(result.outcomes[4]?.written?.rows.length, 0);
  assert.equal(await present(session), 4);
});

test("an atomic batch joins an open transaction", async () => {
  const session = await batched();
  const tx = await session.begin();
  const result = await tx.batch({
    atomicity: "all-or-nothing",
    operations: [1, 2, 3].map((id) => ({
      kind: "insert" as const,
      table: "notes",
      rows: [note(id)],
    })),
  });
  // No sequence: the transaction has not committed, and the batch is not the
  // thing that commits it.
  assert.equal(result.sequence, undefined);
  await tx.rollback();
  assert.equal(await present(session), 0, "the rollback undid it");
});

test("an independent batch may not run inside a transaction", async () => {
  const session = await batched();
  const tx = await session.begin();
  await assert.rejects(() =>
    tx.batch({
      atomicity: "independent",
      operations: [{ kind: "insert", table: "notes", rows: [note(1)] }],
    }),
  );
  await tx.rollback();
});
