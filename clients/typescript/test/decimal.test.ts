import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  SlateError,
  type Session,
  type Value,
  int,
  str,
  uint,
  units,
  unitsToString,
} from "../src/index.js";

/**
 * A decimal column, and the conditional update it exists to protect.
 *
 * Both were built in the kernel and reachable only from Rust until the
 * protocol grew `Value.decimal_value` and `UpdateRequest.expected`.
 *
 * A decimal here is a count of the column's smallest unit and nothing else.
 * `units(1250n)` in a column declared scale 2 is 12.50, and the identical
 * value in a scale-0 column is 1250 — nothing on the wire says which, because
 * the protocol publishes no schema. So these tests are written to fail if a
 * decimal ever arrives as an `int`: that is the confusion that loses two
 * decimal places with no error anywhere.
 */
const DECIMAL_TABLES = `
[[tables]]
name = "prices"
id = 51
columns = [
  { name = "id",     type = "u64" },
  { name = "label",  type = "str" },
  { name = "amount", type = "decimal", scale = 2 },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["prices"]
actions = ["everything"]
`;

/**
 * The scale `prices.amount` is declared with, restated here because the
 * protocol publishes no schema and a client's idea of a scale is a local
 * declaration. It is exactly the hole that makes `unitsToString` take the
 * scale as an argument.
 */
const SCALE = 2;

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

function priceRow(id: number, label: string, amount: bigint | number): Value[] {
  return [uint(id), str(label), units(amount)];
}

/** A node holding three prices: tea 2.50, coffee 12.50, free 0. */
async function priced(): Promise<Session> {
  const server = await start(DECIMAL_TABLES);
  servers.push(server);
  const session = server.client().session();
  await session.insert(
    "prices",
    priceRow(1, "tea", 250),
    priceRow(2, "coffee", 1250),
    priceRow(3, "free", 0),
  );
  return session;
}

async function amountOf(session: Session, id: number): Promise<bigint> {
  const row = await session.get("prices", [uint(id)]);
  assert.ok(row, `row ${id} is not there`);
  const amount = row[2];
  // Not a soft check: reading a decimal as an `int` is the whole failure this
  // file is about, and every assertion after it would compare the wrong kind.
  assert.equal(amount?.kind, "units", `a decimal came back as ${amount?.kind}`);
  return (amount as { kind: "units"; value: bigint }).value;
}

test("a decimal round trips through the server", async () => {
  const session = await priced();
  assert.equal(await amountOf(session, 2), 1250n);
  assert.equal(unitsToString(await amountOf(session, 2), SCALE), "12.50");
});

test("a negative decimal survives", async () => {
  // A refund is the value most likely to be mangled by an encoding that
  // assumed a price is positive.
  const session = await priced();
  await session.insert("prices", priceRow(9, "refund", -75));
  assert.equal(await amountOf(session, 9), -75n);
});

test("an integer in a decimal column is refused", async () => {
  // `int`, not `units` — the mistake a caller who did not notice the new
  // builder would make. The kernel orders values type first, so storing it
  // would put a value in the column that does not sort with its neighbours.
  const session = await priced();
  await assert.rejects(
    () => session.insert("prices", [uint(8), str("wrong"), int(100)]),
    SlateError,
  );
});

test("units render against a scale", () => {
  // The table the Python and Go clients render identically; a difference
  // between the three is a bug in one of them rather than a dialect.
  const cases: [bigint, number, string][] = [
    [1250n, 2, "12.50"],
    [250n, 2, "2.50"],
    [0n, 2, "0.00"],
    [-75n, 2, "-0.75"],
    [5n, 3, "0.005"],
    [1250n, 0, "1250"],
    // The i64 extreme, whose magnitude does not fit in an i64. A `bigint` has
    // no trouble with it; the case is here so the three clients agree at the
    // end of the range rather than only in the middle.
    [-9223372036854775808n, 2, "-92233720368547758.08"],
    // A negative scale is a caller's mistake in a render path, and a render
    // path is the worst place to throw. It reads as 0.
    [1250n, -1, "1250"],
  ];
  for (const [value, scale, expected] of cases) {
    assert.equal(unitsToString(value, scale), expected, `${value} at ${scale}`);
  }
});

