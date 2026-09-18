/**
 * Ask the deployed head node the same questions the Python check asks, through
 * the TypeScript SDK, and require the same answers.
 *
 * The oracle stays in Python. `check.py --expect` writes what its fold
 * computed and this reads it, rather than decoding the packed trip file again:
 * three decoders of one file would be three places to be wrong, and a
 * common-mode error in them would agree with itself. What is checked here is
 * the *client* — that a TypeScript caller building the same request over the
 * same socket gets the same answer as a Python one.
 *
 * Which matters because the two build requests differently. `BigInt` versus
 * `int`, a `Column` with a `kind` versus a `ColumnRef` with a oneof, `sort`
 * keys that carry both a `column` and a `ref`. Every one of those is a place a
 * client could send something subtly different, and until this file the
 * deployed example exercised exactly one SDK.
 */

import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";

import {
  at,
  avgOf,
  Client,
  col,
  computed0,
  count,
  eq,
  extract,
  joinComputed,
  newJoin,
  ref,
  uint,
  type Grouping,
  type Identity,
  type Session,
} from "@slate-orm/client";

/** What `check.py --expect` wrote. */
interface Expected {
  readonly trips: number;
  readonly byHour: Record<string, number>;
  readonly byBorough: Record<string, number>;
  readonly joinedHours: Record<string, [number, number]>;
  readonly zone132: number;
  readonly replicas: string[];
  /** The sequence the Python arm pinned its reads to. */
  readonly pinnedSequence: number;
}

// The ordinals `head.toml` declares. Written out rather than fetched, which is
// this client's arrangement too: it names a table and an ordinal.
const TRIP_PICKUP_ZONE = 1;
const TRIP_PICKUP_TIME = 3;
const TRIP_FARE = 7;
const ZONE_ID = 0;
const ZONE_BOROUGH = 1;

const failures: string[] = [];

function check(name: string, ok: boolean, detail = ""): void {
  if (ok) {
    console.log(`ok    ${name}`);
    return;
  }
  console.log(`FAIL  ${name}   ${detail}`);
  failures.push(name);
}

/** One aggregate over one table, as a number, with the view that served it. */
async function aggregateOne(
  session: Session,
  table: string,
  grouping: Grouping,
): Promise<{ value: number; servedBy: string | undefined }> {
  const stream = session.aggregate({ table }, grouping);
  const groups = await stream.collect();
  const first = groups[0];
  if (!first) return { value: 0, servedBy: stream.servedBy?.replica };
  const n = first.values[0];
  if (n?.kind !== "uint") throw new Error(`a count came back as ${n?.kind}`);
  return { value: Number(n.value), servedBy: stream.servedBy?.replica };
}

