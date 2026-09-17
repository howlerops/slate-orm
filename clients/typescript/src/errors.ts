import { status as GrpcStatus, type ServiceError } from "@grpc/grpc-js";

import { DETAILS_KEY, reasonOf } from "./details.js";

/** The trailer a redirect carries, naming the node to try instead. */
export const LEADER_KEY = "slate-leader";

/**
 * Which sort of failure a [SlateError] is.
 *
 * A closed set rather than the gRPC code, because two codes can mean the same
 * thing to a caller and one code can mean two: UNAVAILABLE with a
 * `slate-leader` trailer is a redirect and without one is an outage, and the
 * difference decides whether to retry here or elsewhere.
 */
export type Kind =
  | "internal"
  | "invalid-request"
  | "not-found"
  | "already-exists"
  | "permission-denied"
  | "unauthenticated"
  | "conflict"
  | "unavailable"
  | "not-leader"
  | "resource-limit"
  | "unknown-outcome"
  | "data-loss"
  | "deadline-exceeded"
  | "cancelled";

const BY_CODE: Partial<Record<GrpcStatus, Kind>> = {
  [GrpcStatus.INVALID_ARGUMENT]: "invalid-request",
  [GrpcStatus.NOT_FOUND]: "not-found",
  [GrpcStatus.ALREADY_EXISTS]: "already-exists",
  [GrpcStatus.PERMISSION_DENIED]: "permission-denied",
  [GrpcStatus.UNAUTHENTICATED]: "unauthenticated",
  [GrpcStatus.ABORTED]: "conflict",
  [GrpcStatus.UNAVAILABLE]: "unavailable",
  [GrpcStatus.RESOURCE_EXHAUSTED]: "resource-limit",
  [GrpcStatus.UNKNOWN]: "unknown-outcome",
  [GrpcStatus.DATA_LOSS]: "data-loss",
  [GrpcStatus.DEADLINE_EXCEEDED]: "deadline-exceeded",
  [GrpcStatus.CANCELLED]: "cancelled",
  [GrpcStatus.INTERNAL]: "internal",
  [GrpcStatus.FAILED_PRECONDITION]: "invalid-request",
  [GrpcStatus.OUT_OF_RANGE]: "invalid-request",
  [GrpcStatus.UNIMPLEMENTED]: "invalid-request",
};

const RETRYABLE = new Set<Kind>([
  "conflict",
  "unavailable",
  "not-leader",
  "resource-limit",
]);

/** A failure the head node reported. */
export class SlateError extends Error {
  /** What sort of failure this is. */
  readonly kind: Kind;
  /** The gRPC code it arrived as. */
  readonly code: GrpcStatus;
  /** The node to try instead, when this is a redirect. */
  readonly leader: string | undefined;
  /** The call's text-valued trailing metadata. */
  readonly trailers: Readonly<Record<string, string>>;
  /**
   * The server's stable token for this failure, or `""`.
   *
   * Populated on **every** failure the head node reports, batched or not. The
   * two paths carry it differently, which is the server's doing rather than
   * this client's: a batched failure has it in the message body, because a
   * batch's per-operation errors are data inside a successful response, and a
   * lone failure has it in `grpc-status-details-bin` as a `google.rpc.ErrorInfo`.
   *
   * The tokens are one per kernel variant — `UNIQUE_VIOLATION`,
   * `REPLICA_TOO_STALE`, `PREDICATE_WRITE_TOO_LARGE` — and the server
   * guarantees no two errors behind one status code share one, which is what
   * makes this usable where `code` is not. Switch on it rather than on
   * `message`, which is prose and carries no stability promise.
   *
   * `""` when the server sent no `ErrorInfo`, and for a failure raised without
   * reaching the server.
   */
  readonly reason: string;

  constructor(
    kind: Kind,
    message: string,
    code: GrpcStatus,
    trailers: Record<string, string>,
    leader?: string,
    reason = "",
  ) {
    super(leader ? `${kind}: ${message} (leader ${leader})` : `${kind}: ${message}`);
    this.name = "SlateError";
    this.reason = reason;
    this.kind = kind;
    this.code = code;
    this.trailers = trailers;
    this.leader = leader;
  }

  /**
   * Whether trying again could succeed.
   *
   * A redirect is retryable *elsewhere*, which is what `leader` is for. An
   * unknown outcome is not retryable at all: the write may have landed, and
   * retrying is how one write becomes two. A deadline has the same ambiguity.
   */
  get retryable(): boolean {
    return RETRYABLE.has(this.kind);
  }
}

/** Whether an error is a slate error of this kind. */
export function isKind(error: unknown, kind: Kind): boolean {
  return error instanceof SlateError && error.kind === kind;
}

/** The error a gRPC failure becomes. */
/**
 * The `SlateError` a batch's per-operation failure becomes.
 *
 * An independent batch reports each failure as *data*, inside a successful
 * response, so the code and message arrive in a message body rather than in
 * trailers. This turns them back into the same class a lone call throws, so a
 * caller writes one `instanceof SlateError` check whether or not the write was
 * batched.
 */
export function fromBatchError(failed: {
  code?: number;
  message?: string;
  reason?: string;
}): SlateError {
  const code = (failed.code ?? 2) as GrpcStatus;
  const kind = BY_CODE[code] ?? "internal";
  return new SlateError(kind, failed.message ?? "", code, {}, undefined, failed.reason ?? "");
}

export function fromServiceError(error: ServiceError): SlateError {
  const trailers: Record<string, string> = {};
  const metadata = error.metadata?.getMap?.() ?? {};
  for (const [key, value] of Object.entries(metadata)) {
    // Binary entries are dropped rather than decoded: a Buffer hiding in a
    // Record<string, string> only fails once it reaches a log line. The server
    // does send one, `grpc-status-details-bin`, and `reason` below carries what
    // this client reads out of it, so keeping this `string`-valued loses
    // nothing.
    if (key.endsWith("-bin")) continue;
    if (typeof value === "string") trailers[key] = value;
  }

  let kind = BY_CODE[error.code] ?? "internal";
  let leader: string | undefined;
  // Left as a trailer check rather than moved onto the reason token below,
  // though NOT_LEADER is one: `slate-leader` predates the details blob, a
  // client that reads the trailer but not the blob still follows the redirect,
  // and a working discriminator is not worth churning.
  if (kind === "unavailable" && trailers[LEADER_KEY]) {
    kind = "not-leader";
    leader = trailers[LEADER_KEY];
  }
  return new SlateError(
    kind,
    error.details || error.message,
    error.code,
    trailers,
    leader,
    reasonFromMetadata(error),
  );
}

/**
 * The stable token in the call's `grpc-status-details-bin`, or `""`.
 *
 * Reaches for the binary entry directly because the trailer map above drops
 * binary by design. `getMap` coerces, so this uses `get`, which returns the
 * `Buffer` for a `-bin` key untouched.
 */
function reasonFromMetadata(error: ServiceError): string {
  const values = error.metadata?.get?.(DETAILS_KEY) ?? [];
  for (const value of values) {
    if (typeof value !== "string") return reasonOf(value);
  }
  return "";
}
