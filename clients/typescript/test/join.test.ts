import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  agg,
  at,
  count,
  groupGt,
  groupKey,
  int,
  isKind,
  key0,
  maxOf,
  newJoin,
  SlateError,
  str,
  uint,
  type Group,
  type JoinQuery,
  type JoinType,
  type Session,
} from "../src/index.js";

// Authors and their books, the shape the Rust wire tests and the Go client
// both join.
const JOIN_TABLES = `
[[tables]]
name = "authors"
id = 10
columns = [
  { name = "id",      type = "u64" },
  { name = "name",    type = "str" },
  { name = "country", type = "str" },
]
primary_key = ["id"]

[[tables]]
name = "books"
id = 11
columns = [
  { name = "id",        type = "u64" },
  { name = "author_id", type = "u64" },
  { name = "title",     type = "str" },
  { name = "year",      type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["authors", "books"]
actions = ["everything"]
`;

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

/**
 * A node holding three authors and five books.
 *
 * Author 1 has three books, author 2 has one, author 3 has none, and one book
 * names an author that does not exist. So inner and outer joins disagree, the
 * per-author count is not uniform, and ordering by that count is not the same
 * as ordering by key — each of which some test below depends on.
 */
async function library(): Promise<Session> {
  const server = await start(JOIN_TABLES);
  servers.push(server);
  const session = server.client().session();

  for (const author of [
    [uint(1), str("ada"), str("UK")],
    [uint(2), str("bo"), str("US")],
    [uint(3), str("cy"), str("UK")],
  ]) {
    await session.insert("authors", author);
  }
  for (const book of [
    [uint(10), uint(1), str("a-one"), int(2001)],
    [uint(11), uint(1), str("a-two"), int(1990)],
    [uint(12), uint(1), str("a-three"), int(2003)],
    [uint(13), uint(2), str("b-one"), int(2010)],
    [uint(14), uint(9), str("orphan"), int(2020)],
  ]) {
    await session.insert("books", book);
  }
  return session;
}

function authorsBooks(type: JoinType = "inner"): JoinQuery {
  const b = newJoin();
  const authors = b.add({ table: "authors" });
  b.add({
    table: "books",
    type,
    // authors.id equals books.author_id: the left side names its input, the
    // right side is an ordinal of the input it is written on.
    on: [{ earlier: at(authors, 0), own: 1 }],
  });
  return b.query();
}

function keyOf(group: Group): bigint {
  const first = group.key[0];
  assert.ok(first && first.kind === "uint", "a group key should be the author's u64 id");
  return first.value;
}

function countOfGroup(group: Group): bigint {
  const first = group.values[0];
  assert.ok(first && first.kind === "uint", "a count is a u64");
  return first.value;
}

test("an inner join keeps only matched rows", async () => {
  const session = await library();
  const rows = await session.join(authorsBooks("inner")).collect();

  assert.equal(rows.length, 4, "four books have an author");
  for (const row of rows) {
    assert.equal(row.length, 2, "one array per input");
    assert.ok(row[0] && row[1], "an inner join has no unmatched side");
  }
});

// A left join leaves the unmatched side `undefined` rather than an array of
// nulls, which is the distinction a flat row cannot make.
test("a left join keeps the unmatched left side", async () => {
  const session = await library();
  const rows = await session.join(authorsBooks("left")).collect();

  assert.equal(rows.length, 5, "four matches plus author 3");
  assert.equal(
    rows.filter((row) => row[1] === undefined).length,
    1,
    "exactly author 3 is unmatched",
  );
});

test("a join limit applies", async () => {
  const session = await library();
  const b = newJoin();
  const authors = b.add({ table: "authors" });
  b.add({ table: "books", on: [{ earlier: at(authors, 0), own: 1 }] });

  const rows = await session.join(b.query({ limit: 2 })).collect();
  assert.equal(rows.length, 2);
});

// The four join types must not all return the same thing, or the type is not
// reaching the server at all.
test("the four join types differ", async () => {
  const session = await library();
  const counts: Record<string, number> = {};
  for (const type of ["inner", "left", "right", "full"] as JoinType[]) {
    counts[type] = (await session.join(authorsBooks(type)).collect()).length;
  }
  assert.ok(counts["inner"]! < counts["left"]!, "a left join keeps author 3");
  assert.ok(counts["inner"]! < counts["right"]!, "a right join keeps the orphan");
  assert.ok(counts["full"]! > counts["left"]!, "a full join keeps most");
});

test("aggregate over one table", async () => {
  const session = await library();
  const groups = await session.aggregate({ table: "books" }, { aggregates: [count()] }).collect();

  assert.equal(groups.length, 1, "no group by means one group");
  assert.equal(countOfGroup(groups[0]!), 5n);
});

test("a grouped aggregate", async () => {
  const session = await library();
  const groups = await session
    .aggregate(
      { table: "books" },
      { groupBy: [key0(1)], aggregates: [count(), maxOf(key0(3))] },
    )
    .collect();

  assert.equal(groups.length, 3, "authors 1, 2 and the orphan's 9");
  const first = groups.find((g) => keyOf(g) === 1n);
  assert.ok(first, "author 1 should have a group");
  assert.equal(countOfGroup(first), 3n);
  assert.equal((first.values[1] as { value: bigint }).value, 2003n, "author 1's latest year");
});

