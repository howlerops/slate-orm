import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  add,
  at,
  compare,
  computed0,
  concat,
  count,
  dateTrunc,
  dayOfMonth,
  dayOfWeek,
  div,
  extract,
  int,
  joinComputed,
  lit,
  month,
  monthStart,
  mul,
  newJoin,
  ref,
  round,
  col,
  str,
  sub,
  uint,
  upper,
  year,
  yearStart,
  computedAt,
  type Session,
  type Value,
} from "../src/index.js";

// Computed values, and the two kinds of reference that name them.
//
// TypeScript had no `Scalar` surface at all until now: it could not build a
// computed value, could not name one, and dropped the ones a row carried.
// Python could do all three, which is why the three-SDK conformance runner
// could not compare any of the date-and-time work.
//
// The oracle is JavaScript's own `Date`, which knows the Gregorian calendar
// independently of the kernel's transcribed `civil_from_days`.

const EVENT_TABLES = `
[[tables]]
name = "events"
id = 20
columns = [
  { name = "id",   type = "u64" },
  { name = "at",   type = "i64" },
  { name = "kind", type = "str" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["events"]
actions = ["everything"]
`;

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

/**
 * Instants in different years, months and weekdays — including a leap day and
 * one before the epoch, which are the two cases a naive `days / 365` or an
 * unsigned division gets wrong.
 */
const INSTANTS = [
  Date.UTC(2024, 1, 29, 13, 45, 0),
  Date.UTC(1999, 11, 31, 23, 59, 59),
  Date.UTC(2001, 0, 1, 0, 0, 0),
  Date.UTC(1969, 6, 20, 20, 17, 0),
  Date.UTC(2010, 5, 15, 6, 0, 0),
].map((ms) => Math.floor(ms / 1000));

const EVENT_AT = 1;

async function events(): Promise<Session> {
  const server = await start(EVENT_TABLES);
  servers.push(server);
  const session = server.client().session();
  for (const [id, at] of INSTANTS.entries()) {
    await session.insert("events", [uint(id), int(at), str("e")]);
  }
  return session;
}

/** Each row's id mapped to its computed values. */
async function computedByID(
  session: Session,
  query: Parameters<Session["query"]>[0],
): Promise<Map<bigint, Value[]>> {
  const out = new Map<bigint, Value[]>();
  for await (const row of session.query(query).withComputed()) {
    const id = row.values[0];
    assert.ok(id && id.kind === "uint", "the id is a u64");
    out.set(id.value, row.computed);
  }
  return out;
}

function asInt(value: Value | undefined, what: string): bigint {
  assert.ok(value && value.kind === "int", `${what} should be an i64, got ${value?.kind}`);
  return value.value;
}

test("the calendar functions agree with JavaScript's own Date", async () => {
  const session = await events();
  const got = await computedByID(session, {
    table: "events",
    compute: [
      year(col(EVENT_AT)),
      month(col(EVENT_AT)),
      dayOfMonth(col(EVENT_AT)),
      dayOfWeek(col(EVENT_AT)),
      extract("hour", col(EVENT_AT)),
    ],
  });
  assert.equal(got.size, INSTANTS.length);

  for (const [id, seconds] of INSTANTS.entries()) {
    const when = new Date(seconds * 1000);
    const values = got.get(BigInt(id));
    assert.ok(values, `row ${id} came back`);
    const want = [
      BigInt(when.getUTCFullYear()),
      BigInt(when.getUTCMonth() + 1),
      BigInt(when.getUTCDate()),
      BigInt(when.getUTCDay()), // 0 is Sunday, which is what the kernel uses.
      BigInt(when.getUTCHours()),
    ];
    for (const [n, expected] of want.entries()) {
      assert.equal(
        asInt(values[n], `computed value ${n} of ${when.toISOString()}`),
        expected,
        `computed value ${n} of ${when.toISOString()}`,
      );
    }
  }
});

test("date_trunc to the day, and round returning an integer", async () => {
  const session = await events();
  const got = await computedByID(session, {
    table: "events",
    compute: [
      dateTrunc("day", col(EVENT_AT)),
      // Dividing by a float is what makes this exercise `round`: two integers
      // divide exactly, so rounding that would be the identity.
      round(div(col(EVENT_AT), lit({ kind: "float", value: 86400 }))),
    ],
  });

  for (const [id, seconds] of INSTANTS.entries()) {
    const values = got.get(BigInt(id));
    assert.ok(values);
    const day = 86400;
    // Floor toward negative infinity, which is what truncating to a day means
    // for an instant before the epoch — and the case a `/` in C would get
    // wrong.
    const wantTrunc = BigInt(Math.floor(seconds / day) * day);
    assert.equal(asInt(values[0], "date_trunc"), wantTrunc);

    const exact = seconds / day;
    const wantRound = BigInt(exact < 0 ? Math.trunc(exact - 0.5) : Math.trunc(exact + 0.5));
    assert.equal(asInt(values[1], "round"), wantRound);
  }
});

