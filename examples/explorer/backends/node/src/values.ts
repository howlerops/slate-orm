/**
 * The contract's tagged value encoding, for the TypeScript adapter.
 *
 * Three adapters encode values identically or the conformance runner is
 * comparing formatting rather than answers. The rules, and why:
 *
 * - **Tagged**, because an `i64` and a `u64` of the same magnitude are
 *   different values to this database.
 * - **64-bit integers as strings**, because a JSON number above 2^53 does not
 *   survive `JSON.parse` — this client already refuses `number` for them.
 * - **Doubles as fixed-precision strings**, because Go, Python and JavaScript
 *   each have their own shortest-round-trip float formatter and they do not
 *   always agree on the last digit.
 */
import { type Value } from "@slate-orm/client";

export function formatFloat(value: number): string {
  if (!Number.isFinite(value)) return "null";
  return value.toFixed(6);
}

export function encode(value: Value): Record<string, unknown> {
  switch (value.kind) {
    case "null":
      return { null: true };
    case "bool":
      return { bool: value.value };
    case "string":
      return { str: value.value };
    case "int":
      return { i64: value.value.toString() };
    case "uint":
      return { u64: value.value.toString() };
    case "float":
      return { f64: formatFloat(value.value) };
    case "units":
      // The units, as a string, exactly like the two integer arms — a decimal
      // is 64 bits and a JSON number would round it above 2^53, and a currency
      // total in cents is where that is reached first.
      //
      // *Not* the rendered "12.50": the scale is the column's and this
      // function has only a value. `/api/conditional-update` renders one,
      // against a scale it is given, which is where the three clients'
      // renderers are compared.
      return { decimal: value.value.toString() };
    case "bytes":
      return { bytes: Buffer.from(value.value).toString("hex") };
    case "uuid":
      return { uuid: Buffer.from(value.value).toString("hex") };
    case "vector":
      return { vector: value.value.map(formatFloat) };
    case "array":
      // Each element tagged in turn, matching the testserver's own
      // `value_json`. A list of bare values would let an adapter that decoded
      // `["1"]` as strings agree with one that decoded `[1]` as integers,
      // which is the confusion the tagging exists to stop, one level down.
      return { array: value.value.map(encode) };
  }
}

export function encodeRow(
  row: Value[] | undefined,
): Record<string, unknown>[] | null {
  return row ? row.map(encode) : null;
}

export function decode(tagged: Record<string, unknown>): Value {
  if (typeof tagged !== "object" || tagged === null) {
    throw new TypeError("a value must be a tagged object");
  }
  if ("null" in tagged) return { kind: "null" };
  if ("bool" in tagged) return { kind: "bool", value: Boolean(tagged["bool"]) };
  if ("str" in tagged) return { kind: "string", value: String(tagged["str"]) };
  if ("i64" in tagged)
    return { kind: "int", value: BigInt(String(tagged["i64"])) };
  if ("u64" in tagged)
    return { kind: "uint", value: BigInt(String(tagged["u64"])) };
  if ("f64" in tagged) return { kind: "float", value: Number(tagged["f64"]) };
  if ("decimal" in tagged)
    return { kind: "units", value: BigInt(String(tagged["decimal"])) };
  if ("array" in tagged) {
    const elements = tagged["array"];
    if (!Array.isArray(elements)) {
      throw new TypeError(
        "an array value must carry a list of tagged elements",
      );
    }
    return {
      kind: "array",
      value: elements.map((element) =>
        decode(element as Record<string, unknown>),
      ),
    };
  }
  throw new TypeError(
    `a value carried no known kind: ${Object.keys(tagged).join(", ")}`,
  );
}
