/**
 * `or` and an ORed `having`, through a real node.
 *
 * # Why this file exists
 *
 * `ledger/2026-09-25-the-disjunction-the-kernel-always-had.md` taught the SQL
 * front end to write `OR`, and two of its caveats said no client could send
 * one and that the gRPC `Query` had no equivalent. Both were wrong:
 * `Expr.disjunction` is field 9 of the wire and every client has had a builder
 * for it. `ledger/2026-09-25-the-clients-could-always-send-a-disjunction.md`
 * withdrew them.
 *
 * This is the TypeScript third of the proof, beside the Python and Go ones.
 * It exists because a claim of that shape got written twice by somebody
 * reading the front end and generalising, and the only thing that stops a
 * third is something that runs.
 *
 * The oracle is the two arms: a disjunction returns their union and a
 * conjunction their intersection. Both are asserted, and the arms are checked
 * to differ first — a fixture where one arm contained the other would make the
 * two connectives agree and the test would pass with `or` implemented as
 * `and`.
 */
import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  agg,
  and,
  count,
  eq,
  groupGt,
  groupLt,
  gt,
  int,
  key0,
  or,
  str,
  uint,
  type Expr,
  type Session,
  type Value,
} from "../src/index.js";

// Sizes chosen so `size > 20` and `kind = "note"` overlap on exactly one row
// and each admits one the other does not.
const SEEDED: ReadonlyArray<readonly [number, string, number]> = [
  [1, "note", 10], // neither
  [2, "note", 30], // both
  [3, "memo", 40], // size only
  [4, "note", 5], // kind only
  [5, "memo", 15], // neither
  [6, "sheet", 50], // size only
];

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

async function seeded(): Promise<Session> {
  const server = await start();
  servers.push(server);
  const session = server.client().session();
  await session.insert(
    "docs",
    ...SEEDED.map(([id, kind, size]) => [uint(id), str(kind), int(size)] as Value[]),
  );
  return session;
}

/** The ids a filter admits, sorted. */
async function matching(session: Session, filter: Expr): Promise<bigint[]> {
  const rows = await session
    .query({ table: "docs", filter, sort: [{ column: 0, direction: "asc" }] })
    .collect();
  return rows.map((row) => {
    const id = row[0]!;
    assert.equal(id.kind, "uint", `the first column is ${id.kind}`);
    return (id as { value: bigint }).value;
  });
}

const union = (a: bigint[], b: bigint[]): bigint[] => [...new Set([...a, ...b])].sort(byValue);
const intersection = (a: bigint[], b: bigint[]): bigint[] => b.filter((v) => a.includes(v)).sort(byValue);
const byValue = (x: bigint, y: bigint): number => (x < y ? -1 : x > y ? 1 : 0);

const BIG = (): Expr => gt(2, int(20));
const NAMED = (): Expr => eq(1, str("note"));

test("a client sends a disjunction in a filter", async () => {
  const session = await seeded();

  const big = await matching(session, BIG());
  const named = await matching(session, NAMED());

  // The arms must discriminate, or a server that ANDed them would satisfy
  // every assertion below.
  assert.ok(big.length > 0 && named.length > 0, `an empty arm proves nothing: ${big} / ${named}`);
  assert.notDeepEqual(big, named, "the arms admit the same rows, so union and intersection agree");

  assert.deepEqual(await matching(session, or(BIG(), NAMED())), union(big, named));

  // And the conjunction really is smaller, which is what makes the line above
  // a test of the connective rather than of the fixture.
  const conjoined = await matching(session, and(BIG(), NAMED()));
  assert.deepEqual(conjoined, intersection(big, named));
  assert.notDeepEqual(conjoined, union(big, named));
});

test("an empty or is false and an empty and is true", async () => {
  // The documented identities, which are what keep a filter built up in a loop
  // from flipping meaning when the loop adds nothing. A server that read an
  // empty disjunction as `true` would return the whole table, which is the
  // dangerous direction.
  const session = await seeded();
  assert.deepEqual(await matching(session, or()), []);
  assert.equal((await matching(session, and())).length, SEEDED.length);
});

/** The `kind` of every group a HAVING keeps, sorted. */
async function groupKinds(session: Session, having?: Expr): Promise<string[]> {
  // Spread rather than `having`: `exactOptionalPropertyTypes` makes an
  // explicit `undefined` a different thing from an absent key, and the
  // ungrouped baseline needs the key absent.
  const groups = await session
    .aggregate(
      { table: "docs" },
      { groupBy: [key0(1)], aggregates: [count()], ...(having ? { having } : {}) },
    )
    .collect();
  return groups
    .map((group) => {
      const key = group.key[0]!;
      assert.equal(key.kind, "string", `the key is ${key.kind}`);
      return (key as { value: string }).value;
    })
    .sort();
}

test("a client sends a disjunction in a having", async () => {
  // The counts are note=3, memo=2, sheet=1. `> 2` keeps note and `< 2` keeps
  // sheet, so the union leaves memo out — without which this would pass
  // against a server that ignored `having` entirely.
  const session = await seeded();

  assert.deepEqual(await groupKinds(session), ["memo", "note", "sheet"]);
  assert.deepEqual(await groupKinds(session, groupGt(agg(0), uint(2))), ["note"]);
  assert.deepEqual(await groupKinds(session, groupLt(agg(0), uint(2))), ["sheet"]);
  assert.deepEqual(
    await groupKinds(session, or(groupGt(agg(0), uint(2)), groupLt(agg(0), uint(2)))),
    ["note", "sheet"],
  );
});
