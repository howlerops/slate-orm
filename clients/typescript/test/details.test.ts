import { strict as assert } from "node:assert";
import { test } from "node:test";

import { Metadata, status as GrpcStatus, type ServiceError } from "@grpc/grpc-js";

import { DETAILS_KEY, ERROR_INFO_URL, checkFailuresOf, reasonOf } from "../src/details.js";
import { fromBatchError, fromServiceError } from "../src/errors.js";

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

/**
 * A real `grpc-status-details-bin` from a write refused by three checks,
 * captured the same way `BLOB_HEX` was — printed by
 *
 * ```
 * cargo test -p slate-server --test status -- --ignored --nocapture \
 *     emit_a_check_violation_blob
 * ```
 *
 * which exists to produce exactly this. The Python and Go suites decode the
 * same bytes.
 *
 * Three failures on purpose, and the third with neither a column nor a
 * message: `discount_under_price` spans two columns, so naming one would be a
 * lie a form renders beside the wrong field. A fixture with one failure, or
 * three identical ones, would not separate "reads the list" from "reads the
 * first" or "assumes every failure has a column".
 */
const CHECKS_HEX =
  "0803129b01726f772076696f6c61746573203320636865636b73206f6e207461626c" +
  "652060646f6373603a20607469746c655f6c656e677468603a205469746c65206d75" +
  "7374206265203120746f20383020636861726163746572732e3b206073697a655f70" +
  "6f736974697665603a2053697a652063616e6e6f74206265206e656761746976652e" +
  "3b2060646973636f756e745f756e6465725f7072696365601ae1020a28747970652e" +
  "676f6f676c65617069732e636f6d2f676f6f676c652e7270632e4572726f72496e66" +
  "6f12b4020a0f434845434b5f56494f4c4154494f4e1209736c6174652d6f726d1a0f" +
  "0a06636f6c756d6e12057469746c651a180a07636865636b2e31120d73697a655f70" +
  "6f7369746976651a1f0a07636865636b2e321214646973636f756e745f756e646572" +
  "5f70726963651a110a08636f6c756d6e2e3012057469746c651a2e0a096d65737361" +
  "67652e3012215469746c65206d757374206265203120746f20383020636861726163" +
  "746572732e1a100a08636f6c756d6e2e31120473697a651a250a096d657373616765" +
  "2e31121853697a652063616e6e6f74206265206e656761746976652e1a0d0a057461" +
  "626c651204646f63731a0f0a0a76696f6c6174696f6e731201331a170a0763686563" +
  "6b2e30120c7469746c655f6c656e6774681a150a05636865636b120c7469746c655f" +
  "6c656e677468";
const CHECKS = Buffer.from(CHECKS_HEX, "hex");

test("every failing check comes back typed", () => {
  // The payoff of publishing `column` and `message`: no prose to parse. A form
  // reads `column` to pick the field and `message` to fill it; the alternative
  // a caller has without this is a regular expression over the status text,
  // which lasts until somebody rewords a sentence.
  assert.deepEqual(checkFailuresOf(CHECKS), [
    {
      check: "title_length",
      column: "title",
      message: "Title must be 1 to 80 characters.",
    },
    { check: "size_positive", column: "size", message: "Size cannot be negative." },
    // The cross-column one: a name and nothing to hang it on.
    { check: "discount_under_price", column: "", message: "" },
  ]);
});

test("the order is the schema's and not the map's", () => {
  // `check.10` must not sort between `check.1` and `check.2`. The metadata is a
  // string-keyed map and the server sends the index in the key, so anything
  // walking the map in key order would be right for nine failures and wrong for
  // eleven. Reading `violations` and counting up is what makes that
  // unreachable, and this asserts it against the real blob, whose map order is
  // not the declaration order.
  assert.ok(
    CHECKS.indexOf(Buffer.from("check.1")) < CHECKS.indexOf(Buffer.from("check.0")),
    "the fixture no longer carries its keys out of order; it proves less now",
  );
  assert.equal(checkFailuresOf(CHECKS)[0]?.check, "title_length");
});

/**
 * An `ErrorInfo` with a chosen reason and a chosen metadata map.
 *
 * Hand-encoded in the opposite direction from the decoder, like `decoy` above
 * and for the same reason. This one exists because two properties cannot be
 * reached with a blob the server would actually send: a *non*-check failure
 * carrying check-shaped keys, and a metadata map whose count and keys disagree.
 * Both are what the decoder's guards are for.
 */
function violationDecoy(reason: string, metadata: Record<string, string>): Buffer {
  let info = Buffer.concat([
    bytesField(1, Buffer.from(reason)),
    bytesField(2, Buffer.from("slate-orm")),
  ]);
  for (const [key, value] of Object.entries(metadata)) {
    const entry = Buffer.concat([
      bytesField(1, Buffer.from(key)),
      bytesField(2, Buffer.from(value)),
    ]);
    info = Buffer.concat([info, bytesField(3, entry)]);
  }
  const any = Buffer.concat([
    bytesField(1, Buffer.from(ERROR_INFO_URL)),
    bytesField(2, info),
  ]);
  return Buffer.concat([varint((1 << 3) | 0), varint(3), bytesField(3, any)]);
}

