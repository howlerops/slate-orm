/**
 * Column values.
 *
 * A tagged union rather than plain JavaScript values, for the reason the Go
 * client gives at more length: `7` as an `i64` and `7` as a `u64` are
 * *different values* to this server — its ordering is type-first — and
 * JavaScript cannot tell them apart. Guessing would write rows that cannot be
 * found again.
 *
 * 64-bit integers are `bigint`, not `number`. A `number` silently loses
 * precision above 2^53, and a primary key is exactly where that shows up
 * latest and hurts most.
 */
export type Value =
  | { kind: "null" }
  | { kind: "bool"; value: boolean }
  | { kind: "bytes"; value: Uint8Array }
  | { kind: "string"; value: string }
  | { kind: "int"; value: bigint }
  | { kind: "uint"; value: bigint }
  | { kind: "float"; value: number }
  | { kind: "units"; value: bigint }
  | { kind: "uuid"; value: Uint8Array }
  | { kind: "vector"; value: number[] };

/** The absence of a value, which is not a zero. */
export const nullValue: Value = { kind: "null" };

/** A boolean. */
export const bool = (value: boolean): Value => ({ kind: "bool", value });

/** A byte string. */
export const bytes = (value: Uint8Array): Value => ({ kind: "bytes", value });

/** A UTF-8 string. */
export const str = (value: string): Value => ({ kind: "string", value });

/** A signed 64-bit integer. */
export const int = (value: bigint | number): Value => ({
  kind: "int",
  value: BigInt(value),
});

/** An unsigned 64-bit integer. */
export const uint = (value: bigint | number): Value => ({
  kind: "uint",
  value: BigInt(value),
});

/** A double. */
export const float = (value: number): Value => ({ kind: "float", value });

/**
 * A count of a decimal column's smallest unit.
 *
 * The same type the Rust surface has, and for the same reason: the *scale*
 * lives in the schema, not in the value. `units(1250n)` in a column declared
 * scale 2 is 12.50, and the identical value in a scale-0 column is 1250.
 * Nothing on the wire says which, because the protocol publishes no schema.
 *
 * `bigint`, like the other 64-bit integers — a decimal column is exactly where
 * a `number`'s 2^53 would be reached by a currency total and lost silently.
 */
export const units = (value: bigint | number): Value => ({
  kind: "units",
  value: BigInt(value),
});

/**
 * Render units against a scale, as a decimal string.
 *
 * Mirrors `slate_orm::Units::to_string_with_scale`, and the conformance corpus
 * compares the two. A scale below zero is treated as zero rather than throwing:
 * this is a rendering helper, and a caller who got a scale wrong wants a number
 * they can see is wrong, not an exception in a render path.
 */
export function unitsToString(value: bigint, scale: number): string {
  const digits = Math.max(0, Math.trunc(scale));
  if (digits === 0) {
    return value.toString();
  }
  const divisor = 10n ** BigInt(digits);
  const negative = value < 0n;
  const magnitude = negative ? -value : value;
  const whole = magnitude / divisor;
  const part = magnitude % divisor;
  return `${negative ? "-" : ""}${whole}.${part.toString().padStart(digits, "0")}`;
}

/** A UUID, as its sixteen bytes. */
export function uuid(value: Uint8Array): Value {
  if (value.length !== 16) {
    throw new TypeError(`a uuid is 16 bytes, got ${value.length}`);
  }
  return { kind: "uuid", value };
}

/** A dense f32 vector, for embeddings. */
export const vector = (value: number[]): Value => ({ kind: "vector", value });

/** The wire form of a value, as `@grpc/proto-loader` wants it. */
export function valueToWire(value: Value): Record<string, unknown> {
  switch (value.kind) {
    case "null":
      return { nullValue: "NULL_VALUE" };
    case "bool":
      return { boolValue: value.value };
    case "bytes":
      return { bytesValue: Buffer.from(value.value) };
    case "string":
      return { stringValue: value.value };
    case "int":
      return { int64Value: value.value.toString() };
    case "uint":
      return { uint64Value: value.value.toString() };
    case "float":
      return { doubleValue: value.value };
    case "units":
      return { decimalValue: value.value.toString() };
    case "uuid":
      return { uuidValue: Buffer.from(value.value) };
    case "vector":
      return { vectorValue: { elements: value.value } };
  }
}

