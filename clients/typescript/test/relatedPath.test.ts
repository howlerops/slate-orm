import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import { isKind, str, uint, type Session, type Step, type Value } from "../src/index.js";

/**
 * Three tables and two foreign keys, so a relationship *path* has somewhere to
 * go: `libraries → shelves → copies`.
 *
 * A path needs two keys where a relationship needs one, which is why this is
 * not the `related` fixture with a row added.
 */
const PATH_TABLES = `
[[tables]]
name = "libraries"
id = 20
columns = [
  { name = "id",   type = "u64" },
  { name = "name", type = "str" },
]
primary_key = ["id"]

[[tables]]
name = "shelves"
id = 21
# "region" sits in front of "id" on purpose, so that shelves.id is ordinal 1
# rather than 0.
#
# The second level's key_ordinal is the primary key of the level above, and
# with "id" at ordinal 0 the number a client must read off the wire is the same
# number it would get by hardcoding zero. A mutation that ignored key_ordinal
# entirely passed every test in this file until this column existed - it was
# caught only by the Python suite, whose fixture is tenant-scoped and so has
# "id" at ordinal 1 by accident. A fixture where the right answer is zero
# cannot tell "read the ordinal" from "assume the first column".
columns = [
  { name = "region",     type = "str" },
  { name = "id",         type = "u64" },
  { name = "library_id", type = "u64" },
  { name = "label",      type = "str" },
]
primary_key = ["id"]

[[tables.foreign_keys]]
name = "shelf_library"
parent = "libraries"
columns = ["library_id"]

[[tables]]
name = "copies"
id = 22
columns = [
  { name = "id",       type = "u64" },
  { name = "shelf_id", type = "u64" },
  { name = "barcode",  type = "str" },
]
primary_key = ["id"]

[[tables.foreign_keys]]
name = "copy_shelf"
parent = "shelves"
columns = ["shelf_id"]

[[security.grants]]
role = "app"
tables = ["libraries", "shelves", "copies"]
actions = ["everything"]
`;

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

const PATH: Step[] = [
  { on: "shelves", through: "shelf_library", way: "children", table: "shelves" },
  { on: "copies", through: "copy_shelf", way: "children", table: "copies" },
];

/**
 * Two libraries, four shelves, four copies.
 *
 * Shelf 103 is deliberately bare. A middle row with nothing below it separates
 * "the rows at the bottom level" from "the rows with no children", and those
 * two readings agree on every other input — the Python client shipped the
 * second one for an hour and handed back a shelf where a copy was asked for.
 */
async function stacked(): Promise<Session> {
  const server = await start(PATH_TABLES);
  servers.push(server);
  const session = server.client().session();

  for (const library of [
    [uint(1), str("main")],
    [uint(2), str("annexe")],
  ]) {
    await session.insert("libraries", library);
  }
  for (const shelf of [
    [str("north"), uint(100), uint(1), str("alpha")],
    [str("north"), uint(101), uint(1), str("beta")],
    [str("south"), uint(102), uint(2), str("gamma")],
    [str("south"), uint(103), uint(2), str("bare")],
  ]) {
    await session.insert("shelves", shelf);
  }
  for (const copy of [
    [uint(200), uint(100), str("a-1")],
    [uint(201), uint(100), str("a-2")],
    [uint(202), uint(101), str("b-1")],
    [uint(203), uint(102), str("g-1")],
  ]) {
    await session.insert("copies", copy);
  }
  return session;
}

function textAt(row: Value[], at: number): string {
  const value = row[at]!;
  assert.equal(value.kind, "string", `value ${at} is not a string`);
  return (value as { value: string }).value;
}

test("a path returns a tree per parent", async () => {
  const session = await stacked();
  const trees = await session.relatedPath(PATH, [uint(1), uint(2)]);

  assert.equal(trees.length, 2, "one tree per key");
  assert.deepEqual(
    trees.map((tree) => tree.map((node) => textAt(node.row, 3))),
    [
      ["alpha", "beta"],
      ["gamma", "bare"],
    ],
  );
  assert.deepEqual(
    trees.map((tree) =>
      tree.map((node) => node.related.map((leaf) => textAt(leaf.row, 2))),
    ),
    [
      [["a-1", "a-2"], ["b-1"]],
      [["g-1"], []],
    ],
  );
});

test("relatedThrough drops the middle level", async () => {
  const session = await stacked();
  const through = await session.relatedThrough(PATH, [uint(1), uint(2)]);
  // Library 2's bare shelf contributes nothing at all — not itself, which is
  // what "nodes with no children" would have returned.
  assert.deepEqual(
    through.map((rows) => rows.map((row) => textAt(row, 2))),
    [["a-1", "a-2", "b-1"], ["g-1"]],
  );
});

test("a parent with nothing related gets an empty tree", async () => {
  const session = await stacked();
  const trees = await session.relatedPath(PATH, [uint(9)]);
  assert.deepEqual(trees, [[]]);
});

test("an empty path is refused by the client", async () => {
  const session = await stacked();
  await assert.rejects(
    () => session.relatedPath([], [uint(1)]),
    (error: unknown) => isKind(error, "invalid-request"),
  );
});

test("a path past the depth limit is refused by name", async () => {
  const session = await stacked();
  // The shipped limit is four; six steps is past it. The path also does not
  // compose, and the depth is checked first on purpose — the limit bounds the
  // resolution that would otherwise happen.
  const deep = [...PATH, ...PATH, ...PATH];
  await assert.rejects(
    () => session.relatedPath(deep, [uint(1)]),
    (error: unknown) => String(error).includes("max_relation_depth"),
  );
});
