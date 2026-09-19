"""What the head node's gRPC status codes mean, as Python exceptions.

`crates/slate-server/src/status.rs` is emphatic that the code is not a label:
"a gRPC client library retries on `UNAVAILABLE` and on nothing else by default,
so the code is not a label — it is an instruction to every client in the
fleet." This module is the other end of that instruction. Every exception below
answers one question — *may this be retried, and by whom* — and the hierarchy
is arranged so that a caller can answer it with `except`.

    SlateError
        Retryable            — the same call again may succeed
            Conflict         — ABORTED: retry the whole transaction
            Unavailable      — UNAVAILABLE: retry, possibly elsewhere
                NotLeader    — ... and `leader` says where
            ResourceLimit    — RESOURCE_EXHAUSTED: retry smaller
        InvalidRequest       — INVALID_ARGUMENT
        NotFound             — NOT_FOUND
        AlreadyExists        — ALREADY_EXISTS
        PermissionDenied     — PERMISSION_DENIED
        Unauthenticated      — UNAUTHENTICATED
        UnknownOutcome       — UNKNOWN: the write may have landed. Do NOT retry.
        DataLoss             — DATA_LOSS
        Cancelled / DeadlineExceeded / InternalError

`Retryable` is the load-bearing class. `UnknownOutcome` is deliberately not
under it, and that is the single most important line in the file: the server
maps a commit timeout to `UNKNOWN` precisely so that a client does not retry an
insert that may already have succeeded, and a hierarchy that put it under
`Retryable` would undo that decision on every machine that imported this
package.

# What this cannot do, and why there is no string matching here

The mapping from kernel error to status code is many-to-one. `UNAVAILABLE`
alone covers a fenced writer, a replica too stale, no replica available and a
failed object-store call; `ALREADY_EXISTS` covers both a duplicate primary key
and a unique-index violation; `NOT_FOUND` covers both an unknown table and a
missing row.

They could be recovered by matching on the message text. This package does not,
because a message is not an interface: it is prose, it is not tested for
stability anywhere in the server, and a client that branches on it breaks
silently when somebody improves the wording.

Two structural discriminators travel alongside the code instead, and both are
metadata rather than prose. A write refused because this node is not the leader
carries the leader's address in a `slate-leader` trailer, and `NotLeader` reads
it. And **every** status carries a `google.rpc.ErrorInfo` in
`grpc-status-details-bin` whose `reason` is a stable token per kernel variant —
`UNIQUE_VIOLATION`, `REPLICA_TOO_STALE`, `PREDICATE_WRITE_TOO_LARGE` — which
`SlateError.reason` exposes on every failure. The server's own guarantee is
that no two kernel errors behind one status code share a token, so the token
undoes the collapse the code performs. `_details.py` decodes it, and explains
at length why it does not import `google.rpc` to do so.
"""

from __future__ import annotations

from typing import Final, Protocol, runtime_checkable

import grpc

from ._details import CheckFailure, check_failures_of, reason_of

__all__ = [
    "AlreadyExists",
    "Cancelled",
    "CheckFailure",
    "Conflict",
    "DataLoss",
    "DeadlineExceeded",
    "InternalError",
    "InvalidRequest",
    "NotFound",
    "NotLeader",
    "PermissionDenied",
    "ResourceLimit",
    "Retryable",
    "RpcCall",
    "SlateError",
    "Unauthenticated",
    "Unavailable",
    "UnknownOutcome",
    "from_rpc_error",
]

#: The trailer the head node puts the current writer's address in.
#: `crates/slate-server/src/status.rs::LEADER_KEY`.
LEADER_KEY: Final = "slate-leader"

#: Where gRPC puts a status's `google.rpc.Status`. Named by the gRPC spec
#: rather than by this server, and binary by the `-bin` convention.
_DETAILS_KEY: Final = "grpc-status-details-bin"


