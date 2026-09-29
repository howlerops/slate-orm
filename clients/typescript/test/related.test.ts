import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  bytes,
  eq,
  grpc,
  int,
  isKind,
  str,
  uint,
  uuid,
  valueKey,
  type Relation,
  type Session,
  type Value,
} from "../src/index.js";

// A parent and a child with a real foreign key.
//
// Not the `authors`/`books` fixture the join tests use: that pair deliberately
// has no constraint between them — one book there names an author that does
// not exist, and is called `orphan` — so no relationship can be named through
// it. A relationship *is* a foreign key here, so this declares one.
const RELATED_TABLES = `
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
columns = [
  { name = "id",         type = "u64" },
  { name = "library_id", type = "u64" },
  { name = "label",      type = "str" },
]
primary_key = ["id"]

[[tables.foreign_keys]]
name = "shelf_library"
parent = "libraries"
columns = ["library_id"]

[[security.grants]]
role = "app"
tables = ["libraries", "shelves"]
actions = ["everything"]
`;

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

const CHILDREN: Relation = { on: "shelves", through: "shelf_library", way: "children" };
const PARENTS: Relation = { on: "shelves", through: "shelf_library", way: "parents" };

/**
 * A node holding three libraries and three shelves.
 *
 * Library 1 has two shelves, library 2 has one, library 3 has none — so a
 * parent with nothing related is exercised and the per-parent counts are not
 * uniform, which is what makes the grouping assertions mean anything.
 */
async function shelved(): Promise<Session> {
  const server = await start(RELATED_TABLES);
  servers.push(server);
  const session = server.client().session();

  for (const library of [
    [uint(1), str("main")],
    [uint(2), str("annexe")],
    [uint(3), str("empty")],
  ]) {
    await session.insert("libraries", library);
  }
  for (const shelf of [
    [uint(100), uint(1), str("history")],
    [uint(101), uint(1), str("poetry")],
    [uint(102), uint(2), str("maps")],
  ]) {
    await session.insert("shelves", shelf);
  }
  return session;
}

/** The `label` column of every row in one group. */
function labels(groups: Value[][][], at: number): string[] {
  return groups[at]!.map((row) => {
    const label = row[2]!;
    assert.equal(label.kind, "string", `label is ${label.kind}`);
    return String((label as { value: string }).value);
  });
}

test("children come back grouped per parent", async () => {
  const session = await shelved();
  const got = await session.related("shelves", CHILDREN, [uint(1), uint(2), uint(3)]);

  assert.equal(got.length, 3);
  assert.deepEqual(labels(got, 0).sort(), ["history", "poetry"]);
  assert.deepEqual(labels(got, 1), ["maps"]);
  // Empty, not missing: the caller indexes this by its own loop counter.
  assert.deepEqual(got[2], []);
});

test("parents reads the relationship the other way", async () => {
  const session = await shelved();
  const got = await session.related("libraries", PARENTS, [uint(1), uint(2)]);

  assert.equal(got.length, 2);
  for (const [at, want] of [["main"], ["annexe"]].entries()) {
    assert.equal(got[at]!.length, 1);
    const name = got[at]![0]![1]!;
    assert.deepEqual(name, str(want[0]!));
  }
});

// Two parents with the same key both get the rows, from the one group the
// server sent — which is the saving, on the response as well as on the read.
test("a repeated key is one group and two answers", async () => {
  const session = await shelved();
  const got = await session.related("shelves", CHILDREN, [uint(1), uint(1)]);

  assert.equal(got.length, 2);
  assert.deepEqual(labels(got, 0).sort(), ["history", "poetry"]);
  assert.deepEqual(labels(got, 1).sort(), ["history", "poetry"]);
});

test("no parents asks for nothing", async () => {
  const session = await shelved();
  assert.deepEqual(await session.related("shelves", CHILDREN, []), []);
});

test("an unknown foreign key names the ones that exist", async () => {
  const session = await shelved();
  await assert.rejects(
    () => session.related("shelves", { on: "shelves", through: "nosuch" }, [uint(1)]),
    (error: unknown) => {
      assert.ok(isKind(error, "invalid-request"), `kind: ${String(error)}`);
      assert.match(String(error), /shelf_library/);
      return true;
    },
  );
});

