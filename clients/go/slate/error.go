package slate

import (
	"errors"
	"fmt"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
)

// LeaderKey is the trailer a redirect carries, naming the node to try instead.
const LeaderKey = "slate-leader"

// Kind is which sort of failure an [Error] is.
//
// A closed set rather than the gRPC code, because two different codes can mean
// the same thing to a caller and one code can mean two — UNAVAILABLE with a
// `slate-leader` trailer is a redirect and without one is an outage, and the
// difference decides whether to retry here or elsewhere.
type Kind int

// The kinds of failure a head node reports.
const (
	// KindInternal is the default for anything unrecognised.
	KindInternal Kind = iota
	// KindInvalidRequest is a request this server will never accept as sent.
	KindInvalidRequest
	// KindNotFound is a missing row, table or transaction.
	KindNotFound
	// KindAlreadyExists is a key that is taken.
	KindAlreadyExists
	// KindPermissionDenied is an identity that is not allowed this.
	KindPermissionDenied
	// KindUnauthenticated is an identity the server could not establish.
	KindUnauthenticated
	// KindConflict is a write that lost a race. Retryable.
	KindConflict
	// KindUnavailable is a node that cannot serve right now. Retryable.
	KindUnavailable
	// KindNotLeader is an Unavailable naming another node. Retryable there.
	KindNotLeader
	// KindResourceLimit is a ceiling the request passed. Retryable smaller.
	KindResourceLimit
	// KindUnknownOutcome is a write that may or may not have landed.
	//
	// Deliberately not retryable: retrying a write that may already have
	// happened is how one write becomes two.
	KindUnknownOutcome
	// KindDataLoss is corruption the server detected.
	KindDataLoss
	// KindDeadlineExceeded is a call that ran out of time. See the note on
	// KindUnknownOutcome: a write that timed out has the same ambiguity.
	KindDeadlineExceeded
	// KindCancelled is a call the caller stopped.
	KindCancelled
)

// String names the kind, for messages.
func (k Kind) String() string {
	switch k {
	case KindInvalidRequest:
		return "invalid request"
	case KindNotFound:
		return "not found"
	case KindAlreadyExists:
		return "already exists"
	case KindPermissionDenied:
		return "permission denied"
	case KindUnauthenticated:
		return "unauthenticated"
	case KindConflict:
		return "conflict"
	case KindUnavailable:
		return "unavailable"
	case KindNotLeader:
		return "not leader"
	case KindResourceLimit:
		return "resource limit"
	case KindUnknownOutcome:
		return "unknown outcome"
	case KindDataLoss:
		return "data loss"
	case KindDeadlineExceeded:
		return "deadline exceeded"
	case KindCancelled:
		return "cancelled"
	default:
		return "internal"
	}
}

// Error is a failure the head node reported.
type Error struct {
	// Kind is what sort of failure this is.
	Kind Kind
	// Message is the server's own text.
	Message string
	// Code is the gRPC code it arrived as.
	Code codes.Code
	// Leader is the node to try instead, when this is a redirect.
	Leader string
	// Trailers are the call's text-valued trailing metadata.
	//
	// Binary entries are dropped rather than decoded: nothing this server
	// sends is binary, and a []byte hiding in a map[string]string is the kind
	// of thing that only fails once it reaches a log line.
	Trailers map[string]string
}

func (e *Error) Error() string {
	if e.Leader != "" {
		return fmt.Sprintf("slate: %s: %s (leader %s)", e.Kind, e.Message, e.Leader)
	}
	return fmt.Sprintf("slate: %s: %s", e.Kind, e.Message)
}

// Retryable reports whether trying again could succeed.
//
// A redirect is retryable *elsewhere*, which is why [Error.Leader] is there;
// an unknown outcome is not retryable at all, because the write may have
// landed.
func (e *Error) Retryable() bool {
	switch e.Kind {
	case KindConflict, KindUnavailable, KindNotLeader, KindResourceLimit:
		return true
	default:
		return false
	}
}

// IsKind reports whether err is a slate error of this kind.
func IsKind(err error, kind Kind) bool {
	var e *Error
	return errors.As(err, &e) && e.Kind == kind
}

var byCode = map[codes.Code]Kind{
	codes.InvalidArgument:    KindInvalidRequest,
	codes.NotFound:           KindNotFound,
	codes.AlreadyExists:      KindAlreadyExists,
	codes.PermissionDenied:   KindPermissionDenied,
	codes.Unauthenticated:    KindUnauthenticated,
	codes.Aborted:            KindConflict,
	codes.Unavailable:        KindUnavailable,
	codes.ResourceExhausted:  KindResourceLimit,
	codes.Unknown:            KindUnknownOutcome,
	codes.DataLoss:           KindDataLoss,
	codes.DeadlineExceeded:   KindDeadlineExceeded,
	codes.Canceled:           KindCancelled,
	codes.Internal:           KindInternal,
	codes.FailedPrecondition: KindInvalidRequest,
	codes.OutOfRange:         KindInvalidRequest,
	codes.Unimplemented:      KindInvalidRequest,
}

// fromRPC turns a gRPC failure into an [Error].
//
// Returns nil for a nil error so call sites can wrap unconditionally.
func fromRPC(err error) error {
	if err == nil {
		return nil
	}
	st, ok := status.FromError(err)
	if !ok {
		return &Error{Kind: KindInternal, Message: err.Error(), Code: codes.Unknown}
	}
	kind, known := byCode[st.Code()]
	if !known {
		kind = KindInternal
	}
	out := &Error{Kind: kind, Message: st.Message(), Code: st.Code()}
	return out
}

// withTrailers refines an error using the call's trailers.
//
// Separate from [fromRPC] because trailers are only available from the call
// object, which a streaming caller has and a unary one gets through an option.
// The one structural refinement available: a redirect is an UNAVAILABLE naming
// another node, and telling that apart from "storage is down" is the
// difference between retrying elsewhere and retrying here.
func withTrailers(err error, md metadata.MD) error {
	var e *Error
	if !errors.As(err, &e) {
		return err
	}
	if len(md) > 0 {
		e.Trailers = make(map[string]string, len(md))
		for key, values := range md {
			if len(values) > 0 && len(key) > 4 && key[len(key)-4:] == "-bin" {
				continue
			}
			if len(values) > 0 {
				e.Trailers[key] = values[0]
			}
		}
	}
	if leader := e.Trailers[LeaderKey]; leader != "" && e.Kind == KindUnavailable {
		e.Kind = KindNotLeader
		e.Leader = leader
	}
	return e
}