/**
 * A value as it arrived.
 *
 * An unset `kind` throws rather than reading as null: the two mean different
 * things, and a server that sent nothing is one this client does not
 * understand. Turning that into a null would move the failure somewhere
 * further away and make it look like data.
 */
export function valueFromWire(wire: unknown): Value {
  if (wire === null || typeof wire !== "object") {
    throw new TypeError("slate: a column carried no value at all");
  }
  const w = wire as Record<string, unknown>;
  const which = w["kind"];
  switch (which) {
    case "nullValue":
      return nullValue;
    case "boolValue":
      return { kind: "bool", value: Boolean(w["boolValue"]) };
    case "bytesValue":
      return { kind: "bytes", value: toBytes(w["bytesValue"]) };
    case "stringValue":
      return { kind: "string", value: String(w["stringValue"]) };
    case "int64Value":
      return { kind: "int", value: BigInt(String(w["int64Value"])) };
    case "uint64Value":
      return { kind: "uint", value: BigInt(String(w["uint64Value"])) };
    case "doubleValue":
      return { kind: "float", value: Number(w["doubleValue"]) };
    case "decimalValue":
      return { kind: "units", value: BigInt(String(w["decimalValue"])) };
    case "uuidValue": {
      const raw = toBytes(w["uuidValue"]);
      if (raw.length !== 16) {
        throw new TypeError(`slate: a uuid value carried ${raw.length} bytes, not 16`);
      }
      return { kind: "uuid", value: raw };
    }
    case "vectorValue": {
      const v = w["vectorValue"] as { elements?: number[] } | undefined;
      return { kind: "vector", value: v?.elements ?? [] };
    }
    default:
      throw new TypeError(
        `slate: a column carried a value kind this client does not know: ${String(which)}`,
      );
  }
}

function toBytes(raw: unknown): Uint8Array {
  if (raw instanceof Uint8Array) return raw;
  if (typeof raw === "string") return Buffer.from(raw, "base64");
  return new Uint8Array();
}

/** Whether two values are the same value, including their kind. */
export function valuesEqual(a: Value, b: Value): boolean {
  if (a.kind !== b.kind) return false;
  switch (a.kind) {
    case "null":
      return true;
    case "bytes":
    case "uuid": {
      const other = (b as { value: Uint8Array }).value;
      return a.value.length === other.length && a.value.every((x, i) => x === other[i]);
    }
    case "vector": {
      const other = (b as { value: number[] }).value;
      return a.value.length === other.length && a.value.every((x, i) => x === other[i]);
    }
    default:
      return a.value === (b as { value: unknown }).value;
  }
}

/** A UUID in the usual hyphenated form. */
export function formatUuid(value: Uint8Array): string {
  const hex = Buffer.from(value).toString("hex");
  return [
    hex.slice(0, 8),
    hex.slice(8, 12),
    hex.slice(12, 16),
    hex.slice(16, 20),
    hex.slice(20, 32),
  ].join("-");
}

/**
 * A string that is the same for two values exactly when they are the same
 * value, for use as a `Map` key.
 *
 * The kind is part of it: `7` as an `i64` and `7` as a `u64` are different
 * values to this server, and a key built from the number alone would collapse
 * them into one — which is the same mistake the tagged union exists to stop.
 *
 * Not the serialised protobuf, which is what the Go and Python clients use: a
 * `Value` arriving here has already been decoded, and re-encoding it to
 * compare would mean carrying the wire shape around for no gain.
 */
export function valueKey(value: Value): string {
  switch (value.kind) {
    case "null":
      return "null";
    case "bytes":
    case "uuid":
      return `${value.kind}:${Buffer.from(value.value).toString("hex")}`;
    case "vector":
      return `vector:${value.value.join(",")}`;
    default:
      return `${value.kind}:${String(value.value)}`;
  }
}

/**
 * A row, in the shape the wire wants.
 *
 * Here rather than beside the client because `query.ts` needs it too, for a
 * batch's rows, and `client.ts` imports `query.ts` — so a copy there would be
 * a circular import or a second implementation.
 */
export function rowToWire(values: Value[]): Record<string, unknown> {
  return { values: values.map(valueToWire) };
}