class SlateError(Exception):
    """Anything the head node refused.

    `code` is the gRPC status code, kept because it is the server's own
    classification and discarding it would make this hierarchy the only account
    of what happened.
    """

    def __init__(
        self,
        message: str,
        *,
        code: grpc.StatusCode,
        trailers: dict[str, str] | None = None,
        reason: str = "",
        request_id: str = "",
        violations: list[CheckFailure] | None = None,
    ) -> None:
        super().__init__(message)
        self.message = message
        self.code = code
        self.trailers = trailers or {}
        #: The id this client sent for the call that failed, or `""`.
        #:
        #: Not the server's — the server assigns none. This is the value this
        #: client put in `slate-request-id`, kept so a caller holding a failure
        #: can go and find the line the server logged for it. It is therefore
        #: present on failures that never reached the server too: an id with no
        #: matching log line says the call did not arrive, which is itself the
        #: answer to a question somebody would otherwise spend an hour on.
        #:
        #: `""` only for a failure raised before a call was made at all.
        self.request_id = request_id
        #: The server's stable token for this failure, or `""`.
        #:
        #: Populated on **every** failure the head node reports, batched or
        #: not. The two paths carry it differently and that is the server's
        #: doing rather than this client's: a batched failure has it in the
        #: message body, because a batch's per-operation errors are data in a
        #: successful response, and a lone failure has it in
        #: `grpc-status-details-bin`. `_details.reason_of` decodes the latter.
        #:
        #: `""` when the server sent no `ErrorInfo`, when the blob did not
        #: parse, and for a failure this client raised without ever reaching
        #: the server. Compare it against the tokens in `status.rs::reason_for`
        #: rather than branching on `str(error)`.
        self.reason = reason
        #: Every `CHECK` a refused row violated, in declaration order.
        #:
        #: Empty for every failure that is not a check violation, which is
        #: almost all of them. Each entry carries the constraint's name, the
        #: column it is about and the sentence to show — so a form puts the
        #: message beside the field rather than parsing it out of `message`,
        #: which is the contract this exists to avoid inventing.
        #:
        #: The server reports *every* failing check rather than the first, so
        #: a row with three bad fields produces three entries and one round
        #: trip. `str(error)` summarises the same set for a log.
        self.violations = violations or []

    def __str__(self) -> str:
        return f"{self.code.name.lower()}: {self.message}"


class Retryable(SlateError):
    """The same request may succeed on another attempt.

    Catch this to write a retry loop that cannot accidentally include
    `UnknownOutcome`, which is the failure that must not be retried and which
    is deliberately not a subclass.
    """


class Conflict(Retryable):
    """Two writers touched the same key: ABORTED.

    `topology.md`: "Conflicts are ordinary. A unique index is enforced by two
    writers colliding on one key." This is the one error `Session.transact`
    retries, and the only one — a unique violation, an access denial or a
    fenced writer fails identically forever, and retrying them turns a clear
    error into a hang.
    """


class Unavailable(Retryable):
    """This node cannot serve it now: UNAVAILABLE.

    Covers four different kernel errors — a fenced writer, a replica too stale,
    no replica fresh enough, and object storage failing. They are one code on
    the wire and there is no structured way to tell them apart; see the module
    docstring.
    """


class NotLeader(Unavailable):
    """A write reached a node that does not hold the lease.

    `leader` is the address the `slate-leader` trailer named, when the node knew
    one. Best effort by construction: the lease may have moved again before this
    arrived, which is why the server puts it in metadata rather than promising
    it in the message.

    # Why a property and not an `__init__`

    This was the one subclass with its own constructor, forwarding each of the
    base's keywords by hand so it could set `self.leader` afterwards. A keyword
    added to the base was then silently unsupported here until something passed
    it, and the comment that used to sit below said this had happened *twice*.
    It happened a third time, on `violations`, and the third time is the
    argument: the override was never doing anything a property could not.

    `leader` is derived from `trailers`, which the base already stores and which
    nothing mutates, so reading it on demand is the same answer with no
    constructor to keep in step. `**kwargs` was the other way out and is worse —
    it forwards a typo as readily as a field.
    """

    @property
    def leader(self) -> str | None:
        """The node the `slate-leader` trailer named, or `None`."""
        return self.trailers.get(LEADER_KEY)


class ResourceLimit(Retryable):
    """The server declined to spend the memory: RESOURCE_EXHAUSTED.

    Retryable in the weak sense that a smaller request may succeed — a join's
    `build_limit` is the usual cause. Retrying the same request unchanged will
    fail the same way.
    """


class InvalidRequest(SlateError):
    """The request does not make sense against this schema: INVALID_ARGUMENT.

    Almost every refusal in `convert.rs` lands here: an unset `oneof`, an
    ordinal past the end of a table, a `HAVING` naming an ungrouped column, a
    sort on a join input, a nested loop asked for on a right outer join.
    """


class NotFound(SlateError):
    """NOT_FOUND: no such table, or no such row.

    A row the caller's policy hides is also absent, deliberately — the status
    must not distinguish "not there" from "not yours".
    """


class AlreadyExists(SlateError):
    """ALREADY_EXISTS: a primary key or a unique index key is taken."""


class PermissionDenied(SlateError):
    """PERMISSION_DENIED: the security layer refused."""


class Unauthenticated(SlateError):
    """UNAUTHENTICATED: the transport carried no usable identity.

    The head node never reads an identity from a request body, so this means
    the metadata a proxy would normally set was missing or unreadable.
    """


class UnknownOutcome(SlateError):
    """UNKNOWN: the write may or may not have landed.

    Not `Retryable`, and the class comment in `status.rs` is the reason: a
    commit timeout becomes `UNKNOWN` rather than `DEADLINE_EXCEEDED` because
    "`DEADLINE_EXCEEDED` invites a retry, and a retried insert that already
    succeeded is a duplicate row". Read before writing again.
    """


