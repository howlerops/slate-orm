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

/**
 * Counts calls per method and weighs them, forwarding them untouched.
 *
 * # Why it weighs as well as counts
 *
 * `ledger/2026-09-29-bytes-do-not-need-a-network.md` weighed the Python
 * client's requests and found two things the counts could not see: a batch of
 * twenty costs *more* bytes than twenty singles, and a cursor page costs more
 * than an offset page. Both against the phrase they were meant to confirm.
 *
 * It also left this open — only Python weighed, so nothing compared the three.
 * That is the question no count can reach: three clients that send the same
 * *number* of requests can still build different ones, and the answers would
 * be identical either way. `related.test.ts` found exactly that shape in the
 * freshness floor.
 *
 * The count comes from the interceptor being entered; the weight comes from
 * `sendMessage`, because that is where the message is. The two are therefore
 * not redundant: an RPC opened and never sent would count and weigh nothing,
 * which is the honest reading of it.
 */
class Counting {
  readonly calls = new Map<string, number>();
  readonly bytes = new Map<string, number>();

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
    const weigh = (message: unknown): number =>
      // The channel's own serializer, not a re-encode of our own: this is the
      // exact buffer grpc-js is about to put on the stream, so the number
      // cannot drift from what is sent.
      options.method_definition.requestSerialize(message).length;
    return new grpc.InterceptingCall(nextCall(options), {
      sendMessage: (message, next) => {
        this.bytes.set(method, (this.bytes.get(method) ?? 0) + weigh(message));
        next(message);
      },
    });
  };

  count(method: string): number {
    return this.calls.get(method) ?? 0;
  }

  /** Serialized request bytes across every method. */
  weight(): number {
    let total = 0;
    for (const n of this.bytes.values()) total += n;
    return total;
  }

  clear(): void {
    this.calls.clear();
    this.bytes.clear();
  }

  /** So a failure message shows every method, not just the one asserted on. */
  toString(): string {
    return JSON.stringify({
      calls: Object.fromEntries(this.calls),
      bytes: Object.fromEntries(this.bytes),
    });
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
  // Through the harness rather than `Client.connect` directly, so `stop()`
  // closes the channel before it kills the child. A client built here is
  // unknown to the serving, and killing the server under a live channel
  // produced four `14 UNAVAILABLE: Connection dropped` errors *after* a test
  // had passed — node's runner turns that into a file-level failure with
  // every subtest green, which is the most confusing shape a failure has.
  // `client()` grew its options argument today for the freshness interceptor;
  // this is the second caller and the reason it was worth adding.
  const client = serving.client(APP, { interceptors: [counter.interceptor] });
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

test("batching saves round trips and costs bytes, here too", async () => {
  // The finding `ledger/2026-09-29-bytes-do-not-need-a-network.md` measured in
  // Python, re-measured here — not to confirm it, but because a *second*
  // measurement of the same shape is the only thing that can say whether the
  // two clients build the same request. Three clients sending the same number
  // of requests can still build different ones, and every answer-comparing
  // case in the conformance suite passes either way.
  //
  // The direction is the same: a batch costs more bytes than the singles it
  // replaces, because each statement gains a tag and a length prefix from
  // being nested and the batch adds an envelope. The saving is the round trip.
  const { client, counter } = await counted();
  const session = client.session();
  counter.clear();

  for (let n = 0; n < ROWS; n += 1) {
    await session.insert("docs", row(n));
  }
  const singly = counter.weight();

  counter.clear();
  const batch: Batch = {
    atomicity: "independent",
    operations: Array.from({ length: ROWS }, (_unused, n) => ({
      kind: "insert" as const,
      table: "docs",
      rows: [row(ROWS + n)],
    })),
  };
  await session.batch(batch);
  const batched = counter.weight();

  // The formula rather than the literal, for the reason the Python twin gives:
  // the totals move with the rows — a key is a varint — and the delta does
  // not. `4n + 2` is four bytes of framing per statement and two for the
  // envelope, and it held at 1, 2, 5, 10 and 20 rows when it was measured.
  //
  // The rows are not the same twenty as the singles above, because these are
  // plain inserts and a repeat would collide. Framing does not depend on the
  // key, which is what makes that safe — and is itself checked, because a
  // difference in payload would break the equality rather than hide in it.
  assert.equal(
    batched - singly,
    4 * ROWS + 2,
    `a batch costs 4 bytes per statement plus a 2-byte envelope; ` +
      `${ROWS} rows weighed ${batched} batched against ${singly} singly`,
  );
  assert.equal(counter.count("Batch"), 1, `still one call; got ${counter}`);
  client.close();
});

test("a cursor page costs more bytes than an offset page, here too", async () => {
  // The paging half, and the same correction: keyset paging's saving is in
  // store reads, not in what the caller sends. A cursor carries the last row's
  // key where an offset carries a small integer, so the cursor walk is the
  // heavier request.
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
  let cursor: Value[] | undefined;
  for (let p = 0; p < PAGES; p += 1) {
    const page = await session.page({ table: "docs", limit: PAGE_SIZE, after: cursor });
    cursor = page.cursor;
    assert.ok(cursor !== undefined, "the fixture ran out of rows before the pages did");
  }
  const byCursor = counter.weight();

  counter.clear();
  for (let p = 0; p < PAGES; p += 1) {
    // `.collect()`, not a bare `await`: `query` returns a `RowStream` rather
    // than a promise, so awaiting it is a no-op that leaves the stream
    // undrained. The first draft did exactly that, every assertion passed, and
    // node's runner then reported "asynchronous activity after the test
    // ended" four times over — a passing test with four open streams behind
    // it. Draining is also what makes the byte count right: the request is
    // sent when the stream starts, so an unstarted stream weighs nothing.
    for await (const _unused of session.query({
      table: "docs",
      limit: PAGE_SIZE,
      offset: p * PAGE_SIZE,
    })) {
      // Drained, not collected. `.collect()` returns the rows and left the
      // call's teardown in flight: node's runner reported "asynchronous
      // activity after the test ended" once per page, from a test whose every
      // assertion had passed. `for await` is what `paging.test.ts` uses and it
      // does not. A bare `await session.query(...)` is worse again — `query`
      // returns a `RowStream`, not a promise, so awaiting it sends nothing.
    }
  }
  const byOffset = counter.weight();

  // A range, not a literal: the difference is a key whose varint widens with
  // the value. What is pinned is that a cursor costs *more* and is not free —
  // a cursor dropped from the request would make the two equal.
  assert.ok(
    byCursor - byOffset >= 8 && byCursor - byOffset <= 48,
    `four cursor pages should be measurably heavier than four offset pages; ` +
      `got ${byCursor} against ${byOffset}`,
  );
  assert.equal(counter.count("Query"), PAGES, `four offset pages; got ${counter}`);
  // No `client.close()`, unlike the write cases above, and the test twin
  // directly before this one does not either. A server-streaming read tears
  // down after the rows arrive, so closing the channel the moment the last
  // `await` returns raced it: node's runner reported "asynchronous activity
  // after the test ended" with `14 UNAVAILABLE: Connection dropped`, from an
  // assertion that had already passed. The `after()` hook stops the servers.
});