async function main(): Promise<number> {
  const { values } = parseArgs({
    options: {
      address: { type: "string" },
      expect: { type: "string" },
    },
  });
  if (!values.address || !values.expect) {
    console.error("usage: check --address HOST:PORT --expect FILE");
    return 2;
  }
  const want = JSON.parse(readFileSync(values.expect, "utf8")) as Expected;

  const identity: Identity = {
    principal: "u64:1",
    tenant: "u64:1",
    roles: ["app"],
  };
  const client = Client.connect(values.address, identity);
  const session = client.session();

  // --- pin to the snapshot the Python arm pinned to ----------------------
  //
  // This process starts with an empty watermark, so without this every read
  // below asks for nothing in particular and may be served by a replica that
  // has not finished polling the load — and then the check reports a
  // disagreement with the fold that is really a disagreement in time. It is
  // the same defect the Python arm had, and it hides better here: this runs
  // last, so the replicas have had longest and the luck holds most often.
  //
  // `observe` rather than a per-read argument because this client does not
  // have one — Python's `Client.query(freshness=…)` has no counterpart in
  // TypeScript or Go, where the session watermark is the whole mechanism.
  // `observe` is documented for carrying a position between sessions, which
  // is precisely what `pinnedSequence` is.
  check(
    "the Python arm named a sequence to pin to",
    want.pinnedSequence > 0,
    `pinnedSequence ${want.pinnedSequence}`,
  );
  session.observe(BigInt(want.pinnedSequence));

  // --- every row is there ------------------------------------------------
  const total = await aggregateOne(session, "trips", { aggregates: [count()] });
  check(
    "every trip is visible to the TypeScript client too",
    total.value === want.trips,
    `${total.value} against ${want.trips}`,
  );
  check(
    "and the response names the view that served it",
    total.servedBy !== undefined && want.replicas.includes(total.servedBy),
    `${total.servedBy}`,
  );

  // --- a computed column, grouped ---------------------------------------
  const byHour: Record<string, number> = {};
  for (const group of await session
    .aggregate(
      {
        table: "trips",
        compute: [extract("hour", col(TRIP_PICKUP_TIME))],
      },
      { groupBy: [computed0(0)], aggregates: [count()] },
    )
    .collect()) {
    const key = group.key[0];
    const n = group.values[0];
    if (key?.kind !== "int" || n?.kind !== "uint") {
      throw new Error(`a group is ${key?.kind} / ${n?.kind}`);
    }
    byHour[String(key.value)] = Number(n.value);
  }
  check(
    "trips per hour of day agree with the Python fold",
    sameCounts(byHour, want.byHour),
    firstDifference(byHour, want.byHour),
  );

  // --- a join, with the key and an aggregate both on the left -----------
  const b = newJoin();
  const trips = b.add({ table: "trips" });
  b.add({
    table: "zones",
    on: [{ earlier: at(trips, TRIP_PICKUP_ZONE), own: ZONE_ID }],
  });
  const joined: Record<string, [number, number]> = {};
  for (const group of await session
    .aggregateJoin(
      {
        ...b.query(),
        compute: [extract("hour", ref(at(trips, TRIP_PICKUP_TIME)))],
      },
      {
        groupBy: [joinComputed(0)],
        aggregates: [count(), avgOf(at(trips, TRIP_FARE))],
      },
    )
    .collect()) {
    const key = group.key[0];
    const n = group.values[0];
    const mean = group.values[1];
    if (key?.kind !== "int" || n?.kind !== "uint" || mean?.kind !== "float") {
      throw new Error(`a joined group is ${key?.kind} / ${n?.kind} / ${mean?.kind}`);
    }
    joined[String(key.value)] = [Number(n.value), mean.value];
  }
  check(
    "the hour and the average fare, both from `trips`, across a join",
    sameJoined(joined, want.joinedHours),
    JSON.stringify(Object.entries(joined).slice(0, 2)),
  );

  // --- a group key on the right side of the join ------------------------
  const rb = newJoin();
  const rtrips = rb.add({ table: "trips" });
  const rzones = rb.add({
    table: "zones",
    on: [{ earlier: at(rtrips, TRIP_PICKUP_ZONE), own: ZONE_ID }],
  });
  const byBorough: Record<string, number> = {};
  for (const group of await session
    .aggregateJoin(rb.query(), {
      groupBy: [at(rzones, ZONE_BOROUGH)],
      aggregates: [count()],
    })
    .collect()) {
    const key = group.key[0];
    const n = group.values[0];
    if (key?.kind !== "string" || n?.kind !== "uint") {
      throw new Error(`a borough group is ${key?.kind} / ${n?.kind}`);
    }
    byBorough[key.value] = Number(n.value);
  }
  check(
    "grouping by the right table's borough agrees with the fold",
    sameCounts(byBorough, want.byBorough),
    firstDifference(byBorough, want.byBorough),
  );

  // --- the index answers without reading a row --------------------------
  const plan = await session.explain({
    table: "trips",
    filter: eq(TRIP_PICKUP_ZONE, uint(132n)),
    columns: [TRIP_PICKUP_ZONE],
  });
  check(
    "an index answers the covering query without touching a row",
    plan.indexOnly && plan.display.includes("by_pickup_zone"),
    `${plan.display} indexOnly=${plan.indexOnly}`,
  );

  const inZone = await session
    .aggregate(
      { table: "trips", filter: eq(TRIP_PICKUP_ZONE, uint(132n)) },
      { aggregates: [count()] },
    )
    .collect();
  const first = inZone[0]?.values[0];
  const counted = first?.kind === "uint" ? Number(first.value) : -1;
  check(
    "the indexed count is the fold's count",
    counted === want.zone132,
    `${counted} against ${want.zone132}`,
  );

  client.close();
  console.log();
  if (failures.length) {
    console.log(`${failures.length} failed: ${failures.join(", ")}`);
    return 1;
  }
  console.log("the TypeScript client agrees with the Python fold, through the same socket");
  return 0;
}

function sameCounts(got: Record<string, number>, want: Record<string, number>): boolean {
  const keys = Object.keys(want);
  return (
    Object.keys(got).length === keys.length && keys.every((key) => got[key] === want[key])
  );
}

function firstDifference(got: Record<string, number>, want: Record<string, number>): string {
  for (const key of Object.keys(want).sort()) {
    if (got[key] !== want[key]) return `${key}: ${got[key]} against ${want[key]}`;
  }
  return `${Object.keys(got).length} groups against ${Object.keys(want).length}`;
}

function sameJoined(
  got: Record<string, [number, number]>,
  want: Record<string, [number, number]>,
): boolean {
  const keys = Object.keys(want);
  if (Object.keys(got).length !== keys.length) return false;
  return keys.every((key) => {
    const mine = got[key];
    const theirs = want[key]!;
    // The average is a float computed two different ways — a streaming mean in
    // the kernel, a sum over a list in Python — so it is compared with a
    // tolerance rather than for equality.
    return mine !== undefined && mine[0] === theirs[0] && Math.abs(mine[1] - theirs[1]) < 1e-6;
  });
}

process.exitCode = await main();
