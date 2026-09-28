// How many round trips the TypeScript client makes, counted at its own channel.
//
// # Why this file exists, and what it needed first
//
// Two ledger entries say a batch is one round trip and that no client shows
// it. Python got a counter today (`clients/python/tests/test_round_trips.py`)
// and Go turned out to have had one since relations landed — `dialRecording`
// in `related_test.go` — with a batch count added beside it. TypeScript had
// nothing, and *could not* have had anything: `Client.connect` took a target,
// an identity and credentials, with no way to reach the channel.
//
// That is the finding this file starts from. Python's `Client` takes
// `channel=` and Go's `Dial` takes `...grpc.DialOption`, so a caller could
// configure the transport — interceptors, keepalive, message-size limits — in
// two languages of three. `connect` now takes a fourth `options` argument,
// forwarded to the channel untouched. Additive, so no existing call site
// changes; the gap was found by needing somewhere to put an interceptor.
//
// # Why counting rather than timing
//
// A count has no spread. The inherited 9.4x was a ratio of durations on a
// loopback socket, which is mostly scheduling, and a timing claim from this
// repository reversed between two machines earlier today. Twenty writes are
// twenty requests or they are one, on any machine.
//
// What a count cannot see is the per-request work the clients do that the Rust
// wire test did not: building a schema claim, encoding each value. Twenty rows
// cost twenty encodings inside a batch too. That half of the caveat stands and
// is recorded as the residual rather than implied away.

import assert from "node:assert/strict";
import { after, test } from "node:test";

import * as grpc from "@grpc/grpc-js";

import { start, type Serving } from "./harness.js";
import { Client, int, str, uint, type Batch, type Identity, type Value } from "../src/index.js";

/** Matches `ctx()` in the daemon's fixture, as every other test's client does. */
const APP: Identity = { principal: "u64:1", tenant: "u64:1", roles: ["app"] };

/** Counts calls per method, and forwards them untouched. */
class Counting {
  readonly calls = new Map<string, number>();

  /**
   * The grpc-js interceptor, as a bound arrow so it can be handed straight to
   * `interceptors` without losing `this`.
   */
  readonly interceptor = (options: grpc.InterceptorOptions, nextCall: grpc.NextCall) => {
    // The path is `/slate.v1.Records/Insert`, and the last segment is what a
    // reader of a failure message wants. Worth knowing: the RPC names are not
    // the client's method names — a single insert is `Insert` and a keyset
    // page is `Query`, which is how the Python version of this failed first.
    const method = options.method_definition.path.split("/").pop() ?? "?";
    this.calls.set(method, (this.calls.get(method) ?? 0) + 1);
    return new grpc.InterceptingCall(nextCall(options));
  };

  count(method: string): number {
    return this.calls.get(method) ?? 0;
  }

  clear(): void {
    this.calls.clear();
  }

  /** So a failure message shows every method, not just the one asserted on. */
  toString(): string {
    return JSON.stringify(Object.fromEntries(this.calls));
  }
}

/**
 * Far enough from one that a failure message is worth reading.
 *
 * `20 != 1` says what happened; `2 != 1` could be an off-by-one anywhere.
 */
const ROWS = 20;

/** Above anything the fixture seeds, so these rows collide with no other test. */
const FIRST_KEY = 20_000;

function row(n: number) {
  return [uint(BigInt(FIRST_KEY + n)), str("counted"), int(1n)];
}

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

/** A server of this file's own, and a client whose channel counts. */
async function counted(): Promise<{ client: Client; counter: Counting }> {
  const serving = await start();
  servers.push(serving);
  const counter = new Counting();
  const client = Client.connect(
    serving.address,
    APP,
    grpc.credentials.createInsecure(),
    { interceptors: [counter.interceptor] },
  );
  return { client, counter };
}

test("writing n rows one at a time is n round trips", async () => {
  // The control. Without it the batch case shows only that a batch works — a
  // client sending one request per row returns exactly the same rows, so
  // nothing about the answer distinguishes them.
  const { client, counter } = await counted();
  const session = client.session();
  counter.clear();

  for (let n = 0; n < ROWS; n += 1) {
    await session.insert("docs", row(n));
  }

  assert.equal(counter.count("Insert"), ROWS, `one Insert per row; got ${counter}`);
  client.close();
});

test("writing the same n rows in a batch is one round trip", async () => {
  const { client, counter } = await counted();
  const session = client.session();
  counter.clear();

  const batch: Batch = {
    atomicity: "independent",
    operations: Array.from({ length: ROWS }, (_unused, n) => ({
      kind: "insert" as const,
      table: "docs",
      rows: [row(n)],
    })),
  };
  await session.batch(batch);

  assert.equal(counter.count("Batch"), 1, `one Batch for ${ROWS} rows; got ${counter}`);
  assert.equal(counter.count("Insert"), 0, `a batch must not also Insert; got ${counter}`);
});

/**
 * Four pages of five, which the seeding below writes exactly.
 *
 * Separate constants rather than dividing `ROWS`, so that changing how many
 * rows the write cases use cannot silently resize the paging case.
 */
const PAGES = 4;
const PAGE_SIZE = 5;

test("paging by cursor is one round trip per page", async () => {
  // The other half of the same caveat. The README's "page 99 read 495
  // key-value pairs by offset and 5 by cursor" is a kernel measurement of what
  // a page reads from the store; this is what a page costs the caller, which
  // is the number someone sizing a page against a network actually needs.
  const { client, counter } = await counted();
  const session = client.session();

  const seeding: Batch = {
    atomicity: "independent",
    operations: Array.from({ length: PAGES * PAGE_SIZE }, (_unused, n) => ({
      kind: "insert" as const,
      table: "docs",
      rows: [row(n)],
    })),
  };
  await session.batch(seeding);
  counter.clear();

  // A cursor is the last row's key, not an opaque blob: `Value[]`.
  let cursor: Value[] | undefined;
  let seen = 0;
  for (let p = 0; p < PAGES; p += 1) {
    // No filter: `counted()` starts a node of this test's own and the memory
    // backend seeds nothing, so the twenty rows above are every row in `docs`.
    const page = await session.page({ table: "docs", limit: PAGE_SIZE, after: cursor });
    seen += page.rows.length;
    cursor = page.cursor;
  }

  // Both sides are literals, and that is the point: the Python version of this
  // first asserted against a page counter the loop itself incremented, so it
  // held for any number of pages including one, and a mutation shrinking the
  // loop survived. An expected value computed by the code under test is not an
  // assertion.
  //
  // `Query`, not a `Page` RPC: a keyset page is an ordinary query carrying a
  // cursor and a limit, which is the wire shape the interceptor sees.
  assert.equal(counter.count("Query"), PAGES, `${PAGES} pages cost ${PAGES} calls; got ${counter}`);
  assert.equal(seen, PAGES * PAGE_SIZE, `${PAGES} full pages of ${PAGE_SIZE}; got ${seen} rows`);
});
