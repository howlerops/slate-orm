/**
 * The stable reason token out of a status's `grpc-status-details-bin`.
 *
 * The head node puts a `google.rpc.ErrorInfo` in every status's details —
 * `crates/slate-server/src/status.rs` — because the status code alone is
 * many-to-one: `UNAVAILABLE` covers a fenced writer, a stale replica, no
 * replica and a failed object-store call, and `RESOURCE_EXHAUSTED` covers a
 * join build, a group count, a distinct count, a sort, and a predicate write
 * that matched more rows than it may hand back. The token is how a caller
 * tells them apart without matching on prose.
 *
 * The two `.proto` files this reads have been vendored in `proto/google/rpc/`
 * since the server grew rich errors, and until now nothing loaded them. They
 * are already listed in `files`, so they ship with the package and this adds
 * nothing to what is published.
 */

import * as path from "node:path";
// A default import, not `import * as`. protobufjs is CommonJS, and under
// NodeNext a namespace import of a CommonJS module gives the module namespace
// rather than `module.exports` — so `protobuf.Root` is `undefined` and the
// first call fails with "Root is not a constructor". Caught by the conformance
// runner rather than by the build: the namespace shape type-checks, and the
// decoder swallows its own failures by design, so the only symptom was this
// client reporting an empty token while the other two reported a real one.
import protobuf from "protobufjs";

import { directoryOf, findUpContaining } from "./paths.js";

/** The `Any.type_url` the head node packs its `ErrorInfo` under. */
export const ERROR_INFO_URL = "type.googleapis.com/google.rpc.ErrorInfo";

/** Where gRPC puts a status's `google.rpc.Status`, per the gRPC spec. */
export const DETAILS_KEY = "grpc-status-details-bin";

type StatusMessage = {
  details?: ReadonlyArray<{ type_url?: string; value?: Uint8Array }>;
};

let cached: { status: protobuf.Type; info: protobuf.Type } | undefined;
/**
 * Loaded once, on the first failure rather than at import.
 *
 * A client that read two `.proto` files off disk to import is a client that
 * pays for rich errors on a run that never has one, and `client.ts` loads its
 * own descriptor the same way and for the same reason.
 */
function types(): { status: protobuf.Type; info: protobuf.Type } | undefined {
  if (!cached) {
    try {
      const base = protoRoot();
      const root = new protobuf.Root();
      // `status.proto` imports `google/protobuf/any.proto`, which protobufjs
      // resolves from its own bundled copy; every other import resolves under
      // the vendored tree.
      root.resolvePath = (origin, target) =>
        target.startsWith("google/protobuf/")
          ? protobuf.Root.prototype.resolvePath(origin, target)
          : path.join(base, target);
      root.loadSync(["google/rpc/status.proto", "google/rpc/error_details.proto"], {
        keepCase: true,
      });
      cached = {
        status: root.lookupType("google.rpc.Status"),
        info: root.lookupType("google.rpc.ErrorInfo"),
      };
    } catch {
      // Losing the token is a degradation; throwing here would replace the
      // server's failure with this client's, which is not.
      return undefined;
    }
  }
  return cached;
}

let root: string | undefined;
/**
 * Where the vendored protos are.
 *
 * Found by looking for the file rather than counting `..` segments, for the
 * reason `client.ts` gives about its own `PROTO_ROOT`: this module runs from
 * `src/` in the repository, from `dist/` once built and from a package root
 * once installed, and a fixed depth is right in exactly one of the three. The
 * helpers live in `paths.ts` rather than in `client.ts` so that reusing them
 * here does not make `client.ts` and `errors.ts` import each other.
 */
function protoRoot(): string {
  if (root === undefined) {
    root = path.join(
      findUpContaining(
        directoryOf(import.meta.url),
        [path.join("proto", "google", "rpc", "error_details.proto")],
        "bundled proto directory",
      ),
      "proto",
    );
  }
  return root;
}

/**
 * The `ErrorInfo.reason` in a `grpc-status-details-bin` blob, or `""`.
 *
 * `""` for a blob carrying no `ErrorInfo` and for one that does not parse.
 *
 * `ErrorInfo.metadata` is deliberately not returned. The server fills it with
 * a variant's own payload — `index` and `table` on a unique violation, `limit`
 * on a predicate write that matched too many — and exposing it means promising
 * something about keys that differ per variant. The token alone is what lets
 * `errors.ts` branch below a status code, and it is what three clients can
 * agree on.
 */
export function reasonOf(blob: Uint8Array): string {
  const loaded = types();
  if (!loaded) return "";
  try {
    const decoded = loaded.status.decode(blob) as unknown as StatusMessage;
    for (const detail of decoded.details ?? []) {
      if (detail.type_url !== ERROR_INFO_URL || !detail.value) continue;
      const info = loaded.info.decode(detail.value) as unknown as { reason?: string };
      return info.reason ?? "";
    }
  } catch {
    return "";
  }
  return "";
}
