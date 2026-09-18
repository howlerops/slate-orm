import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import { int, isKind, str, uint, type Transaction } from "../src/index.js";

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

async function serving() {
  const server = await start("");
  servers.push(server);
  return server;
}

/**
 * `transact` retries a conflict, and loses no update.
 *
 * Two writers read the same counter and both increment it. Without the barrier
 * they would almost always run serially and the test would prove nothing: the
 * retry would never fire, and a `transact` that did not retry at all would
 * pass. With it, both have read before either commits, so one must conflict.
 */
test("transact retries a conflict and loses no update", async () => {
  const server = await serving();
  const setup = server.client().session();
  await setup.insert("docs", [uint(900), str("counter"), int(0)]);

  let waiting = 0;
  let release!: () => void;
  const both = new Promise<void>((resume) => {
    release = resume;
  });
  const attempts = [0, 0];

  const bump = async (who: number) => {
    const session = server.client().session();
    await session.transact(async (tx: Transaction) => {
      attempts[who] = (attempts[who] ?? 0) + 1;
      const row = await tx.get("docs", [uint(900)]);
      assert.ok(row, "the counter row vanished");
      const size = row[2];
      assert.ok(size?.kind === "int", "size is not an i64");
      if (attempts[who] === 1) {
        // Only on the first attempt: waiting again would deadlock once the two
        // are no longer running in step.
        waiting += 1;
        if (waiting === 2) release();
        await both;
      }
      await tx.update("docs", [uint(900), str("counter"), int(size.value + 1n)]);
    });
  };

  await Promise.all([bump(0), bump(1)]);

  // Both increments landed: 0 -> 2. A lost update leaves 1, which is exactly
  // what a `transact` that swallowed the conflict would produce.
  const row = await setup.get("docs", [uint(900)]);
  assert.ok(row);
  assert.deepEqual(row[2], { kind: "int", value: 2n }, "an update was lost");
  // And somebody actually retried, or the barrier did not do its job and this
  // test is passing for the wrong reason.
  assert.ok(
    attempts[0]! + attempts[1]! >= 3,
    `no retry happened: attempts ${attempts} — the conflict was not forced`,
  );
});

/**
 * A duplicate key fails identically forever; retrying it is a hang.
 *
 * `RecordStore::transact`'s rule, restated on this side.
 */
test("transact does not retry what can never succeed", async () => {
  const server = await serving();
  const session = server.client().session();
  await session.insert("docs", [uint(901), str("taken"), int(1)]);

  let tries = 0;
  await assert.rejects(
    () =>
      session.transact(async (tx) => {
        tries += 1;
        await tx.insert("docs", [uint(901), str("again"), int(2)]);
      }),
    (error: unknown) => isKind(error, "already-exists"),
  );
  assert.equal(tries, 1, "a permanent failure was retried");
});

test("transact refuses fewer than one attempt", async () => {
  const server = await serving();
  const session = server.client().session();
  await assert.rejects(
    () => session.transact(async () => 1, { attempts: 0 }),
    (error: unknown) => error instanceof RangeError,
  );
});