test("a computed value reads an earlier one, and a filter names it", async () => {
  const session = await events();
  const got = await computedByID(session, {
    table: "events",
    compute: [year(col(EVENT_AT)), sub(ref(computed0(0)), lit(int(2000)))],
    filter: compare(computed0(0), "ge", int(2000)),
  });
  assert.ok(got.size > 0, "the fixture has years at or after 2000");
  for (const [id, values] of got) {
    const y = asInt(values[0], "the year");
    assert.ok(y >= 2000n, `row ${id} has year ${y}, which the filter should have removed`);
    assert.equal(asInt(values[1], "years since 2000"), y - 2000n);
  }
});

test("a computed value may not read a later one", async () => {
  const session = await events();
  await assert.rejects(
    async () => {
      // Reads computed value 1, which does not exist when it runs.
      const stream = session.query({
        table: "events",
        compute: [ref(computed0(1)), year(col(EVENT_AT))],
      });
      await stream.collect();
    },
    (error: Error) => {
      assert.match(error.message, /available/);
      return true;
    },
  );
});

// --- a join's own computed value ------------------------------------------

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

async function library(): Promise<Session> {
  const server = await start(JOIN_TABLES);
  servers.push(server);
  const session = server.client().session();
  for (const author of [
    [uint(1), str("ada"), str("UK")],
    [uint(2), str("bo"), str("US")],
  ]) {
    await session.insert("authors", author);
  }
  for (const book of [
    [uint(10), uint(1), str("a-one"), int(2001)],
    [uint(11), uint(1), str("a-two"), int(1990)],
    [uint(12), uint(1), str("a-three"), int(2003)],
    [uint(13), uint(2), str("b-one"), int(2010)],
  ]) {
    await session.insert("books", book);
  }
  return session;
}

test("a join's computed value reads both sides", async () => {
  const session = await library();
  const b = newJoin();
  const authors = b.add({ table: "authors" });
  const books = b.add({
    table: "books",
    on: [{ earlier: at(authors, 0), own: 1 }],
  });
  const join = {
    ...b.query(),
    // `authors.name || "/" || books.title`, which needs both inputs — the
    // thing no input's own `compute` can express.
    compute: [concat(ref(at(authors, 1)), lit(str("/")), ref(at(books, 2)))],
  };

  let seen = 0;
  for await (const row of session.join(join).withComputed()) {
    assert.equal(row.computed.length, 1);
    const [left, right] = row.inputs;
    assert.ok(left && right, "an inner join pairs both");
    const name = left[1];
    const title = right[2];
    assert.ok(name?.kind === "string" && title?.kind === "string");
    const computed = row.computed[0];
    assert.ok(computed?.kind === "string");
    assert.equal(computed.value, `${name.value}/${title.value}`);
    seen += 1;
  }
  assert.equal(seen, 4);
});

test("a grouped join groups by the join's computed value", async () => {
  const session = await library();
  const b = newJoin();
  const authors = b.add({ table: "authors" });
  const books = b.add({
    table: "books",
    on: [{ earlier: at(authors, 0), own: 1 }],
  });
  const join = {
    ...b.query(),
    // The decade a book came out in, which neither table stores.
    compute: [mul(div(ref(at(books, 3)), lit(int(10))), lit(int(10)))],
  };

  const groups = await session
    .aggregateJoin(join, { groupBy: [joinComputed(0)], aggregates: [count()] })
    .collect();

  const want = new Map<bigint, bigint>([
    [2000n, 2n],
    [1990n, 1n],
    [2010n, 1n],
  ]);
  assert.equal(groups.length, want.size);
  for (const group of groups) {
    const decade = asInt(group.key[0], "the decade");
    const n = group.values[0];
    assert.ok(n?.kind === "uint");
    assert.equal(n.value, want.get(decade), `decade ${decade}`);
  }
});

test("both kinds of computed value come back on a join", async () => {
  // Sending is not reading back. `withComputed` carried the *join's* computed
  // values and threw each input's away: `rowFromWire` takes a row's `values`
  // and drops its `computed`, which is right for a stored column and meant an
  // input-level computed value arrived and vanished with no error anywhere.
  //
  // So this asks for both at once and checks each against the stored columns
  // it was computed from — which is what distinguishes a value that survived
  // from one that happened to be the right shape.
  const session = await library();
  const b = newJoin();
  const authors = b.add({
    table: "authors",
    // Reads `authors` and nothing else.
    compute: [upper(col(1))],
  });
  // An input's own compute still has to *name* that input: a bare `col(3)`
  // means input 0, and on input 1 the server refuses it — "computed value 0
  // names input 0, and is evaluated over input 1" — rather than quietly
  // reading `authors.country`. The handle is not available inside the object
  // that needs it, so it is written down and then checked.
  const booksAt = 1;
  const books = b.add({
    table: "books",
    on: [{ earlier: at(authors, 0), own: 1 }],
    // Reads `books` and nothing else.
    compute: [mul(div(ref(at(booksAt, 3)), lit(int(10))), lit(int(10)))],
  });
  assert.equal(books, booksAt, "the second input's compute names the wrong one");
  const join = {
    ...b.query(),
    // Reads both, which is what no input's own compute can do.
    compute: [concat(ref(at(authors, 1)), lit(str("/")), ref(at(books, 2)))],
  };

  let seen = 0;
  for await (const row of session.join(join).withComputed()) {
    const [left, right] = row.inputs;
    const [leftOwn, rightOwn] = row.inputComputed;
    assert.ok(left && right, "an inner join pairs both");
    assert.ok(leftOwn && rightOwn, "and both inputs computed something");

    const name = left[1];
    const title = right[2];
    const year = right[3];
    assert.ok(name?.kind === "string" && title?.kind === "string");
    assert.ok(year?.kind === "int");

    const joined = row.computed[0];
    assert.ok(joined?.kind === "string");
    assert.equal(joined.value, `${name.value}/${title.value}`);

    const upperName = leftOwn[0];
    assert.ok(upperName?.kind === "string");
    assert.equal(upperName.value, name.value.toUpperCase());

    const decade = rightOwn[0];
    assert.ok(decade?.kind === "int");
    assert.equal(decade.value, (year.value / 10n) * 10n);
    seen += 1;
  }
  assert.equal(seen, 4);
});

