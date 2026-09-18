import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import { SlateError, type Session, type Value, int, str, uint } from "../src/index.js";

/**
 * A conditional delete.
 *
 * `updateIfUnchanged` shipped first because overwriting somebody else's edit
 * is the loss everybody recognises. Deleting a row somebody else just edited
 * is the same mistake: the caller read the row, decided *from what it said*
 * that it should go, and by the time the delete lands it says something else.
 *
 * The one place this is not simply the update's twin: a plain delete reports
 * an absent key in `affected`, and a conditional one is refused with
 * `NOT_FOUND`. The caller said what it expected to find, so "it was already
 * gone" is an answer it wants rather than a smaller count it will read as
 * success.
 */
const CONDITIONAL_TABLES = `
[[tables]]
name = "notes"
id = 61
columns = [
  { name = "id",   type = "u64" },
  { name = "body", type = "str" },
  { name = "size", type = "i64" },
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

const noteRow = (id: number, size: number): Value[] => [
  uint(id),
  str("note"),
  int(size),
];
const noteKey = (id: number): Value[] => [uint(id)];

/** A node holding four notes, ids 0..3, size = id. */
async function noted(): Promise<Session> {
  const server = await start(CONDITIONAL_TABLES);
  servers.push(server);
  const session = server.client().session();
  await session.insert("notes", ...[0, 1, 2, 3].map((id) => noteRow(id, id)));
  return session;
}

async function present(session: Session, id: number): Promise<boolean> {
  return (await session.get("notes", noteKey(id))) !== undefined;
}

test("a delete naming the row it read is applied", async () => {
  const session = await noted();
  const result = await session.deleteIfUnchanged("notes", {
    key: noteKey(2),
    was: noteRow(2, 2),
  });
  assert.equal(result.affected, 1n);
  assert.equal(await present(session, 2), false);
});

test("a delete naming a row that moved is refused", async () => {
  const session = await noted();
  await session.update("notes", noteRow(2, 5000));

  await assert.rejects(
    () => session.deleteIfUnchanged("notes", { key: noteKey(2), was: noteRow(2, 2) }),
    SlateError,
  );
  assert.equal(await present(session, 2), true);
});

test("a row already gone is refused rather than counted as absent", async () => {
  // Both halves, because the refusal only means something beside the zero it
  // replaces.
  const session = await noted();
  await session.delete("notes", noteKey(3));

  const plain = await session.delete("notes", noteKey(3));
  assert.equal(plain.affected, 0n, "a plain delete of an absent key is not an error");

  await assert.rejects(
    () => session.deleteIfUnchanged("notes", { key: noteKey(3), was: noteRow(3, 3) }),
    (error: unknown) => {
      assert.ok(error instanceof SlateError);
      // `not-found` and not `conflict`: a row that moved can be re-read and
      // the decision remade, and a row that is gone cannot, so a caller
      // retrying a conflict would loop.
      assert.equal(error.kind, "not-found", `${error.kind}: ${error.message}`);
      return true;
    },
  );
});

test("a stale first key refuses the whole statement", async () => {
  const session = await noted();
  await session.update("notes", noteRow(0, 99));

  await assert.rejects(
    () =>
      session.deleteIfUnchanged(
        "notes",
        { key: noteKey(0), was: noteRow(0, 0) },
        { key: noteKey(1), was: noteRow(1, 1) },
      ),
    SlateError,
  );
  assert.equal(
    await present(session, 1),
    true,
    "the second row was deleted by a refused statement",
  );
});

test("a stale conditional delete inside a transaction is refused", async () => {
  // The session path is separate code from the autocommit one, and only a
  // stale row tells a conditional delete apart from a plain one.
  const session = await noted();
  await session.update("notes", noteRow(2, 77));

  const tx = await session.begin();
  await assert.rejects(
    () => tx.deleteIfUnchanged("notes", { key: noteKey(2), was: noteRow(2, 2) }),
    SlateError,
  );
  await tx.rollback();
  assert.equal(await present(session, 2), true);
});

test("no row deletes is a delete of nothing", async () => {
  const session = await noted();
  const result = await session.deleteIfUnchanged("notes");
  assert.equal(result.affected, 0n);
  assert.equal(await present(session, 2), true);
});
