/**
 * A per-call deadline.
 *
 * Before this the client passed no `CallOptions` on any RPC, so a head node
 * that accepted a connection and then stopped answering blocked the caller for
 * ever — the failure mode a deadline exists for, and the one no error-code
 * classification helps with, because no error ever arrives.
 *
 * "a call with no deadline does not come back" is the half that makes the rest
 * mean anything: it demonstrates the hang against a listener that accepts and
 * says nothing, rather than asserting only that the fixed version works.
 *
 * The unit is **milliseconds**, which is every other duration in JavaScript.
 * The Python client's `with_timeout` takes seconds. Both name the unit in the
 * parameter, which is the whole defence against a caller reading one and
 * writing the other.
 */
import assert from "node:assert/strict";
import net from "node:net";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import { Client, SlateError, type Identity, type TableDef } from "../src/index.js";

/** `docs`, as the harness's fixture declares it. */
const DOCS: TableDef = {
  name: "docs",
  columns: [
    { name: "id", type: "u64" },
    { name: "kind", type: "string" },
    { name: "size", type: "i64" },
  ],
  primaryKey: ["id"],
};

const APP: Identity = { principal: "u64:1", tenant: "u64:1", roles: ["app"] };

/** Long enough that a loopback round trip could not explain the failure. */
const GRACE_MS = 1000;

// The two tests that wait on a deadline carry `{ timeout: GRACE_MS * 8 }`.
// Without it, a build that stopped passing the deadline makes them hang rather
// than fail — the runner sits with no output, which reads as broken
// infrastructure rather than a broken assertion. That is not hypothetical: it
// is what the mutation that deletes `#options()` from `call` actually does.

const servers: Serving[] = [];
const listeners: net.Server[] = [];
const accepted: net.Socket[] = [];
const clients: Client[] = [];

after(() => {
  for (const s of servers) s.stop();
  for (const c of clients) c.close();
  // Destroyed, not just closed: `Server.close` stops accepting and leaves
  // established connections open, and one call is deliberately still pending
  // on one of them — so without this the test runner never exits.
  for (const socket of accepted) socket.destroy();
  for (const listener of listeners) listener.close();
});

/**
 * A listener that accepts connections and never speaks.
 *
 * Not a slow server and not a closed port: either produces a different error.
 * This is the one shape where the client's own deadline is the only thing that
 * can end the call — gRPC completes the TCP connect and then waits for a
 * server preface that never comes.
 */
async function silent(): Promise<string> {
  const listener = net.createServer((socket) => {
    // Held open, so the kernel does not close it and hand gRPC a reset, which
    // would be `unavailable` rather than a deadline.
    accepted.push(socket);
  });
  listeners.push(listener);
  await new Promise<void>((resolve) => listener.listen(0, "127.0.0.1", resolve));
  const address = listener.address();
  assert.ok(address && typeof address === "object");
  return `127.0.0.1:${address.port}`;
}

function connect(target: string, timeoutMs?: number): Client {
  const client = Client.connect(target, APP).withTimeout(timeoutMs);
  clients.push(client);
  return client;
}

const ONE: [{ kind: "uint"; value: bigint }] = [{ kind: "uint", value: 1n }];

test("a silent server is a deadline rather than a hang", { timeout: GRACE_MS * 8 }, async () => {
  const session = connect(await silent(), GRACE_MS).session();
  const started = Date.now();
  await assert.rejects(
    () => session.get("docs", ONE),
    (error: Error) => {
      assert.ok(error instanceof SlateError, `${error}`);
      assert.equal(error.kind, "deadline-exceeded", error.message);
      return true;
    },
  );
  const elapsed = Date.now() - started;
  // Bounded both ways. Too fast would mean something else failed the call and
  // the deadline proved nothing; too slow would mean the number is not the one
  // being honoured.
  assert.ok(elapsed >= GRACE_MS - 50, `came back in ${elapsed}ms`);
  assert.ok(elapsed < GRACE_MS + 5000, `came back in ${elapsed}ms`);
});

test("the deadline reaches a streaming call too", { timeout: GRACE_MS * 8 }, async () => {
  // `get` is unary and `query` streams, and they are separate lines in
  // `Client` — one of them carrying the options is not both.
  const session = connect(await silent(), GRACE_MS).session();
  await assert.rejects(
    () => session.query({ table: "docs" }).collect(),
    (error: Error) => {
      assert.equal((error as SlateError).kind, "deadline-exceeded", error.message);
      return true;
    },
  );
});

test("a call with no deadline does not come back", async () => {
  // The hang this feature exists to end, demonstrated. The promise is left
  // pending on purpose: there is nothing that can cancel it, which is the
  // point. It settles when `after` destroys the socket.
  const session = connect(await silent()).session();
  let settled = false;
  const mark = (): void => {
    settled = true;
  };
  void session.get("docs", ONE).then(mark, mark);
  await new Promise((resolve) => setTimeout(resolve, GRACE_MS * 2));
  assert.equal(settled, false, "a call with no deadline came back");
});

test("a view does not change the client it came from", async () => {
  // The reason it is a new object rather than a setting: a short deadline left
  // switched on by a caller who forgot to restore it is a bug this shape
  // cannot have.
  //
  // Asserted on the *original*, which is what a `withTimeout` implemented as
  // `this.#timeoutMs = ms; return this` would break — and deliberately not on
  // the view, because "1ms is too short for a real call" is a race rather than
  // a property. The silent-server tests above cover the view.
  const serving = await start();
  servers.push(serving);
  const patient = serving.client();
  const impatient = patient.withTimeout(1);
  assert.notEqual(impatient, patient, "a view is a different object");

  await patient.session().get("docs", ONE);
});

test("a view keeps the schema declarations it came from", async () => {
  // `declaring` is the one piece of state on `Client` that is not the
  // connection, and a `withTimeout` that dropped it would silently stop
  // sending the schema claim. That fails *open* — a request with no claim is
  // served exactly as before — so nothing else would notice.
  const serving = await start();
  servers.push(serving);
  const declared = serving.client().declaring({ docs: DOCS });
  const view = declared.withTimeout(5000);
  assert.deepEqual(view.claim("docs"), declared.claim("docs"));
  assert.ok(view.claim("docs"), "the fixture should produce a claim at all");
});