test("an input's own computed value is not nameable across a join", async () => {
  const session = await library();
  const b = newJoin();
  const authors = b.add({ table: "authors", compute: [upper(col(1))] });
  b.add({ table: "books", on: [{ earlier: at(authors, 0), own: 1 }] });

  await assert.rejects(
    async () => {
      const stream = session.aggregateJoin(b.query(), {
        // Names input 0's own computed value, which has no slot in a joined row.
        groupBy: [computedAt(0, 0)],
        aggregates: [count()],
      });
      await stream.collect();
    },
    (error: Error) => {
      assert.match(error.message, /joined_computed/);
      return true;
    },
  );
});

// A timezone shift is arithmetic, which is the whole reason no zone field
// exists: `hour(t, '-05:00')` is `hour(t - 18000)`.
test("a fixed offset is an addition, and rotates the hours", async () => {
  const session = await events();
  const got = await computedByID(session, {
    table: "events",
    compute: [
      extract("hour", col(EVENT_AT)),
      extract("hour", add(col(EVENT_AT), lit(int(-5 * 3600)))),
    ],
  });
  for (const [id, seconds] of INSTANTS.entries()) {
    const values = got.get(BigInt(id));
    assert.ok(values);
    const utc = asInt(values[0], "the UTC hour");
    const shifted = asInt(values[1], "the shifted hour");
    assert.equal(shifted, BigInt(new Date((seconds - 5 * 3600) * 1000).getUTCHours()));
    // And the shift is five hours modulo the day, which is the property that
    // would survive a different fixture.
    assert.equal((utc - shifted + 24n) % 24n, 5n);
  }
});

// Sorting **by** a computed value, which `SortKey.ref` is for.
//
// A mutation that ignored `ref` and sorted by `column` instead survived the
// first pass of this file: every other test named a computed value in a
// filter, a group key or a projection, and none in an ordering. `column`
// defaults to 0, so the mutant sorted by `id` — which on a fixture whose ids
// happen to follow the intended order would still have passed. The instants
// below deliberately do not: by year they are 1969, 1999, 2001, 2010, 2024,
// which is a different permutation from their ids.
test("a sort key may name a computed value", async () => {
  const session = await events();
  const years: bigint[] = [];
  for await (const row of session
    .query({
      table: "events",
      compute: [year(col(EVENT_AT))],
      sort: [{ column: 0, ref: computed0(0), direction: "asc" }],
    })
    .withComputed()) {
    years.push(asInt(row.computed[0], "the year"));
  }

  const sorted = [...years].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  assert.deepEqual(years, sorted, "the rows should arrive in year order");
  // And that order is genuinely not the id order, so sorting by column 0
  // instead would be a different answer.
  const byID = INSTANTS.map((s) => BigInt(new Date(s * 1000).getUTCFullYear()));
  assert.notDeepEqual(years, byID, "the fixture must not already be in year order");
});

// Truncating to a month and a year, which `dateTrunc` cannot do: `TimeUnit`
// promises a fixed number of seconds and a month has none. `Date.UTC` is the
// oracle.
test("calendar truncation agrees with JavaScript's own Date", async () => {
  const session = await events();
  const got = await computedByID(session, {
    table: "events",
    compute: [monthStart(col(EVENT_AT)), yearStart(col(EVENT_AT))],
  });
  for (const [id, seconds] of INSTANTS.entries()) {
    const values = got.get(BigInt(id));
    assert.ok(values);
    const when = new Date(seconds * 1000);
    const wantMonth = BigInt(
      Math.floor(Date.UTC(when.getUTCFullYear(), when.getUTCMonth(), 1) / 1000),
    );
    const wantYear = BigInt(Math.floor(Date.UTC(when.getUTCFullYear(), 0, 1) / 1000));
    assert.equal(asInt(values[0], "month start"), wantMonth, when.toISOString());
    assert.equal(asInt(values[1], "year start"), wantYear, when.toISOString());
    // Floors rather than rounds, which the 1969 instant is here to check.
    assert.ok(asInt(values[0], "month start") <= BigInt(seconds));
  }
});
