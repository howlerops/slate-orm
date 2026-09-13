import { status as GrpcStatus, type ServiceError } from "@grpc/grpc-js";
/** The trailer a redirect carries, naming the node to try instead. */
export declare const LEADER_KEY = "slate-leader";
/**
 * Which sort of failure a [SlateError] is.
 *
 * A closed set rather than the gRPC code, because two codes can mean the same
 * thing to a caller and one code can mean two: UNAVAILABLE with a
 * `slate-leader` trailer is a redirect and without one is an outage, and the
 * difference decides whether to retry here or elsewhere.
 */
export type Kind = "internal" | "invalid-request" | "not-found" | "already-exists" | "permission-denied" | "unauthenticated" | "conflict" | "unavailable" | "not-leader" | "resource-limit" | "unknown-outcome" | "data-loss" | "deadline-exceeded" | "cancelled";
/** A failure the head node reported. */
export declare class SlateError extends Error {
    /** What sort of failure this is. */
    readonly kind: Kind;
    /** The gRPC code it arrived as. */
    readonly code: GrpcStatus;
    /** The node to try instead, when this is a redirect. */
    readonly leader: string | undefined;
    /** The call's text-valued trailing metadata. */
    readonly trailers: Readonly<Record<string, string>>;
    constructor(kind: Kind, message: string, code: GrpcStatus, trailers: Record<string, string>, leader?: string);
    /**
     * Whether trying again could succeed.
     *
     * A redirect is retryable *elsewhere*, which is what `leader` is for. An
     * unknown outcome is not retryable at all: the write may have landed, and
     * retrying is how one write becomes two. A deadline has the same ambiguity.
     */
    get retryable(): boolean;
}
/** Whether an error is a slate error of this kind. */
export declare function isKind(error: unknown, kind: Kind): boolean;
/** The error a gRPC failure becomes. */
export declare function fromServiceError(error: ServiceError): SlateError;