test("a decimal larger than 2^53 keeps every digit", () => {
  // The reason `units` takes a bigint. As a `number`, this value rounds, and a
  // currency total in the smallest unit is exactly where 2^53 is reached
  // first.
  const exact = 9007199254740993n;
  assert.equal(unitsToString(exact, 2), "90071992547409.93");
  assert.notEqual(BigInt(Number(exact)), exact, "the premise of this test");
});

test("an update naming the row it read is applied", async () => {
  const session = await priced();
  const result = await session.updateIfUnchanged("prices", {
    row: priceRow(2, "coffee", 1400),
    was: priceRow(2, "coffee", 1250),
  });
  assert.equal(result.affected, 1n);
  assert.equal(await amountOf(session, 2), 1400n);
});

test("an update naming a row that moved is refused", async () => {
  const session = await priced();
  // Somebody else's write lands between this caller's read and its write.
  await session.update("prices", priceRow(2, "coffee", 1300));

  await assert.rejects(
    () =>
      session.updateIfUnchanged("prices", {
        row: priceRow(2, "coffee", 1400),
        was: priceRow(2, "coffee", 1250),
      }),
    SlateError,
  );
  // And the refusal is total: the write it guarded did not land.
  assert.equal(await amountOf(session, 2), 1300n);
});

test("a stale first row refuses the rest", async () => {
  // The order a loop which does not stop at the first refusal gets wrong: it
  // would report the *last* row's outcome, so a stale first row and a current
  // second row would answer with a success. That is a lost update reported as
  // a successful conditional update.
  const session = await priced();
  await session.update("prices", priceRow(1, "tea", 251));

  await assert.rejects(
    () =>
      session.updateIfUnchanged(
        "prices",
        { row: priceRow(1, "tea", 300), was: priceRow(1, "tea", 250) },
        { row: priceRow(2, "coffee", 1400), was: priceRow(2, "coffee", 1250) },
      ),
    SlateError,
  );
  assert.equal(await amountOf(session, 2), 1250n);
});

test("a conditional update inside a transaction", async () => {
  // The session path is different code from the autocommit one, and a feature
  // wired into one and not the other is this repository's recurring shape.
  const session = await priced();
  const tx = await session.begin();
  await tx.updateIfUnchanged("prices", {
    row: priceRow(2, "coffee", 1500),
    was: priceRow(2, "coffee", 1250),
  });
  await tx.commit();
  assert.equal(await amountOf(session, 2), 1500n);
});

test("a stale conditional update inside a transaction is refused", async () => {
  // Written because a mutation survived without it. The happy-path
  // transaction test above passes whether or not the transaction's
  // `updateIfUnchanged` actually sends its expected rows — an unconditional
  // update of an unchanged row produces the same outcome. Only a *stale* row
  // tells the two apart, and the transaction path is separate code from the
  // autocommit one on both sides of the wire.
  const session = await priced();
  await session.update("prices", priceRow(2, "coffee", 1300));

  const tx = await session.begin();
  await assert.rejects(
    () =>
      tx.updateIfUnchanged("prices", {
        row: priceRow(2, "coffee", 1400),
        was: priceRow(2, "coffee", 1250),
      }),
    SlateError,
  );
  await tx.rollback();
  assert.equal(await amountOf(session, 2), 1300n);
});

test("no row updates is an ordinary update of nothing", async () => {
  // The degenerate case, worth pinning because `expected` is `repeated` on the
  // wire and an empty one means "no condition": a call with no pairs must not
  // become an unconditional update of every row.
  const session = await priced();
  const result = await session.updateIfUnchanged("prices");
  assert.equal(result.affected, 0n);
  assert.equal(await amountOf(session, 2), 1250n);
});
