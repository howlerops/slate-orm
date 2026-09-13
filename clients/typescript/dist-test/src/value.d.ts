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
export type Value = {
    kind: "null";
} | {
    kind: "bool";
    value: boolean;
} | {
    kind: "bytes";
    value: Uint8Array;
} | {
    kind: "string";
    value: string;
} | {
    kind: "int";
    value: bigint;
} | {
    kind: "uint";
    value: bigint;
} | {
    kind: "float";
    value: number;
} | {
    kind: "uuid";
    value: Uint8Array;
} | {
    kind: "vector";
    value: number[];
};
/** The absence of a value, which is not a zero. */
export declare const nullValue: Value;
/** A boolean. */
export declare const bool: (value: boolean) => Value;
/** A byte string. */
export declare const bytes: (value: Uint8Array) => Value;
/** A UTF-8 string. */
export declare const str: (value: string) => Value;
/** A signed 64-bit integer. */
export declare const int: (value: bigint | number) => Value;
/** An unsigned 64-bit integer. */
export declare const uint: (value: bigint | number) => Value;
/** A double. */
export declare const float: (value: number) => Value;
/** A UUID, as its sixteen bytes. */
export declare function uuid(value: Uint8Array): Value;
/** A dense f32 vector, for embeddings. */
export declare const vector: (value: number[]) => Value;
/** The wire form of a value, as `@grpc/proto-loader` wants it. */
export declare function valueToWire(value: Value): Record<string, unknown>;
/**
 * A value as it arrived.
 *
 * An unset `kind` throws rather than reading as null: the two mean different
 * things, and a server that sent nothing is one this client does not
 * understand. Turning that into a null would move the failure somewhere
 * further away and make it look like data.
 */
export declare function valueFromWire(wire: unknown): Value;
/** Whether two values are the same value, including their kind. */
export declare function valuesEqual(a: Value, b: Value): boolean;
/** A UUID in the usual hyphenated form. */
export declare function formatUuid(value: Uint8Array): string;
