/**
 * The native gRPC transport, over `@grpc/grpc-js`: what `Client.connect` has
 * always used, moved behind the `Transport` seam and otherwise unchanged.
 *
 * Kept out of `client.ts` so that the web entry point can load the client
 * without loading this. `@grpc/grpc-js` imports `node:http2` when it is
 * loaded, which an edge runtime does not have; `docs/edge-client.md` §2.
 * Importing this module installs it as `Client.connect`'s transport, which is
 * what the Node entry point, `index.ts`, does first.
 */
import path from "node:path";
import * as grpc from "@grpc/grpc-js";
import * as protoLoader from "@grpc/proto-loader";

import { installNodeTransport } from "./client.js";
import { directoryOf, findUpContaining } from "./paths.js";
import { CONVERSION, type Metadata, type MessageStream, type Transport } from "./transport.js";

/**
 * Where the bundled `.proto` files are, found by looking for the file rather
 * than counting `..` segments: this module runs from `src/` in the repository,
 * `dist/` once built, and a package root once installed, and a fixed depth is
 * correct in exactly one of those.
 */
const PROTO_ROOT = path.join(
  findUpContaining(
    directoryOf(import.meta.url),
    [path.join("proto", "slate", "v1", "records.proto")],
    "bundled proto directory",
  ),
  "proto",
);

// `longs: String` rather than Number: a u64 primary key above 2^53 would
// silently lose precision, and a primary key is where that shows up latest.
// The client turns them into bigint at the edge. The conversion half is
// `transport.ts`'s, so the web transport decodes to the same objects.
/** @internal Exported for `test/descriptor.test.ts`, which compares the two transports' decoding. */
export const LOADER_OPTIONS: protoLoader.Options = {
  keepCase: false,
  ...(CONVERSION as protoLoader.Options),
  includeDirs: [PROTO_ROOT],
};

type RawClient = grpc.Client & Record<string, Function>;

let cachedService: grpc.ServiceClientConstructor | undefined;

function service(): grpc.ServiceClientConstructor {
  if (!cachedService) {
    const definition = protoLoader.loadSync("slate/v1/records.proto", LOADER_OPTIONS);
    const loaded = grpc.loadPackageDefinition(definition) as unknown as {
      slate: { v1: { Records: grpc.ServiceClientConstructor } };
    };
    cachedService = loaded.slate.v1.Records;
  }
  return cachedService;
}

function grpcMetadata(metadata: Metadata): grpc.Metadata {
  const md = new grpc.Metadata();
  for (const [key, value] of Object.entries(metadata)) md.set(key, value);
  return md;
}

export class GrpcJsTransport implements Transport {
  readonly #raw: RawClient;

  constructor(target: string, credentials: grpc.ChannelCredentials, options: grpc.ChannelOptions) {
    const Records = service();
    this.#raw = new Records(target, credentials, options) as RawClient;
  }

  unary(method: string, request: unknown, metadata: Metadata, deadline?: number): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const fn = this.#raw[method];
      if (!fn) {
        reject(new Error(`slate: this server has no ${method} method`));
        return;
      }
      // An absolute instant, because that is what grpc-js wants: a relative
      // number here would be read as a Unix timestamp in 1970 and expire every
      // call immediately.
      const options: grpc.CallOptions = deadline === undefined ? {} : { deadline };
      fn.call(
        this.#raw,
        request,
        grpcMetadata(metadata),
        options,
        (error: grpc.ServiceError | null, response: unknown) => {
          if (error) reject(error);
          else resolve(response);
        },
      );
    });
  }

  stream(method: string, request: unknown, metadata: Metadata, deadline?: number): MessageStream {
    const fn = this.#raw[method];
    if (!fn) throw new Error(`slate: this server has no ${method} method`);
    const options: grpc.CallOptions = deadline === undefined ? {} : { deadline };
    return fn.call(this.#raw, request, grpcMetadata(metadata), options) as grpc.ClientReadableStream<unknown>;
  }

  close(): void {
    this.#raw.close();
  }
}

installNodeTransport(
  (target, credentials, options) =>
    new GrpcJsTransport(target, credentials ?? grpc.credentials.createInsecure(), options ?? {}),
);
