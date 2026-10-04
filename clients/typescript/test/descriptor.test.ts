/**
 * The bundled descriptor matches the `.proto` files, and both transports
 * decode the same bytes to the same objects.
 *
 * `docs/edge-client.md` §2 and §3. The first is the arrangement the Go and
 * Python stubs already have: a generated file, committed, regenerated here and
 * compared byte for byte. The second is the property the two transports exist
 * to share. A field decoded under a different name, a long as a number, an
 * enum as its index, or a default left out would make the web client a
 * different client wearing the same name. The suite running over both
 * transports would catch some of that in the answers; this catches it at
 * the decoder, for every kind of value at once.
 */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { test } from "node:test";
import * as protoLoader from "@grpc/proto-loader";

import { LOADER_OPTIONS } from "../src/node.js";
import { decode, encode, method } from "../src/transport.js";
import { directoryOf, findUpContaining } from "../src/paths.js";

const PACKAGE = findUpContaining(directoryOf(import.meta.url), ["package.json", "scripts"], "package root");

test("the generated codecs match proto/", () => {
  const run = spawnSync(process.execPath, [path.join(PACKAGE, "scripts", "generate-codecs.mjs"), "--check"], {
    encoding: "utf8",
  });
  assert.equal(run.status, 0, run.stderr || run.stdout);
});

test("both transports decode the same bytes to the same object", () => {
  const definition = protoLoader.loadSync("slate/v1/records.proto", LOADER_OPTIONS);
  const records = definition["slate.v1.Records"] as Record<
    string,
    { responseDeserialize: (bytes: Buffer) => unknown; requestDeserialize: (bytes: Buffer) => unknown }
  >;

  // Every arm of `Value`, the 64-bit cases past a double's precision, a
  // nested message, an enum, and the defaults a decoder must fill in.
  const response = {
    rows: [
      {
        values: [
          { nullValue: "NULL_VALUE" },
          { boolValue: true },
          { stringValue: "ada" },
          { int64Value: "-9007199254740993" },
          { uint64Value: "18446744073709551615" },
          { doubleValue: 0.1 },
          { decimalValue: "1250" },
          { vectorValue: { values: [0.5, 1.5] } },
          { bytesValue: new Uint8Array([1, 2, 3]) },
        ],
      },
      { values: [] },
    ],
    servedBy: { replica: "a", sequence: "9007199254740993" },
  };
  const bytes = encode(method("Query").response, response);
  const web = decode(method("Query").response, bytes);
  const native = records.Query!.responseDeserialize(Buffer.from(bytes));
  // `Buffer` and `Uint8Array` hold the same bytes and differ as types; the
  // client reads either by index, so they are compared as bytes.
  const normalised = (value: unknown): unknown =>
    JSON.parse(JSON.stringify(value, (_k, v) => (v instanceof Uint8Array ? [...v] : v)));
  assert.deepEqual(normalised(web), normalised(native));

  // And a request, which the web transport encodes and the server decodes:
  // the same object must come back out of either decoder.
  const request = { transaction: "t", query: { table: "books", limit: { value: "5" } } };
  const requestBytes = encode(method("Query").request, request);
  assert.deepEqual(
    normalised(decode(method("Query").request, requestBytes)),
    normalised(records.Query!.requestDeserialize(Buffer.from(requestBytes))),
  );
});