// The default is children, so a relation written without a `way` reads the
// same as one that says so. Worth pinning: it is the commoner direction and
// the one a caller will leave off.
test("children is the default direction", async () => {
  const session = await shelved();
  const explicit = await session.related("shelves", CHILDREN, [uint(1)]);
  const implicit = await session.related(
    "shelves",
    { on: "shelves", through: "shelf_library" },
    [uint(1)],
  );
  assert.deepEqual(implicit, explicit);
});

/**
 * The oracle: one relationship load equals a filtered read per parent.
 *
 * A second, independent way to the same answer, so a `related` that dropped or
 * duplicated a row disagrees with it — rather than with a hand-written
 * expectation copied from the seed data above, which would agree with the same
 * misreading.
 */
test("related agrees with filtering once per parent", async () => {
  const session = await shelved();
  const keys = [uint(1), uint(2), uint(3)];
  const related = await session.related("shelves", CHILDREN, keys);

  for (const [at, key] of keys.entries()) {
    const byHand: Value[][] = [];
    for await (const row of session.query({
      table: "shelves",
      filter: eq(1, key),
    })) {
      byHand.push(row);
    }
    assert.deepEqual(
      related[at]!.map((row) => row.map(valueKey)).sort(),
      byHand.map((row) => row.map(valueKey)).sort(),
      `library ${valueKey(key)}`,
    );
  }
});

test("a relationship read in a transaction sees its own writes", async () => {
  const session = await shelved();
  const tx = await session.begin();
  try {
    await tx.insert("shelves", [uint(103), uint(3), str("atlases")]);

    const inside = await tx.related("shelves", CHILDREN, [uint(3)]);
    assert.equal(inside[0]!.length, 1, "the transaction should see its own shelf");

    const outside = await session.related("shelves", CHILDREN, [uint(3)]);
    assert.deepEqual(outside[0], [], "an uncommitted shelf is not visible outside");
  } finally {
    await tx.rollback();
  }
});

// The schema claim rides on `related` too.
//
// It is attached at every call site by hand, so a new one that forgets it is
// simply not checked and nothing else notices.
test("the schema claim rides on a relationship load", async () => {
  const server = await start(RELATED_TABLES);
  servers.push(server);
  // `shelves`, with two of its columns declared the wrong way round.
  const session = server
    .client()
    .declaring({
      shelves: {
        name: "shelves",
        columns: [
          { name: "id", type: "u64" },
          { name: "label", type: "string" },
          { name: "library_id", type: "u64" },
        ],
        primaryKey: ["id"],
      },
    })
    .session();

  await assert.rejects(
    () => session.related("shelves", CHILDREN, [uint(1)]),
    (error: unknown) => isKind(error, "invalid-request"),
  );
});

/**
 * The grouping key distinguishes kinds, which nothing above can prove.
 *
 * Within one relationship load the keys all come from one column and are all
 * one kind, so dropping the kind from the key changes no answer there — a
 * mutation doing exactly that survived every test above. `valueKey` is
 * exported, though, and a caller keying its own map across two tables is
 * exactly where `7` as an `i64` and `7` as a `u64` would collide.
 */
test("the grouping key tells an i64 from a u64", () => {
  assert.notEqual(valueKey(int(7)), valueKey(uint(7)));
  assert.equal(valueKey(uint(7)), valueKey(uint(7)));
  // And bytes from the uuid with the same sixteen bytes.
  const sixteen = new Uint8Array(16).fill(3);
  assert.notEqual(valueKey(bytes(sixteen)), valueKey(uuid(sixteen)));
});

