import { strict as assert } from "node:assert";
import { test } from "node:test";

import { Metadata, status as GrpcStatus, type ServiceError } from "@grpc/grpc-js";

import { DETAILS_KEY, ERROR_INFO_URL, reasonOf } from "../src/details.js";
import { fromServiceError } from "../src/errors.js";

/**
 * A real `grpc-status-details-bin`, captured from a running head node rather
 * than built here: a fixture this file encoded itself would agree with this
 * file's own idea of the wire format, and agreeing with yourself proves
 * nothing. The Python and Go suites decode these same bytes, so the three
 * clients are checked against one artefact rather than three transcriptions.
 *
 * It came from a `deleteWhere` with `returning` that matched more rows than
 * `max_returned_rows` allowed.
 */
const BLOB_HEX = "08081290016120707265646963617465207772697465206d617463686564206d6f7265207468616e203320726f77732c20616e642069747320726f777320776572652061736b656420666f723b206e6172726f7720746865207072656469636174652c2064726f7020746865207265717565737420666f722074686520726f77732c206f7220726169736520746865206c696d69741a520a28747970652e676f6f676c65617069732e636f6d2f676f6f676c652e7270632e4572726f72496e666f12260a195052454449434154455f57524954455f544f4f5f4c415247451209736c6174652d6f726d";
const BLOB = Buffer.from(BLOB_HEX, "hex");

test("the token comes out of a real blob", () => {
  assert.equal(reasonOf(BLOB), "PREDICATE_WRITE_TOO_LARGE");
});

test("the url is the one the server packs under", () => {
  // Not a restatement of the constant: the captured blob carries the URL as
  // bytes, so this asserts this client's idea of it matches the server's.
  assert.ok(BLOB.includes(Buffer.from(ERROR_INFO_URL)));
});

test("anything that is not a status with an ErrorInfo is the empty token", () => {
  for (const [what, blob] of [
    ["empty", Buffer.alloc(0)],
    ["not a protobuf at all", Buffer.from([0xff, 0xff, 0xff, 0xff])],
    ["a status carrying no details", Buffer.from([0x08, 0x08])],
  ] as Array<[string, Buffer]>) {
    // Never a throw: a client that raised while building an error object would
    // replace the server's failure with its own, and the caller would stop
    // learning why the call failed at all.
    assert.equal(reasonOf(blob), "", what);
  }
});

test("a truncated blob does not throw", () => {
  for (let cut = 0; cut < BLOB.length; cut += 1) {
    const got = reasonOf(BLOB.subarray(0, cut));
    assert.ok(got === "" || got === "PREDICATE_WRITE_TOO_LARGE", `prefix ${cut} -> ${got}`);
  }
});

function varint(value: number): Buffer {
  const out: number[] = [];
  let rest = value;
  for (;;) {
    const byte = rest & 0x7f;
    rest >>>= 7;
    out.push(byte | (rest ? 0x80 : 0));
    if (!rest) return Buffer.from(out);
  }
}

function bytesField(number: number, payload: Buffer): Buffer {
  return Buffer.concat([varint((number << 3) | 2), varint(payload.length), payload]);
}

/**
 * A well-formed `Status` whose one detail is packed under `url`.
 *
 * Encoded by hand, in the opposite direction from the decoder under test, so
 * that agreeing with it says something. The payload is shaped exactly like an
 * `ErrorInfo` — field 1, a string — so a decoder that skipped the type check
 * would hand back `reason` rather than `""`. That is the whole point: the check
 * is what stops one message's field 1 being read as another's.
 */
function decoy(url: string, reason = "DECOY"): Buffer {
  const packed = bytesField(1, Buffer.from(reason));
  const any = Buffer.concat([bytesField(1, Buffer.from(url)), bytesField(2, packed)]);
  return Buffer.concat([varint((1 << 3) | 0), varint(8), bytesField(3, any)]);
}

test("a detail of another type is skipped, not misread", () => {
  assert.equal(reasonOf(decoy("type.googleapis.com/google.rpc.RetryInfo")), "");
});

test("the decoy would be read without the type check", () => {
  // The negative control for the test above. Without it that test passes for
  // two different reasons — the URL check working, or the decoy being
  // malformed and decoding to nothing either way — and only one is the
  // property. Packed under the URL the server really uses, the very same bytes
  // come back as a token.
  assert.equal(reasonOf(decoy(ERROR_INFO_URL)), "DECOY");
});

/**
 * A `ServiceError` as grpc-js delivers one, with the blob in its metadata.
 *
 * Hand-built rather than mocked: `fromServiceError` reads four things off the
 * error and this supplies exactly those, so a change to what it reads shows up
 * here rather than as a silently empty field.
 */
function serviceError(blob: Buffer): ServiceError {
  const metadata = new Metadata();
  // A text entry beside the binary one, because the trailer map and the reason
  // walk the same metadata and must not trip over each other.
  metadata.set("content-type", "application/grpc");
  metadata.set(DETAILS_KEY, blob);
  return {
    name: "Error",
    message: "resource exhausted",
    code: GrpcStatus.RESOURCE_EXHAUSTED,
    details: "a predicate write matched more rows than it may return",
    metadata,
  } as ServiceError;
}

test("fromServiceError carries the token onto the error", () => {
  // The wiring test, and a different test from the ones above on purpose.
  // Those call `reasonOf` directly, so all of them keep passing if
  // `fromServiceError` stops asking for a token — the state this change fixed.
  // The same mutation in the Go client survived every decoder test and was
  // caught only by the conformance runner.
  const error = fromServiceError(serviceError(BLOB));
  assert.equal(error.reason, "PREDICATE_WRITE_TOO_LARGE");
  // Unchanged by carrying a token: the kind, and the text trailers.
  assert.equal(error.kind, "resource-limit");
  assert.deepEqual(error.trailers, { "content-type": "application/grpc" });
});

test("a failure with no details has the empty token", () => {
  assert.equal(fromServiceError(serviceError(Buffer.alloc(0))).reason, "");
});
