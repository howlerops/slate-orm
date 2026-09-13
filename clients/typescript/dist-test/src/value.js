/** The absence of a value, which is not a zero. */
export const nullValue = { kind: "null" };
/** A boolean. */
export const bool = (value) => ({ kind: "bool", value });
/** A byte string. */
export const bytes = (value) => ({ kind: "bytes", value });
/** A UTF-8 string. */
export const str = (value) => ({ kind: "string", value });
/** A signed 64-bit integer. */
export const int = (value) => ({
    kind: "int",
    value: BigInt(value),
});
/** An unsigned 64-bit integer. */
export const uint = (value) => ({
    kind: "uint",
    value: BigInt(value),
});
/** A double. */
export const float = (value) => ({ kind: "float", value });
/** A UUID, as its sixteen bytes. */
export function uuid(value) {
    if (value.length !== 16) {
        throw new TypeError(`a uuid is 16 bytes, got ${value.length}`);
    }
    return { kind: "uuid", value };
}
/** A dense f32 vector, for embeddings. */
export const vector = (value) => ({ kind: "vector", value });
/** The wire form of a value, as `@grpc/proto-loader` wants it. */
export function valueToWire(value) {
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
export function valueFromWire(wire) {
    if (wire === null || typeof wire !== "object") {
        throw new TypeError("slate: a column carried no value at all");
    }
    const w = wire;
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
        case "uuidValue": {
            const raw = toBytes(w["uuidValue"]);
            if (raw.length !== 16) {
                throw new TypeError(`slate: a uuid value carried ${raw.length} bytes, not 16`);
            }
            return { kind: "uuid", value: raw };
        }
        case "vectorValue": {
            const v = w["vectorValue"];
            return { kind: "vector", value: v?.elements ?? [] };
        }
        default:
            throw new TypeError(`slate: a column carried a value kind this client does not know: ${String(which)}`);
    }
}
function toBytes(raw) {
    if (raw instanceof Uint8Array)
        return raw;
    if (typeof raw === "string")
        return Buffer.from(raw, "base64");
    return new Uint8Array();
}
/** Whether two values are the same value, including their kind. */
export function valuesEqual(a, b) {
    if (a.kind !== b.kind)
        return false;
    switch (a.kind) {
        case "null":
            return true;
        case "bytes":
        case "uuid": {
            const other = b.value;
            return a.value.length === other.length && a.value.every((x, i) => x === other[i]);
        }
        case "vector": {
            const other = b.value;
            return a.value.length === other.length && a.value.every((x, i) => x === other[i]);
        }
        default:
            return a.value === b.value;
    }
}
/** A UUID in the usual hyphenated form. */
export function formatUuid(value) {
    const hex = Buffer.from(value).toString("hex");
    return [
        hex.slice(0, 8),
        hex.slice(8, 12),
        hex.slice(12, 16),
        hex.slice(16, 20),
        hex.slice(20, 32),
    ].join("-");
}
