/**
 * `not(isIn(...))` — the composition every client has and none of them tested.
 *
 * # Why this file exists
 *
 * `ledger/2026-09-18-not-in-was-refused-on-a-claim-that-does-not-hold.md`
 * taught the SQL front end to write `NOT IN` and then said of the clients:
 *
 *     **No client can express it.** `notIn` is a front-end operator; the wire
 *     carries `Expr`, and the three SDKs build `Expr::In` with no negation
 *     helper. A client that wants this builds the `Not` itself, which is
 *     possible and undocumented.
 *
 * Its two sentences contradict each other and the second is the true one:
 * `not` has been exported the whole time, so `not(isIn(...))` is the
 * expression. `ledger/2026-09-29-three-could-be-anothers-counted.md` withdrew
 * the first half and found what the caveat had not — **no test in any of the
 * three clients used the negation helper at all.** Three doors and nobody
 * walking through any of them, which is how the wrong claim survived: somebody
 * read the front end and generalised, and nothing ran to say otherwise.
 *
 * This is the TypeScript third, beside the Python and Go ones.
 *
 * # The oracle
 *
 * `NOT IN` is the complement of `IN` **over the rows that exist**: the two must
 * be disjoint and must together cover every seeded row. That is stronger than
 * comparing `NOT IN` to a hand-written list, because it fails if either arm
 * drifts, and it is the property a reader relies on.
 *
 * It is not SQL's `NOT IN`. SQL's is three-valued, so a null in the list makes
 * the answer unknown and admits nothing; `Expr::In` has the same rule, which
 * `docs/correctness.md` works through. This fixture has no nulls, which is why
 * the complement holds — a nullable column would need its own case and does
 * not have one.
 */
import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  eq,
  int,
  isIn,
  not,
  str,
  uint,
  type Expr,
  type Session,
  type Value,
} from "../src/index.js";

// The same six rows the disjunction test uses, for the same reason: three
// kinds, so a list can name a proper non-empty subset of them. A list naming
// every kind makes the complement empty and one naming none makes it
// everything, and either passes a `not` that returned its argument.
const SEEDED: ReadonlyArray<readonly [number, string, number]> = [
  [1, "note", 10],
  [2, "note", 30],
  [3, "memo", 40],
  [4, "note", 5],
  [5, "memo", 15],
  [6, "sheet", 50],
];

const EXCLUDED = (): Expr => isIn(1, [str("memo"), str("sheet")]);

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

const sorted = (v: bigint[]): bigint[] => [...v].sort((x, y) => (x < y ? -1 : x > y ? 1 : 0));

test("not in is the complement of in", async () => {
  const session = await seeded();

  const inside = await matching(session, EXCLUDED());
  const outside = await matching(session, not(EXCLUDED()));

  // Both arms must be non-empty, or the complement assertion below is
  // satisfied by a server that answered nothing to one of them.
  assert.ok(
    inside.length > 0 && outside.length > 0,
    `an empty arm proves nothing: in=${inside} notIn=${outside}`,
  );

  const shared = inside.filter((v) => outside.includes(v));
  assert.deepEqual(shared, [], `IN and NOT IN both admit ${shared}, so the negation did nothing`);

  // Exhaustive over the fixture, read off SEEDED rather than written out, so a
  // row added there is covered rather than silently outside the claim.
  const every = sorted(SEEDED.map(([id]) => BigInt(id)));
  assert.deepEqual(sorted([...inside, ...outside]), every);
});

test("negation is not ignored on a single comparison", async () => {
  // The narrower half, and the one that would catch a `not` that unwrapped to
  // its argument: the composition above is disjoint and exhaustive whether or
  // not the negation is applied *if* the server happened to answer the
  // complement for `isIn` too. One comparison and its negation is not.
  const session = await seeded();

  const notes = await matching(session, eq(1, str("note")));
  const others = await matching(session, not(eq(1, str("note"))));

  assert.ok(
    notes.length > 0 && others.length > 0,
    `an empty arm proves nothing: eq=${notes} not=${others}`,
  );
  assert.notDeepEqual(notes, others, "a comparison and its negation admit the same rows");
  assert.deepEqual(
    notes.filter((v) => others.includes(v)),
    [],
  );
});
