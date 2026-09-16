import { status as GrpcStatus, type ServiceError } from "@grpc/grpc-js";

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
   * Set for a failure that came back inside a **batch**, where the server puts
   * it in the message body. Empty for every other failure, and the asymmetry
   * is real rather than an oversight: a lone call carries its token in
   * `grpc-status-details-bin`, a protobuf blob this client does not decode —
   * see the `trailers` comment, which drops binary entries. So a batched
   * failure currently says more about itself than the same failure alone.
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
    // Binary entries are dropped rather than decoded: nothing this server
    // sends is binary, and a Buffer hiding in a Record<string, string> only
    // fails once it reaches a log line.
    if (key.endsWith("-bin")) continue;
    if (typeof value === "string") trailers[key] = value;
  }

  let kind = BY_CODE[error.code] ?? "internal";
  let leader: string | undefined;
  // The one structural refinement available.
  if (kind === "unavailable" && trailers[LEADER_KEY]) {
    kind = "not-leader";
    leader = trailers[LEADER_KEY];
  }
  return new SlateError(kind, error.details || error.message, error.code, trailers, leader);
}
