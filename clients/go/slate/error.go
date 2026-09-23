package slate

import (
	"context"
	"errors"
	"fmt"
	"strconv"

	"google.golang.org/genproto/googleapis/rpc/errdetails"
	rpcstatus "google.golang.org/genproto/googleapis/rpc/status"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/metadata"
	"google.golang.org/grpc/status"
	"google.golang.org/protobuf/proto"

	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
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
	// Binary entries are dropped rather than decoded: a []byte hiding in a
	// map[string]string is the kind of thing that only fails once it reaches a
	// log line. The server does send one — grpc-status-details-bin, the
	// rich-error blob — and Reason below carries what this client reads out of
	// it, so nothing is lost by keeping this map[string]string.
	Trailers map[string]string
	// Reason is the server's stable token for this failure, or "".
	//
	// Populated on every failure the head node reports, batched or not. The
	// two paths carry it differently, which is the server's doing rather than
	// this client's: a batched failure has it in the message body, because a
	// batch's per-operation errors are data inside a successful response, and
	// a lone failure has it in the status details as a google.rpc.ErrorInfo.
	//
	// The tokens are one per kernel variant — UNIQUE_VIOLATION,
	// REPLICA_TOO_STALE, PREDICATE_WRITE_TOO_LARGE — and the server guarantees
	// that no two errors behind one status code share one, which is what makes
	// this usable where Code is not: Code is many-to-one and this is not.
	// Switch on it rather than on Message, which is prose and is not a
	// stability promise.
	//
	// "" when the server sent no ErrorInfo and for a failure raised without
	// reaching the server.
	Reason string
	// Violations is every CHECK a refused row broke, in declaration order.
	//
	// Empty for every failure that is not a check violation, which is almost
	// all of them. Each entry carries the constraint's name, the column it is
	// about and the sentence to show — so a form puts the message beside the
	// field rather than parsing it out of Message, which is prose and is not
	// a stability promise.
	//
	// The server reports every failing check rather than the first, so a row
	// with three bad fields produces three entries and one round trip.
	Violations []CheckViolation
	// RequestID is the id this client sent for the call that failed, or "".
	//
	// Not the server's — the server assigns none. This is what went out in
	// slate-request-id, kept so a caller holding a failure can go and find the
	// line the daemon logged for it when [observability] request_log is on.
	//
	// Present on failures that never reached the server too: an id with no
	// matching log line says the call did not arrive, which is itself the
	// answer to a question somebody would otherwise spend an hour on.
	RequestID string
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
// fromRPC turns a gRPC failure into an *Error.
//
// ctx is the one the call was made with, and is read for the request id the
// session put there. gRPC gives a client no way back to the metadata it *sent*
// — a status carries what came back and nothing of what went out — so the
// context is the only place that value still exists at this point.
func fromRPC(ctx context.Context, err error) error {
	return fromRPCWithID(requestIDOf(ctx), err)
}

// fromRPCWithID is fromRPC for a caller holding the id but not the context.
//
// The streams: Next() reports a failure that arrives on the tenth message,
// long after the context that opened the call has gone out of scope, so they
// keep the id on the struct and hand it in here.
func fromRPCWithID(id string, err error) error {
	if err == nil {
		return nil
	}
	st, ok := status.FromError(err)
	if !ok {
		return &Error{
			Kind:      KindInternal,
			Message:   err.Error(),
			Code:      codes.Unknown,
			RequestID: id,
		}
	}
	kind, known := byCode[st.Code()]
	if !known {
		kind = KindInternal
	}
	return &Error{
		Kind:       kind,
		Message:    st.Message(),
		Code:       st.Code(),
		Reason:     reasonOf(st),
		Violations: violationsOf(st),
		RequestID:  id,
	}
}

// CheckViolation is one CHECK a refused row broke.
//
// Column and Message are empty where the schema supplied none: a check
// spanning two columns has no single one to name, and naming either would be
// a lie a form renders beside the wrong field. Empty strings rather than
// pointers, for the reason [CheckRule] gives.
type CheckViolation struct {
	// Check is the constraint's name, as the schema declares it.
	Check string
	// Column is the column it is about, or "" for one spanning several.
	Column string
	// Message is the sentence to show, or "" where the schema wrote none.
	Message string
}

// violationsOf is every check a refused write broke, in declaration order.
//
// Reads the indexed keys (check.0, column.0, message.0, …) rather than the
// unindexed pair, which is only the first failure. Counting up from the
// violations count rather than walking the map for check.* keys, because the
// metadata is string-keyed: check.10 sorts between check.1 and check.2, so a
// map walk is right for nine failures and wrong for eleven.
//
// Returns nothing rather than a prefix when the count and the keys disagree.
// A caller shown two failures for a row that broke three fixes two fields,
// resubmits and is refused again — which is the round-trip-per-field
// behaviour this exists to remove.
func violationsOf(st *status.Status) []CheckViolation {
	for _, detail := range st.Details() {
		info, ok := detail.(*errdetails.ErrorInfo)
		if !ok {
			continue
		}
		if info.GetReason() != "CHECK_VIOLATION" {
			return nil
		}
		data := info.GetMetadata()
		total, err := strconv.Atoi(data["violations"])
		if err != nil {
			// Old enough to send the unindexed pair and no count. One
			// failure is the honest reading of what it said.
			if name := data["check"]; name != "" {
				return []CheckViolation{{
					Check: name, Column: data["column"], Message: data["message"],
				}}
			}
			return nil
		}
		out := make([]CheckViolation, 0, total)
		for at := range total {
			name := data[fmt.Sprintf("check.%d", at)]
			if name == "" {
				return nil
			}
			out = append(out, CheckViolation{
				Check:   name,
				Column:  data[fmt.Sprintf("column.%d", at)],
				Message: data[fmt.Sprintf("message.%d", at)],
			})
		}
		return out
	}
	return nil
}

// reasonOf is the stable token in a status's details, or "".
//
// st.Details decodes each packed Any against the global protobuf registry, so
// this works only because errdetails is imported: without that import the
// ErrorInfo arrives as an unresolved type and the token is silently lost. That
// is the failure this whole function exists to fix, so the import is load
// bearing rather than incidental.
//
// Details returns an error in place of a message it could not unmarshal. Those
// are skipped rather than surfaced: a client that failed to report why a call
// failed because it could not parse an optional annotation would be replacing
// the server's error with its own.
func reasonOf(st *status.Status) string {
	for _, detail := range st.Details() {
		if info, ok := detail.(*errdetails.ErrorInfo); ok {
			return info.GetReason()
		}
	}
	return ""
}

// fromBatchError is the *Error a batch's per-operation failure becomes.
//
// An independent batch reports each failure as data, inside a successful
// response, so the code and message arrive in a message body rather than in
// trailers. This turns them back into the same type a lone call returns, so a
// caller writes one errors.As whether or not the write was batched.
//
// Including Violations, which used to be the one field a batched failure could
// not carry: there are no trailers, so there was no `grpc-status-details-bin`
// and nothing to decode. The server now puts the same blob in the message
// body, and this reads it with the same decoder the lone path uses — so a form
// submitted as a batch gets the same typed failures as one submitted alone.
func fromBatchError(failed *pb.BatchError) error {
	if failed == nil {
		return &Error{Kind: KindInternal, Message: "a batch reported an empty error", Code: codes.Unknown}
	}
	code := codes.Code(uint32(failed.Code))
	kind, known := byCode[code]
	if !known {
		kind = KindInternal
	}
	return &Error{
		Kind:       kind,
		Message:    failed.Message,
		Code:       code,
		Reason:     failed.Reason,
		Violations: violationsInDetails(failed.Details),
	}
}

// violationsInDetails decodes a `google.rpc.Status` carried as bytes.
//
// [violationsOf] takes a *status.Status because that is what a lone failure
// arrives as; a batched one arrives as the encoded bytes. Rebuilding the
// status from them rather than writing a second walk keeps one decoder: the
// alternative is two, which agree until somebody edits one.
func violationsInDetails(details []byte) []CheckViolation {
	if len(details) == 0 {
		return nil
	}
	var raw rpcstatus.Status
	if proto.Unmarshal(details, &raw) != nil {
		// A blob that does not parse loses the violations and nothing else.
		// Raising here would replace the server's failure with this client's.
		return nil
	}
	return violationsOf(status.FromProto(&raw))
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
