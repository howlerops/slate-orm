import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  type Session,
  type Value,
  aggregateOver,
  denseRank,
  int,
  key0,
  lag,
  lead,
  over,
  rank,
  rowNumber,
  str,
  sumOf,
  uint,
  windowed,
} from "../src/index.js";

/**
 * Window functions from TypeScript, against a real node.
 *
 * A window is not an aggregate and the difference is the cardinality: a
 * grouped read returns one row per group, a window returns one value per
 * *input* row. So the two things to establish are that the values arrive, and
 * that they arrive in their own list — each row's `windowed`, beside `values`
 * and `computed`, rather than folded into either. A value in the wrong list is
 * a value the caller reads as a different thing, which is the failure the
 * three lists exist to prevent.
 *
 * The numbers are checked against `aggregate`, the same session talking to a
 * different RPC that folds rows away: two operators sharing only the
 * accumulator arithmetic, so agreeing is evidence rather than a restatement.
 */
const WINDOW_TABLES = `
[[tables]]
name = "readings"
id = 64
columns = [
  { name = "id",   type = "u64" },
  { name = "site", type = "str" },
  { name = "size", type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["readings"]
actions = ["everything"]
`;

const ID = 0;
const SITE = 1;
const SIZE = 2;

/**
 * Two sites of four rows each, and within a site the sizes go 10, 10, 20, 30.
 *
 * The ties are deliberate: they are what separates a correct `RANK` from a
 * `ROW_NUMBER` wearing its name, and a `RANGE` running total from a `ROWS` one.
 */
const READINGS: Array<[number, string, number]> = [
  [1, "north", 10],
  [2, "north", 10],
  [3, "north", 20],
  [4, "north", 30],
  [5, "south", 10],
  [6, "south", 10],
  [7, "south", 20],
  [8, "south", 30],
];

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

async function seeded(): Promise<Session> {
  const server = await start(WINDOW_TABLES);
  servers.push(server);
  const session = server.client().session();
  await session.insert(
    "readings",
    ...READINGS.map(([id, site, size]): Value[] => [
      uint(id),
      str(site),
      int(size),
    ]),
  );
  return session;
}

function asNumber(value: Value | undefined, what: string): number {
  assert.ok(value, `${what} is missing`);
  assert.ok(
    value.kind === "uint" || value.kind === "int",
    `${what} came back as ${value.kind}`,
  );
  return Number(value.value);
}

/** Drains a query, keyed by id, asserting the row shape on the way. */
async function windowValues(
  session: Session,
  query: Parameters<Session["query"]>[0],
): Promise<Map<number, Value[]>> {
  const out = new Map<number, Value[]>();
  const stream = session.query(query);
  for await (const row of stream.withComputed()) {
    // A readings row has three stored columns and no more, whatever the query
    // computed. A server folding the window into the columns fails here.
    assert.equal(row.values.length, 3, `unexpected row shape: ${row.values}`);
    assert.equal(row.computed.length, 0, "the query computes nothing");
    out.set(asNumber(row.values[ID], "id"), row.windowed);
  }
  return out;
}

test("a row number restarts in each partition and arrives in its own list", async () => {
  const session = await seeded();
  const got = await windowValues(session, {
    table: "readings",
    window: [
      over(rowNumber(), {
        partition: [key0(SITE)],
        order: [{ column: ID, direction: "asc" }],
      }),
    ],
  });

  const bySite = new Map<string, number[]>();
  for (const [id, site] of READINGS) {
    const values = got.get(id);
    assert.ok(values, `row ${id} is missing`);
    assert.equal(values.length, 1, `row ${id} carries ${values.length} windows`);
    const list = bySite.get(site) ?? [];
    list.push(asNumber(values[0], `row ${id}'s window`));
    bySite.set(site, list);
  }
  for (const [site, numbers] of bySite) {
    assert.deepEqual(
      [...numbers].sort((a, b) => a - b),
      [1, 2, 3, 4],
      `${site} numbered ${numbers}`,
    );
  }
});

test("rank leaves the gap that dense rank closes", async () => {
  const session = await seeded();
  const clause = { partition: [key0(SITE)], order: [{ column: SIZE }] };
  const got = await windowValues(session, {
    table: "readings",
    window: [over(rank(), clause), over(denseRank(), clause)],
  });

  // north is 10, 10, 20, 30: ranks 1, 1, 3, 4 and dense ranks 1, 1, 2, 3. The
  // two disagree exactly where the tie is, which is the point.
  const wantRank = new Map([
    [1, 1],
    [2, 1],
    [3, 3],
    [4, 4],
  ]);
  const wantDense = new Map([
    [1, 1],
    [2, 1],
    [3, 2],
    [4, 3],
  ]);
  for (const [id, want] of wantRank) {
    const values = got.get(id);
    assert.ok(values, `row ${id} is missing`);
    assert.equal(values.length, 2, `row ${id} carries ${values.length} windows`);
    assert.equal(asNumber(values[0], `row ${id} rank`), want);
    assert.equal(asNumber(values[1], `row ${id} dense rank`), wantDense.get(id));
  }
});