class DataLoss(SlateError):
    """DATA_LOSS: a stored key or index entry did not decode.

    The index and the table disagreeing. Not a bad request.
    """


class DeadlineExceeded(SlateError):
    """DEADLINE_EXCEEDED: the client's own deadline ran out.

    Not `Retryable`: for a read it would be, for a write it has the same
    may-have-landed problem `UnknownOutcome` describes, and this class cannot
    tell which it was. A caller that knows its call was a read can retry it.
    """


class Cancelled(SlateError):
    """CANCELLED: the call was cancelled, usually by this process."""


class InternalError(SlateError):
    """INTERNAL, or a code this mapping does not know.

    The wildcard is deliberately the code no client retries, matching the
    server's own wildcard: an error nobody has classified is one nobody has
    established is safe to repeat.
    """


# Ordered as a plain dict rather than a match statement so that the whole
# mapping is one readable object and can be asserted over in a test — see
# `tests/test_errors.py`, which requires every code `grpc` defines to have an
# entry rather than falling through to the wildcard by accident.
_BY_CODE: Final[dict[grpc.StatusCode, type[SlateError]]] = {
    grpc.StatusCode.INVALID_ARGUMENT: InvalidRequest,
    grpc.StatusCode.NOT_FOUND: NotFound,
    grpc.StatusCode.ALREADY_EXISTS: AlreadyExists,
    grpc.StatusCode.PERMISSION_DENIED: PermissionDenied,
    grpc.StatusCode.UNAUTHENTICATED: Unauthenticated,
    grpc.StatusCode.ABORTED: Conflict,
    grpc.StatusCode.UNAVAILABLE: Unavailable,
    grpc.StatusCode.RESOURCE_EXHAUSTED: ResourceLimit,
    grpc.StatusCode.UNKNOWN: UnknownOutcome,
    grpc.StatusCode.DATA_LOSS: DataLoss,
    grpc.StatusCode.DEADLINE_EXCEEDED: DeadlineExceeded,
    grpc.StatusCode.CANCELLED: Cancelled,
    grpc.StatusCode.INTERNAL: InternalError,
    # The head node emits none of these today. They are mapped anyway because
    # the alternative is that a proxy, a load balancer or a future version of
    # the server produces one and the client reports it as INTERNAL, which
    # says "the server is broken" about a request that was merely refused.
    grpc.StatusCode.FAILED_PRECONDITION: InvalidRequest,
    grpc.StatusCode.OUT_OF_RANGE: InvalidRequest,
    grpc.StatusCode.UNIMPLEMENTED: InvalidRequest,
    grpc.StatusCode.OK: InternalError,
}


def _trailers(error: grpc.RpcError | RpcCall) -> dict[str, str]:
    """The call's trailing metadata, as a dict of the text-valued entries.

    Binary metadata (a `-bin` key) is dropped rather than decoded: a `bytes`
    hiding in a `dict[str, str]` is the kind of thing that only fails once it
    reaches a log line.

    The head node does send one binary entry — `grpc-status-details-bin`, the
    rich-error blob. It is read by `_reason`, which goes to the metadata
    directly rather than through this, precisely so that this can keep
    promising `str` values.
    """
    out: dict[str, str] = {}
    # `trailing_metadata` exists on a `Call`, and every `RpcError` grpc raises
    # from a unary or streaming call is also a `Call`. Guarded anyway: the
    # exception type does not promise it, and a client that crashed while
    # building an error message would hide the error it was reporting.
    getter = getattr(error, "trailing_metadata", None)
    if getter is None:
        return out
    try:
        metadata = getter()
    except Exception:  # pragma: no cover - defensive, see above
        return out
    for entry in metadata or ():
        key, value = entry[0], entry[1]
        if isinstance(value, str):
            out[key] = value
    return out


def from_batch_error(code: int, message: str, reason: str) -> SlateError:
    """The exception a batch's per-operation failure becomes.

    An independent batch reports each failure *as data*, inside a successful
    response, so the code and message arrive in a message body rather than in
    trailers. This turns them back into the same exception type the same
    operation would have raised had it been sent alone — so a caller writes one
    `except NotFound` whether or not the write was batched.

    `reason` is carried on the exception rather than in `trailers`, because
    there are no trailers: the whole point of an independent batch is that the
    request succeeded and the operation did not.
    """
    status = _BY_VALUE.get(code, grpc.StatusCode.UNKNOWN)
    kind: type[SlateError] = _BY_CODE.get(status, InternalError)
    return kind(message or status.name, code=status, reason=reason)


