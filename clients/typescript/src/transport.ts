/**
 * How a call reaches a head node, separated from what the call means.
 *
 * Every request this client makes goes through two operations — one answer,
 * or a stream of them — and that pair is the seam. `docs/edge-client.md` §2.
 * `node.ts` implements it over `@grpc/grpc-js`, which is what `Client.connect`
 * has always used; `web.ts` implements it over `fetch` and gRPC-web, for a
 * runtime with no `node:http2` and no disk: Cloudflare Workers, Deno, a
 * browser.
 *
 * **Both must hand back the same objects**, or there are two clients wearing
 * one name. So both decode with the conversion options defined here, once —
 * the ones `@grpc/proto-loader` applies — and a failure from either is shaped
 * like grpc-js's `ServiceError`, which is what `errors.ts` reads.
 *
 * This module imports nothing from Node and nothing from grpc-js. That is a
 * property the web entry point depends on, not a style: `@grpc/grpc-js`
 * imports `node:http2` when it is loaded, so a module that so much as imports
 * a constant from it cannot be loaded on Workers.
 */
import type protobuf from "protobufjs/minimal.js";

import { METHODS } from "./descriptor.js";
import { slate } from "./generated/records.js";

// 64-bit integers decode through the `long` package, which protobufjs 7.6
// reaches with a plain `require("long")` — resolved statically by any bundler,
// `wrangler` included. An earlier draft installed it by hand on the theory that
// protobufjs found it through an `eval`, which Workers refuse; a mutation
// removing that install survived the four-adapter conformance run on `workerd`,
// and reading `util/minimal.js` showed the theory was about older versions.

/**
 * How messages become objects: proto-loader's own options, so both transports
 * produce the same shapes. `longs: String` because a u64 above 2^53 would lose
 * precision as a number; `enums: String` because the client compares names;
 * `defaults` and `oneofs` because the reading code relies on both.
 */
export const CONVERSION: protobuf.IConversionOptions = {
  longs: String,
  enums: String,
  defaults: true,
  oneofs: true,
};

/** Call metadata, as plain strings; binary entries carry a `-bin` suffix. */
export type Metadata = Record<string, string>;

/**
 * The metadata of a failed call, with grpc-js's two accessors.
 *
 * `errors.ts` reads `getMap()` for the text entries and `get(key)` for the
 * binary `grpc-status-details-bin`. A shape rather than grpc-js's class, so
 * the web transport can produce one without loading grpc-js.
 */
export interface FailureMetadata {
  getMap(): Record<string, string | Uint8Array>;
  get(key: string): Array<string | Uint8Array>;
}

/** A failed call, shaped like grpc-js's `ServiceError`. */
export interface CallFailure extends Error {
  code: number;
  details: string;
  metadata: FailureMetadata;
}

/** A server stream: messages in order, and a way to stop early. */
export interface MessageStream extends AsyncIterable<unknown> {
  cancel(): void;
}

export interface Transport {
  unary(method: string, request: unknown, metadata: Metadata, deadline?: number): Promise<unknown>;
  stream(method: string, request: unknown, metadata: Metadata, deadline?: number): MessageStream;
  close(): void;
}

/** Plain metadata, as a {@link FailureMetadata}. */
export function failureMetadata(entries: Record<string, string | Uint8Array>): FailureMetadata {
  return {
    getMap: () => ({ ...entries }),
    get: (key: string) => (key in entries ? [entries[key]!] : []),
  };
}

/** A {@link CallFailure} from a status, a message and its metadata. */
export function callFailure(
  code: number,
  details: string,
  entries: Record<string, string | Uint8Array> = {},
): CallFailure {
  const error = new Error(`${code} ${details}`) as CallFailure;
  error.code = code;
  error.details = details;
  error.metadata = failureMetadata(entries);
  return error;
}

/**
 * One message type's generated codec: the static functions `pbjs -t
 * static-module` writes, which are what protobufjs would otherwise compile at
 * runtime with `new Function` — and Workers refuse that.
 */
export interface Codec {
  encode(message: unknown): { finish(): Uint8Array };
  decode(bytes: Uint8Array): unknown;
  fromObject(object: Record<string, unknown>): unknown;
  toObject(message: unknown, options?: protobuf.IConversionOptions): unknown;
}

type Method = {
  request: Codec;
  response: Codec;
  serverStreaming: boolean;
};

/** A generated message type by its full name, `slate.v1.QueryRequest`. */
function codec(fullName: string): Codec {
  let at: unknown = { slate };
  for (const part of fullName.split(".")) at = (at as Record<string, unknown>)[part];
  if (!at) throw new Error(`slate: no generated codec for ${fullName}`);
  return at as Codec;
}

/**
 * An RPC in `records.proto`, with its two codecs. The generator refused any
 * method that streams its request, which gRPC-web cannot carry.
 */
export function method(name: string): Method {
  const found = METHODS[name];
  if (!found) throw new Error(`slate: this server has no ${name} method`);
  return { request: codec(found[0]), response: codec(found[1]), serverStreaming: found[2] };
}

/** A request object as wire bytes, the way proto-loader serialises one. */
export function encode(type: Codec, request: unknown): Uint8Array {
  return type.encode(type.fromObject(request as Record<string, unknown>)).finish();
}

/** Wire bytes as the object proto-loader would have produced. */
export function decode(type: Codec, bytes: Uint8Array): unknown {
  return type.toObject(type.decode(bytes), CONVERSION);
}
