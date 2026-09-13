import { status as GrpcStatus } from "@grpc/grpc-js";
/** The trailer a redirect carries, naming the node to try instead. */
export const LEADER_KEY = "slate-leader";
const BY_CODE = {
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
const RETRYABLE = new Set([
    "conflict",
    "unavailable",
    "not-leader",
    "resource-limit",
]);
/** A failure the head node reported. */
export class SlateError extends Error {
    /** What sort of failure this is. */
    kind;
    /** The gRPC code it arrived as. */
    code;
    /** The node to try instead, when this is a redirect. */
    leader;
    /** The call's text-valued trailing metadata. */
    trailers;
    constructor(kind, message, code, trailers, leader) {
        super(leader ? `${kind}: ${message} (leader ${leader})` : `${kind}: ${message}`);
        this.name = "SlateError";
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
    get retryable() {
        return RETRYABLE.has(this.kind);
    }
}
/** Whether an error is a slate error of this kind. */
export function isKind(error, kind) {
    return error instanceof SlateError && error.kind === kind;
}
/** The error a gRPC failure becomes. */
export function fromServiceError(error) {
    const trailers = {};
    const metadata = error.metadata?.getMap?.() ?? {};
    for (const [key, value] of Object.entries(metadata)) {
        // Binary entries are dropped rather than decoded: nothing this server
        // sends is binary, and a Buffer hiding in a Record<string, string> only
        // fails once it reaches a log line.
        if (key.endsWith("-bin"))
            continue;
        if (typeof value === "string")
            trailers[key] = value;
    }
    let kind = BY_CODE[error.code] ?? "internal";
    let leader;
    // The one structural refinement available.
    if (kind === "unavailable" && trailers[LEADER_KEY]) {
        kind = "not-leader";
        leader = trailers[LEADER_KEY];
    }
    return new SlateError(kind, error.details || error.message, error.code, trailers, leader);
}
