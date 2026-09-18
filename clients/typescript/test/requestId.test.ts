import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { after, test } from "node:test";

import { findUpContaining } from "../src/paths.js";
import { start, type Serving } from "./harness.js";
import { count, REQUEST_ID_KEY, SlateError, uint, type Session } from "../src/index.js";

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

async function connected(): Promise<Session> {
  const server = await start();
  servers.push(server);
  return server.client().session();
}

/**
 * The id on a rejected promise, or a failed assertion.
 *
 * The shape is asserted as well as the presence: the server keeps only
 * `[A-Za-z0-9._:-]` and cuts at 64, so a client that started sending something
 * punctuated or longer would be *altered* on the way into the log rather than
 * refused, and the correlation would break quietly.
 */
async function idOf(what: string, run: () => Promise<unknown>): Promise<string> {
  try {
    await run();
  } catch (error) {
    assert.ok(error instanceof SlateError, `${what}: not a SlateError: ${error}`);
    assert.equal(error.requestId.length, 32, `${what}: requestId ${error.requestId}`);
    assert.match(error.requestId, /^[0-9a-f]{32}$/, what);
    return error.requestId;
  }
  assert.fail(`${what}: the call succeeded; this test needs it to fail`);
}

// Every RPC family gets a row, because the id travels beside the stream or the
// promise rather than in the request body: a call site that opened the call
// one way and reported the failure another would send an id and report none,
// and nothing in the type checker catches that.
test("every call kind names its request id", async () => {
  const session = await connected();
  const absent = "no_such_table";

  await idOf("get", () => session.get(absent, [uint(1)]));
  await idOf("insert", () => session.insert(absent, [uint(1)]));
  await idOf("explain", () => session.explain({ table: absent }));
  await idOf("page", () => session.page({ table: absent, limit: 10n }));
  await idOf("batch", () =>
    session.batch({
      atomicity: "all-or-nothing",
      operations: [{ kind: "insert", table: absent, rows: [[uint(1)]] }],
    }),
  );

  // The streaming calls fail on their first message rather than on the call,
  // so these take the other path -- the id the stream kept -- which is the one
  // a scan that dies on its tenth message also takes.
  await idOf("query", async () => {
    for await (const _ of session.query({ table: absent })) void _;
  });
  await idOf("aggregate", async () => {
    for await (const _ of session.aggregate({ table: absent }, { aggregates: [count()] }))
      void _;
  });
});

test("two calls get two ids", async () => {
  // Per call, not per session or per connection: an id shared by everything a
  // session did names every line it wrote, which is what a caller already has.
  const session = await connected();
  const first = await idOf("first", () => session.get("no_such_table", [uint(1)]));
  const second = await idOf("second", () => session.get("no_such_table", [uint(1)]));
  assert.notEqual(first, second);
});

test("the key matches the one the server reads", () => {
  // A mismatch here is silent: the server ignores metadata it does not know,
  // so a misspelled key means every call carries an id nobody ever sees and
  // every log line says `id=-`. Neither side's tests catch it alone, because
  // each is internally consistent.
  //
  // Read, not skipped when absent. A skip would be green on a wrong path, and
  // the wrong path is the likelier bug -- it was, first time: this file runs
  // compiled, from `dist-test/test/`, so a relative `../../..` counted from
  // the source tree lands one directory short. Searching upward for a marker
  // is right whatever the layout, and `findUpContaining` is what this package
  // already uses to find its own `.proto`.
  const here = path.dirname(fileURLToPath(import.meta.url));
  const root = findUpContaining(here, ["crates", "Cargo.toml"], "the repository root");
  const auth = path.join(root, "crates", "slate-server", "src", "auth.rs");
  const declared = /pub const REQUEST_ID_KEY: &str = "([^"]+)";/.exec(
    readFileSync(auth, "utf8"),
  );
  assert.ok(declared, `no REQUEST_ID_KEY in ${auth}`);
  assert.equal(REQUEST_ID_KEY, declared[1]);
});

test("the header is actually sent", async () => {
  // The mutation this file was missing. Every other test here reads the id off
  // an *error*, and the id on an error is generated client-side: deleting the
  // line that puts it in the outgoing metadata left all of them passing. A
  // client that mints an id, reports it on every failure and never sends it
  // would look perfect from inside and be useless, because the server's log
  // would say `id=-` for every call.
  //
  // This reads the metadata a call would carry, which is what goes on the wire.
  const server = await start();
  servers.push(server);
  const { metadata, requestId } = server.client().sending();

  const sent = metadata.get(REQUEST_ID_KEY);
  assert.equal(sent.length, 1, `${REQUEST_ID_KEY} appears ${sent.length} times`);
  assert.equal(sent[0], requestId, "the header and the reported id must agree");
  assert.match(String(sent[0]), /^[0-9a-f]{32}$/);

  // And beside the identity, not instead of it.
  assert.deepEqual(metadata.get("slate-principal"), ["u64:1"]);
});
