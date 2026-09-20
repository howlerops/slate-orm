import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  SlateError,
  type Session,
  type Value,
  array,
  int,
  nullValue,
  str,
  uint,
} from "../src/index.js";

/**
 * An array column, through a real node.
 *
 * The kernel stores arrays and the wire carries them; this is the TypeScript
 * half of proving a list survives the round trip with its elements' *kinds*
 * intact and not merely their text. A `["1"]` decoded as strings and a `[1]`
 * decoded as integers are different rows to this server — its ordering is
 * type-first — so every assertion below checks the element's kind.
 *
 * Two element types on one table, because a client with one array column can
 * hard-code the element type it decodes and pass.
 */
const ARRAY_TABLES = `
[[tables]]
name = "posts"
id = 61
columns = [
  { name = "id",     type = "u64" },
  { name = "title",  type = "str" },
  { name = "tags",   type = "array", element = "str" },
  { name = "scores", type = "array", element = "i64", nullable = true },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["posts"]
actions = ["everything"]
`;

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

function postRow(
  id: number,
  title: string,
  tags: string[],
  scores: number[] | null,
): Value[] {
  return [
    uint(id),
    str(title),
    array(tags.map(str)),
    scores === null ? nullValue : array(scores.map((n) => int(n))),
  ];
}

async function posted(): Promise<Session> {
  const server = await start(ARRAY_TABLES);
  servers.push(server);
  const session = server.client().session();
  await session.insert(
    "posts",
    postRow(1, "first", ["rust", "db"], [3, 1]),
    postRow(2, "second", ["db"], null),
    postRow(3, "third", [], []),
  );
  return session;
}

async function postAt(session: Session, id: number): Promise<Value[]> {
  const row = await session.get("posts", [uint(id)]);
  assert.ok(row, `row ${id} is not there`);
  return row;
}

test("an array round trips through the server", async () => {
  const session = await posted();
  const row = await postAt(session, 1);

  const tags = row[2];
  // Not a soft check: an array arriving as something else is the whole
  // failure this file is about, and every assertion after it would be about
  // the wrong kind.
  assert.equal(tags?.kind, "array", `tags came back as ${tags?.kind}`);
  assert.deepEqual(tags, array([str("rust"), str("db")]));

  const scores = row[3];
  assert.equal(scores?.kind, "array");
  const elements = (scores as { kind: "array"; value: Value[] }).value;
  // The element's kind, not only its number. An `int` arriving as a `uint`
  // would print the same and would not compare equal to the stored value.
  assert.equal(elements[0]?.kind, "int");
  assert.deepEqual(elements, [int(3n), int(1n)]);
});

test("an empty array is not a null", async () => {
  // The distinction a list type loses first: "no scores" and "scores unknown"
  // are different values, and they encode differently — the array tag and a
  // terminator against a bare null tag.
  const session = await posted();

  const third = await postAt(session, 3);
  assert.deepEqual(third[3], array([]));

  const second = await postAt(session, 2);
  assert.equal(second[3]?.kind, "null");
});

test("an element of the wrong type is refused", async () => {
  // `array` cannot express its element type — that lives on the column, the
  // way a decimal's scale does — so a mixed list type-checks. The server is
  // the only thing that knows, and it must say which element.
  const session = await posted();
  await assert.rejects(
    () =>
      session.insert("posts", [
        uint(9),
        str("bad"),
        array([str("ok"), int(7n)]),
        nullValue,
      ]),
    (error: unknown) => {
      assert.ok(error instanceof SlateError);
      assert.match(error.message, /element 1/);
      return true;
    },
  );
});

test("a nested array is refused", async () => {
  // Refused by the server before the kernel sees it: the depth is chosen by
  // whoever sends the message, and refusing at depth one means there is no
  // depth to bound.
  const session = await posted();
  await assert.rejects(
    () =>
      session.insert("posts", [
        uint(10),
        str("nested"),
        array([array([str("inner")])]),
        nullValue,
      ]),
    SlateError,
  );
});