test("a partition aggregate agrees with the same group by", async () => {
  const session = await seeded();
  const got = await windowValues(session, {
    table: "readings",
    // No order: the whole partition, on every row.
    window: [over(aggregateOver(sumOf(key0(SIZE))), { partition: [key0(SITE)] })],
  });

  const bySite = new Map<string, number>();
  for await (const group of session.aggregate(
    { table: "readings" },
    { groupBy: [key0(SITE)], aggregates: [sumOf(key0(SIZE))] },
  )) {
    const key = group.key[0];
    assert.equal(key?.kind, "string", `a group key is ${key?.kind}`);
    bySite.set(String(key.value), asNumber(group.values[0], "a group's sum"));
  }
  assert.equal(bySite.size, 2);

  for (const [id, site] of READINGS) {
    const values = got.get(id);
    assert.ok(values);
    assert.equal(
      asNumber(values[0], `row ${id}'s window`),
      bySite.get(site),
      `row ${id}: the window and GROUP BY disagree`,
    );
  }
});

test("a running total gives peers the same value", async () => {
  const session = await seeded();
  const got = await windowValues(session, {
    table: "readings",
    window: [
      over(aggregateOver({ function: "count" }), {
        partition: [key0(SITE)],
        order: [{ column: SIZE }],
      }),
    ],
  });

  // north is 10, 10, 20, 30. Under RANGE the two tens are peers and both see
  // 2; under ROWS they would see 1 and 2, which is the mistake this catches.
  for (const [id, want] of [
    [1, 2],
    [2, 2],
    [3, 3],
    [4, 4],
  ] as const) {
    const values = got.get(id);
    assert.ok(values);
    assert.equal(asNumber(values[0], `row ${id}'s running count`), want);
  }
});

test("a sort can name a window", async () => {
  const session = await seeded();
  const numbers: number[] = [];
  const stream = session.query({
    table: "readings",
    window: [
      over(rowNumber(), {
        partition: [key0(SITE)],
        order: [{ column: ID, direction: "asc" }],
      }),
    ],
    sort: [
      { column: 0, ref: windowed(0), direction: "desc" },
      { column: ID, direction: "asc" },
    ],
  });
  for await (const row of stream.withComputed()) {
    numbers.push(asNumber(row.windowed[0], "the window's value"));
  }

  assert.equal(numbers.length, READINGS.length);
  for (let i = 1; i < numbers.length; i += 1) {
    assert.ok(
      (numbers[i - 1] ?? 0) >= (numbers[i] ?? 0),
      `not descending: ${numbers}`,
    );
  }
  // Not vacuous: the values actually differ, so "descending" is a claim.
  assert.notEqual(numbers[0], numbers[numbers.length - 1]);
});

test("a filter naming a window is refused", async () => {
  const session = await seeded();
  const stream = session.query({
    table: "readings",
    window: [over(rowNumber(), { order: [{ column: ID }] })],
    filter: {
      wire: {
        compare: {
          column: { windowed: 0 },
          op: "CMP_OP_EQ",
          value: { uint64Value: "1" },
        },
      },
    },
  });
  // Drained rather than awaited: a gRPC stream does not deliver a rejected
  // request until the first message is read, so the refusal arrives here and
  // not from `query` itself.
  await assert.rejects(
    (async () => {
      for await (const _ of stream) {
        // Nothing: the point is that the loop throws.
      }
    })(),
    /after the filter/,
  );
});

test("an unordered rank is refused", async () => {
  const session = await seeded();
  const stream = session.query({ table: "readings", window: [rank()] });
  await assert.rejects(
    (async () => {
      for await (const _ of stream) {
        // Nothing.
      }
    })(),
  );
});

test("lag at offset zero is refused", async () => {
  const session = await seeded();
  const stream = session.query({
    table: "readings",
    window: [over(lag(key0(SIZE), 0), { order: [{ column: ID }] })],
  });
  await assert.rejects(
    (async () => {
      for await (const _ of stream) {
        // Nothing.
      }
    })(),
  );
});

test("lag and lead step through the partition", async () => {
  const session = await seeded();
  const clause = {
    partition: [key0(SITE)],
    order: [{ column: ID, direction: "asc" as const }],
  };
  const got = await windowValues(session, {
    table: "readings",
    window: [over(lag(key0(SIZE), 1), clause), over(lead(key0(SIZE), 1), clause)],
  });

  // Written because a mutation was caught for the wrong reason: sending the
  // offset one too large was noticed only by "lag at offset zero is refused",
  // which turns into an *accepted* request under that mutation. That proves
  // the offset reaches the server and says nothing about the value coming
  // back, and every other test here uses a ranking function or an aggregate,
  // neither of which carries a column at all.
  //
  // north is ids 1..4 at sizes 10, 10, 20, 30; south is 5..8 with the same. Row
  // 5 is the first of its partition, so its lag is null rather than row 4's 30
  // — which is the assertion that the partition is honoured and not only the
  // order.
  const want: Array<[number, number | null, number | null]> = [
    [1, null, 10],
    [2, 10, 20],
    [3, 10, 30],
    [4, 20, null],
    [5, null, 10],
    [8, 20, null],
  ];
  for (const [id, wantLag, wantLead] of want) {
    const values = got.get(id);
    assert.ok(values, `row ${id} is missing`);
    assert.equal(values.length, 2, `row ${id} carries ${values.length} windows`);
    const steps: Array<[number, number | null, string]> = [
      [0, wantLag, "lag"],
      [1, wantLead, "lead"],
    ];
    for (const [at, wanted, label] of steps) {
      const got: Value | undefined = values[at];
      if (wanted === null) {
        assert.equal(got?.kind, "null", `row ${id} ${label} is ${got?.kind}`);
      } else {
        assert.equal(asNumber(got, `row ${id} ${label}`), wanted);
      }
    }
  }
});