/**
 * Every request this client sent, by RPC path, with its message.
 *
 * The freshness floor and the request count are properties of the *request*,
 * not of the answer: a client that dropped the floor returns exactly the right
 * rows, and nothing assertable about the result distinguishes it. So the
 * request itself is what has to be looked at.
 *
 * The Go suite has done this since 2026-09-16 and this one could not:
 * `Client.connect` had no argument to pass an interceptor through until
 * `ledger/2026-09-28-the-third-client-counts-and-the-go-instrument-was-half-blind.md`
 * added `options`, which is the whole reason that entry left the caveat this
 * closes.
 *
 * One interceptor covers a unary write and a server-streaming read both,
 * because grpc-js runs `interceptors` on every call regardless of shape — Go
 * needs two, and the Go adapter shipped with only one for a day. `sendMessage`
 * rather than the call itself, because the message is what carries the field.
 */
function recording(seen: (method: string, message: unknown) => void) {
  return {
    interceptors: [
      (options: grpc.InterceptorOptions, nextCall: grpc.NextCall) =>
        new grpc.InterceptingCall(nextCall(options), {
          sendMessage(message: unknown, next: (message: unknown) => void) {
            seen(options.method_definition.path, message);
            next(message);
          },
        }),
    ],
  };
}

test("a session that has written sends a freshness floor, monotonic or not", async () => {
  const server = await start(RELATED_TABLES);
  servers.push(server);

  const related: unknown[] = [];
  const client = server.client(undefined, recording((method, message) => {
    if (method.endsWith("/Related")) related.push(message);
  }));

  // A write first: the floor is the watermark, and a session that has read and
  // written nothing has no watermark to send. Without this the assertion below
  // would pass against a client that never sets the field at all.
  const session = client.session();
  await session.insert("libraries", [uint(1), str("main")]);
  await session.related("shelves", CHILDREN, [uint(1)]);

  assert.equal(related.length, 1, "want exactly one Related request");
  assert.ok(
    (related[0] as { freshness?: unknown }).freshness,
    `the request carried no freshness floor: ${JSON.stringify(related[0])}`,
  );

  // A session that does not want monotonic reads still reads its own writes,
  // so it *also* carries a floor once it has written.
  //
  // This asserted the opposite until 2026-09-29, when comparing the three
  // clients found Python doing it this way and the other two not: gating the
  // floor on the flag dropped read-your-writes along with monotonic reads, and
  // a caller who wrote and read back could miss their own write with no error
  // anywhere. See `ledger/2026-09-29-read-your-writes-is-not-monotonic-reads.md`.
  //
  // The write still comes first, and is still the point: without it the
  // session has no watermark and this would hold for a reason that has nothing
  // to do with the flag.
  related.length = 0;
  const loose = client.sessionWithoutMonotonicReads();
  await loose.insert("libraries", [uint(2), str("annexe")]);
  await loose.related("shelves", CHILDREN, [uint(1)]);

  assert.equal(related.length, 1, "want exactly one Related request");
  assert.ok(
    (related[0] as { freshness?: unknown }).freshness,
    "a non-monotonic session that wrote still reads its own writes, so it " +
      `carries a floor: ${JSON.stringify(related[0])}`,
  );
});

test("a read does not advance a non-monotonic session's watermark", async () => {
  // The half that gives the flag its meaning, and what stops the pair above
  // being "the field is always set". This is the read-only caller the flag is
  // named for — a dashboard, a cache warmer — and the one whose behaviour did
  // not change when read-your-writes was restored.
  //
  // **Two reads, not one.** With a single read there is nothing for the first
  // to have folded in, so the assertion would hold against a client that
  // ignored the flag entirely. The Go sibling of this test was written with
  // one read and a mutation removing the guard survived it.
  const server = await start(RELATED_TABLES);
  servers.push(server);

  const related: unknown[] = [];
  const client = server.client(undefined, recording((method, message) => {
    if (method.endsWith("/Related")) related.push(message);
  }));

  const loose = client.sessionWithoutMonotonicReads();
  await loose.related("shelves", CHILDREN, [uint(1)]);
  await loose.related("shelves", CHILDREN, [uint(1)]);

  assert.equal(related.length, 2, "want two Related requests");
  assert.equal(
    (related[1] as { freshness?: unknown }).freshness ?? undefined,
    undefined,
    "a read must not advance a non-monotonic session's watermark, so the " +
      `second read carries no floor: ${JSON.stringify(related[1])}`,
  );
});