test("the decoy is read when it says it is a check violation", () => {
  // The negative control, in the shape this file already uses. Without it the
  // two tests below pass for two different reasons — the guard working, or the
  // hand-encoder producing something no decoder could read — and only one of
  // those is the property.
  assert.deepEqual(
    checkFailuresOf(
      violationDecoy("CHECK_VIOLATION", {
        violations: "1",
        "check.0": "only",
        "column.0": "a",
      }),
    ),
    [{ check: "only", column: "a", message: "" }],
  );
});

test("check-shaped metadata under another reason is ignored", () => {
  // The real blob cannot show this: it carries no `violations` key, so a
  // decoder missing the reason check falls through to the same empty answer by
  // accident. This one carries the keys and the wrong reason, so only the check
  // itself can produce the empty list.
  assert.deepEqual(
    checkFailuresOf(
      violationDecoy("UNIQUE_VIOLATION", {
        violations: "1",
        "check.0": "not_a_check",
        "column.0": "email",
      }),
    ),
    [],
  );
});

test("a count the keys do not match yields nothing", () => {
  // Three promised, two present: the prefix would be a quiet lie. A caller
  // shown two failures for a row that broke three fixes two fields, resubmits
  // and is refused again — the round-trip-per-field behaviour this whole
  // feature exists to remove.
  assert.deepEqual(
    checkFailuresOf(
      violationDecoy("CHECK_VIOLATION", {
        violations: "3",
        "check.0": "one",
        "check.1": "two",
      }),
    ),
    [],
  );
});

test("a server sending only the unindexed pair yields one", () => {
  // One failure is the honest reading of what such a server said, and it is
  // what this client sent before the server collected them all.
  assert.deepEqual(
    checkFailuresOf(
      violationDecoy("CHECK_VIOLATION", {
        check: "title_length",
        column: "title",
        message: "Too long.",
      }),
    ),
    [{ check: "title_length", column: "title", message: "Too long." }],
  );
});

test("a failure that is not a check violation has none", () => {
  // `BLOB` is a predicate write that matched too many rows.
  assert.deepEqual(checkFailuresOf(BLOB), []);
});

test("rubbish yields no failures rather than throwing", () => {
  for (const blob of [Buffer.alloc(0), Buffer.from("not a status")]) {
    // The reasoning `reasonOf` gives: never replace the server's failure.
    assert.deepEqual(checkFailuresOf(blob), []);
  }
  for (let cut = 0; cut < CHECKS.length; cut += 1) {
    checkFailuresOf(CHECKS.subarray(0, cut));
  }
});

test("fromServiceError carries the violations onto the error", () => {
  // The wiring test, and a different test from the ones above on purpose.
  // Those call `checkFailuresOf` directly, so all of them keep passing if
  // `fromServiceError` stops asking — the reasoning the token's wiring test
  // gives two tests up, and the mutation it caught.
  const error = fromServiceError(serviceError(CHECKS));
  assert.equal(error.reason, "CHECK_VIOLATION");
  assert.deepEqual(
    error.violations.map((one) => one.column),
    ["title", "size", ""],
  );
  assert.equal(error.violations[0]?.message, "Title must be 1 to 80 characters.");
});

test("an ordinary failure carries an empty list", () => {
  // Not `undefined`: a caller iterating does not have to check first.
  assert.deepEqual(fromServiceError(serviceError(BLOB)).violations, []);
});

// --- a batch's per-operation failure -----------------------------------------

test("a batched refusal carries the same violations", () => {
  // The field a batched failure could not have. An independent batch reports
  // each failure as data inside a *successful* response, so there are no
  // trailers and no `grpc-status-details-bin` — a form submitted as a batch got
  // the token and the prose and nothing to put beside a field. The server puts
  // the same blob in the message body now.
  //
  // Against the same fixture the lone path uses, which is the point: one blob,
  // one decoder, and a batched refusal that cannot come to disagree with an
  // unbatched one.
  const error = fromBatchError({
    code: GrpcStatus.INVALID_ARGUMENT,
    message: "row violates 3 checks on table `docs`",
    reason: "CHECK_VIOLATION",
    details: CHECKS,
  });
  assert.equal(error.reason, "CHECK_VIOLATION");
  assert.deepEqual(
    error.violations.map((one) => one.check),
    ["title_length", "size_positive", "discount_under_price"],
  );
  assert.equal(error.violations[2]?.column, "");
});

test("a batched failure with no details has no violations", () => {
  // Which is most of them: the field is absent for every failure the server
  // does not attach a blob to, and that must not become a decode attempt.
  const error = fromBatchError({
    code: GrpcStatus.NOT_FOUND,
    message: "no such row",
  });
  assert.deepEqual(error.violations, []);
  assert.equal(error.kind, "not-found");
});

test("a batched failure with rubbish details does not throw", () => {
  // The reasoning `reasonOf` gives, one path over.
  const error = fromBatchError({
    code: GrpcStatus.INVALID_ARGUMENT,
    message: "refused",
    reason: "CHECK_VIOLATION",
    details: Buffer.from("not a status"),
  });
  assert.deepEqual(error.violations, []);
});