test("a grouped join", async () => {
  const session = await library();
  const groups = await session
    .aggregateJoin(authorsBooks(), { groupBy: [at(0, 0)], aggregates: [count()] })
    .collect();

  assert.equal(groups.length, 2, "authors 1 and 2 have books, author 3 does not");
});

// The joined schema has to span every input for this to resolve. A group key
// on input 0 does not prove that.
test("a group key on the right side of the join resolves", async () => {
  const session = await library();
  const groups = await session
    .aggregateJoin(authorsBooks(), { groupBy: [at(1, 1)], aggregates: [count()] })
    .collect();

  assert.equal(groups.length, 2, "the same partition, named from the other side");
});

// Ordering, limiting and offsetting are over groups. Ascending by count is
// chosen because it disagrees with the kernel's own order — ascending by
// encoded key — so an ordering the client dropped would change the answer.
test("groups are ordered, limited and offset", async () => {
  const session = await library();
  const ordered = (extra: { limit?: number; offset?: number } = {}) =>
    session
      .aggregateJoin(authorsBooks(), {
        groupBy: [at(0, 0)],
        aggregates: [count()],
        // Fewest books first, then by key so the tie between equal counts is
        // not left to the hash order.
        sort: [
          { column: agg(0), direction: "asc" },
          { column: groupKey(0), direction: "asc" },
        ],
        ...extra,
      })
      .collect();

  const all = await ordered();
  assert.equal(all.length, 2);
  // Author 2 has one book, author 1 has three: fewest first.
  assert.equal(keyOf(all[0]!), 2n);
  assert.equal(keyOf(all[1]!), 1n);

  const first = await ordered({ limit: 1 });
  assert.equal(first.length, 1);
  assert.equal(keyOf(first[0]!), 2n, "the limit takes the first group");

  const second = await ordered({ offset: 1, limit: 1 });
  assert.equal(second.length, 1);
  assert.equal(keyOf(second[0]!), 1n, "the offset skips it");
});

test("having keeps groups", async () => {
  const session = await library();
  const groups = await session
    .aggregate(
      { table: "books" },
      {
        groupBy: [key0(1)],
        aggregates: [count()],
        having: groupGt(agg(0), uint(1)),
      },
    )
    .collect();

  assert.equal(groups.length, 1, "only author 1 has more than one book");
  assert.equal(keyOf(groups[0]!), 1n);
});

// A three-table chain, grouped.
//
// This asserted a refusal until the kernel grew grouping over a chain.
// Rewritten rather than deleted: a refusal test that outlives the refusal
// passes forever and protects nothing.
test("grouping a chain", async () => {
  const session = await library();
  const b = newJoin();
  const authors = b.add({ table: "authors" });
  const books = b.add({ table: "books", on: [{ earlier: at(authors, 0), own: 1 }] });
  // A third input joining back to the second, which is what makes it a chain.
  b.add({ table: "books", on: [{ earlier: at(books, 0), own: 0 }] });

  const groups = await session
    .aggregateJoin(b.query(), { groupBy: [at(0, 0)], aggregates: [count()] })
    .collect();

  assert.equal(groups.length, 2, "authors 1 and 2 have books");
});

test("explain join describes every input", async () => {
  const session = await library();
  const plan = await session.explainJoin(authorsBooks());

  assert.equal(plan.inputs.length, 2);
  assert.equal(plan.inputs[0]!.plan.table, "authors");
  assert.equal(plan.inputs[1]!.plan.table, "books");
});

// Forcing an algorithm must actually reach the planner. A test that runs each
// algorithm and checks the answers agree cannot tell: if the flag is dropped,
// all of them are the planner's choice and agree trivially. The plan is where
// the difference is visible.
test("a forced algorithm reaches the planner", async () => {
  const session = await library();
  const planFor = async (force: "nested-loop" | "hash-build-left") => {
    const b = newJoin();
    const authors = b.add({ table: "authors" });
    b.add({ table: "books", on: [{ earlier: at(authors, 0), own: 1 }], force });
    const plan = await session.explainJoin(b.query());
    return plan.inputs[1]!.algorithm;
  };

  const nested = await planFor("nested-loop");
  const hash = await planFor("hash-build-left");
  assert.notEqual(nested, hash, `forcing changed nothing: both plans said ${nested}`);
});

// Every algorithm must return the same rows. This is the oracle the Rust suite
// runs; here it guards against a client that mis-sends the force flag in a way
// that changes the answer rather than being ignored.
test("every join algorithm agrees", async () => {
  const session = await library();
  const answers: number[] = [];
  for (const force of [undefined, "hash-build-left", "hash-build-right", "nested-loop"] as const) {
    const b = newJoin();
    const authors = b.add({ table: "authors" });
    b.add({
      table: "books",
      on: [{ earlier: at(authors, 0), own: 1 }],
      ...(force ? { force } : {}),
    });
    answers.push((await session.join(b.query()).collect()).length);
  }
  assert.deepEqual(new Set(answers).size, 1, `algorithms disagreed: ${answers}`);
});