#: gRPC's numeric codes, which arrive as an `int32` in a `BatchError`.
#:
#: `grpc.StatusCode` is an enum of `(number, name)` pairs, so this is built
#: from it rather than typed out — a hand-written table is a second place for
#: the numbers to be wrong, and they are not this repository's numbers to
#: choose.
_BY_VALUE: Final[dict[int, grpc.StatusCode]] = {
    status.value[0]: status for status in grpc.StatusCode
}


@runtime_checkable
class RpcCall(Protocol):
    """What this client reads off a failed gRPC call.

    gRPC raises an object that is *both* a `grpc.RpcError` and a `grpc.Call`,
    and Python's type system has no way to say "both" — so `grpc.RpcError`
    alone, which is what the raise site is annotated with, declares none of
    `code`, `details` or `trailing_metadata`. Every client reading a failure
    therefore reads attributes its declared type does not have.

    mypy never said so, because `grpc.*` is under `ignore_missing_imports` and
    everything from it was `Any`. `ty` resolves the package and says it out
    loud, which is how this came to be written down.

    `runtime_checkable` on purpose: `isinstance` against such a Protocol checks
    that the *attributes exist* and nothing about their signatures, which is
    exactly what the two `hasattr` calls this replaced were doing. The
    duck-typing is deliberate — `tests/test_details.py` passes a hand-rolled
    fake that is not a `grpc.Call` — so the behaviour is unchanged and only the
    declaration is new. `isinstance(error, grpc.Call)` would have been the
    tempting fix and is a different one: a bare `grpc.RpcError` is not a
    `grpc.Call`, and neither is that fake.
    """

    def code(self) -> grpc.StatusCode: ...

    def details(self) -> str: ...


def from_rpc_error(error: grpc.RpcError | RpcCall, request_id: str = "") -> SlateError:
    """The exception a gRPC failure becomes.

    `request_id` is the id this client sent for the failed call. It is passed
    in rather than read off the error because gRPC gives a client no way back
    to the metadata it *sent* — `RpcError` carries the trailers that came back
    and nothing of what went out — so the only place it exists is the call
    site that generated it.
    """
    # `code()` and `details()` come from `grpc.Call`, which every RpcError
    # raised by a call also implements. See `RpcCall` for why this is an
    # `isinstance` against a Protocol rather than two `hasattr` calls or an
    # `isinstance` against `grpc.Call` itself.
    #
    # One deliberate difference from the two `hasattr` calls this replaced:
    # they were independent, so an object carrying `code` and not `details`
    # got its code and a `str(error)` message. This takes them together,
    # because `grpc.Call` provides them together and an object with one and
    # not the other is not a thing gRPC produces. Said out loud because it is
    # a behaviour change, however narrow.
    if isinstance(error, RpcCall):
        code = error.code()
        message = error.details()
    else:
        code = grpc.StatusCode.UNKNOWN
        message = str(error)
    trailers = _trailers(error)

    kind: type[SlateError] = _BY_CODE.get(code, InternalError)
    # A redirect is an UNAVAILABLE that names a different node, and telling it
    # apart from "storage is down" is the difference between retrying elsewhere
    # and retrying here. Left as a trailer check rather than moved onto the
    # reason token below, though NOT_LEADER is one: `slate-leader` predates the
    # details blob, a client that can read the trailer but not the blob still
    # follows the redirect, and a working discriminator is not worth churning.
    if kind is Unavailable and LEADER_KEY in trailers:
        kind = NotLeader

    return kind(
        message or code.name,
        code=code,
        trailers=trailers,
        reason=_reason(error),
        request_id=request_id,
        violations=_check_failures(error),
    )


def _reason(error: grpc.RpcError | RpcCall) -> str:
    """The stable token in the call's `grpc-status-details-bin`, or `""`.

    Goes to the trailing metadata directly because `_trailers` drops binary
    entries by design, and guards the same way and for the same reason: a
    client that raised while building an error object would hide the server's
    failure behind its own.
    """
    getter = getattr(error, "trailing_metadata", None)
    if getter is None:
        return ""
    try:
        metadata = getter()
    except Exception:  # pragma: no cover - defensive, as in `_trailers`
        return ""
    for entry in metadata or ():
        if entry[0] == _DETAILS_KEY and isinstance(entry[1], bytes):
            return reason_of(entry[1])
    return ""


def _check_failures(error: object) -> list[CheckFailure]:
    """Every check a refused write violated, from the same blob.

    A second pass over the trailers rather than one that returns both, so the
    common failure — which is not a check violation — parses nothing extra
    and the reason extraction above stays the shape every caller already
    knows.
    """
    getter = getattr(error, "trailing_metadata", None)
    if getter is None:
        return []
    try:
        metadata = getter()
    except Exception:  # pragma: no cover - defensive, as in `_trailers`
        return []
    for entry in metadata or ():
        if entry[0] == _DETAILS_KEY and isinstance(entry[1], bytes):
            return check_failures_of(entry[1])
    return []
