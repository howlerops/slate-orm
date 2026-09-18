package slate

import (
	"context"
	"errors"
	"math/rand/v2"
	"time"
)

// RetryPolicy is how [Session.Transact] backs off between attempts.
//
// The zero value is not usable; [DefaultRetry] is the one to start from. A
// struct rather than four arguments because three of the four are almost never
// changed, and a call site carrying three defaults reads as though somebody
// chose them.
type RetryPolicy struct {
	// Attempts is how many times the body may run, including the first. One
	// means no retry. Zero or negative is refused rather than silently
	// treated as one: a caller who computed it from configuration and got
	// zero meant something, and it was not "run it once".
	Attempts int
	// BaseDelay is the first backoff. Doubles each attempt.
	BaseDelay time.Duration
	// MaxDelay caps it.
	MaxDelay time.Duration
}

// DefaultRetry matches the Python client's defaults, which is the point: the
// three clients should not disagree about how hard they try.
var DefaultRetry = RetryPolicy{
	Attempts:  5,
	BaseDelay: 5 * time.Millisecond,
	MaxDelay:  500 * time.Millisecond,
}

// Transact runs body in a transaction, committing on success, rolling back on
// any error, and retrying a conflict.
//
//	total, err := slate.Transact(ctx, session, slate.DefaultRetry,
//		func(ctx context.Context, tx *slate.Transaction) (uint64, error) {
//			if _, err := tx.Insert(ctx, "books", row); err != nil {
//				return 0, err
//			}
//			return 1, nil
//		})
//
// # What is retried, and what is not
//
// [KindConflict] and nothing else, which is `RecordStore::transact`'s own rule:
// a unique violation, an access denial or a fenced writer fails identically
// forever, and retrying them turns a clear error into a hang.
//
// In particular [KindUnavailable] is *not* retried even though [Error.Retryable]
// reports it as retryable, and neither is [KindNotLeader]. Both are retryable
// somewhere — against a different node — and this function only has the one it
// was given. Retrying them here spends the caller's attempts on a node that
// will keep saying no.
//
// # Why the body takes a context
//
// So a body that outlives its attempt cannot use a cancelled one. Each attempt
// gets the context it was called with; a body that captured an outer `ctx`
// would work by accident and break the first time somebody added a per-attempt
// deadline.
//
// # Why this is a function and not a method
//
// Go has no generic methods, and a `Transact` on `*Session` would have to
// return `any` or `error` alone — which is the version every caller then wraps
// to get their value back out. A generic free function is the shape that lets
// the body return something.
func Transact[T any](
	ctx context.Context,
	session *Session,
	policy RetryPolicy,
	body func(context.Context, *Transaction) (T, error),
) (T, error) {
	var zero T
	if policy.Attempts < 1 {
		return zero, errors.New("slate: RetryPolicy.Attempts must be at least 1")
	}
	for attempt := 0; ; attempt++ {
		value, err := attemptOnce(ctx, session, body)
		if err == nil {
			return value, nil
		}
		var failure *Error
		// The last attempt returns the conflict rather than a wrapper saying
		// how many times it tried: a caller matching on KindConflict should
		// not have to unwrap a count to do it.
		if attempt == policy.Attempts-1 || !errors.As(err, &failure) || failure.Kind != KindConflict {
			return zero, err
		}
		// Full jitter: uniform in [0, backoff] rather than backoff plus or
		// minus something, which still leaves a mode for the colliding
		// writers to land on together.
		backoff := policy.BaseDelay << attempt
		if backoff > policy.MaxDelay || backoff <= 0 {
			backoff = policy.MaxDelay
		}
		select {
		case <-time.After(rand.N(backoff + 1)):
		case <-ctx.Done():
			// The caller's deadline beats our backoff. Returning the conflict
			// rather than ctx.Err() keeps the cause: "this kept conflicting"
			// is more useful than "time ran out", and the deadline is visible
			// on the context anyway.
			return zero, err
		}
	}
}

// attemptOnce runs one attempt, with the rollback in a defer so a panic in the
// body does not leave the transaction open until the server's idle timeout.
func attemptOnce[T any](
	ctx context.Context,
	session *Session,
	body func(context.Context, *Transaction) (T, error),
) (T, error) {
	var zero T
	transaction, err := session.Begin(ctx)
	if err != nil {
		return zero, err
	}
	// Best effort, and deliberately ignoring its error: the transaction may
	// already be gone — the server rolls one back on an idle timeout — and a
	// second error out of the cleanup would hide the first, which is the one
	// that says what went wrong. After a successful Commit this is a no-op.
	defer func() { _ = transaction.Rollback(ctx) }()

	value, err := body(ctx, transaction)
	if err != nil {
		return zero, err
	}
	if err := transaction.Commit(ctx); err != nil {
		return zero, err
	}
	return value, nil
}
