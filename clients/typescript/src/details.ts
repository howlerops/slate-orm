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
 * `ErrorInfo.metadata` is not returned *here*. The token alone is what lets
 * `errors.ts` branch below a status code, and it is what three clients can
 * agree on. See {@link checkFailuresOf} for the one part of that map this
 * client reads.
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

/** One `CHECK` a refused row violated. */
export interface CheckFailure {
  /** The constraint's name, as the schema declares it. */
  readonly check: string;
  /**
   * The column it is about, or `""` for a check spanning several.
   *
   * `""` rather than `undefined`, so that a caller rendering it into a form
   * writes `failure.column` and not a null check: a check over two columns has
   * no single field to hang the message on, and naming either would put the
   * sentence beside the wrong input.
   */
  readonly column: string;
  /** The sentence to show, or `""` where the schema wrote none. */
  readonly message: string;
}

type ErrorInfoMessage = { reason?: string; metadata?: Record<string, string> };

/**
 * Every `CHECK` a refused write violated, in declaration order.
 *
 * Empty for any failure that is not a check violation — which is almost all of
 * them — and for a blob that does not parse, for the reason {@link reasonOf}
 * gives about never throwing out of an error path.
 *
 * This reads `ErrorInfo.metadata`, which `reasonOf`'s comment used to say was
 * deliberately never exposed. That objection was about keys that vary per
 * variant, and it still holds: the check-violation keys are the one *specified*
 * shape — `violations` is a count and `check.N`, `column.N`, `message.N` are
 * indexed from zero — so this reads them into typed values and hands nobody the
 * raw dictionary. A key this client has no contract for still reaches no
 * caller.
 *
 * Counts up from `violations` rather than walking the map for `check.*`,
 * because the metadata is string-keyed: `check.10` sorts between `check.1` and
 * `check.2`, so a map walk is right for nine failures and wrong for eleven.
 *
 * Returns nothing rather than a prefix when the count and the keys disagree. A
 * caller shown two failures for a row that broke three fixes two fields,
 * resubmits and is refused again — the round-trip-per-field behaviour this
 * exists to remove.
 */
export function checkFailuresOf(blob: Uint8Array): CheckFailure[] {
  const loaded = types();
  if (!loaded) return [];
  try {
    const decoded = loaded.status.decode(blob) as unknown as StatusMessage;
    for (const detail of decoded.details ?? []) {
      if (detail.type_url !== ERROR_INFO_URL || !detail.value) continue;
      const info = loaded.info.decode(detail.value) as unknown as ErrorInfoMessage;
      if (info.reason !== "CHECK_VIOLATION") return [];
      const data = info.metadata ?? {};
      const total = Number.parseInt(data["violations"] ?? "", 10);
      if (!Number.isInteger(total)) {
        // A server old enough to send the unindexed pair and no count. One
        // failure is the honest reading of what it said.
        const name = data["check"];
        if (!name) return [];
        return [
          { check: name, column: data["column"] ?? "", message: data["message"] ?? "" },
        ];
      }
      const out: CheckFailure[] = [];
      for (let at = 0; at < total; at += 1) {
        const name = data[`check.${at}`];
        if (!name) return [];
        out.push({
          check: name,
          column: data[`column.${at}`] ?? "",
          message: data[`message.${at}`] ?? "",
        });
      }
      return out;
    }
  } catch {
    return [];
  }
  return [];
}
