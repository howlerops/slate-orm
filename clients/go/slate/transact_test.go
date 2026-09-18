package slate_test

import (
	"context"
	"errors"
	"strings"
	"sync"
	"testing"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// Transact retries a conflict, and loses no update.
//
// Two writers read the same counter and both increment it. Without the barrier
// they would almost always run serially and the test would prove nothing: the
// retry would never fire and a Transact that did not retry at all would pass.
// With it, both have read before either commits, so one of them *must* conflict.
func TestTransactRetriesAConflictAndLosesNoUpdate(t *testing.T) {
	serving := start(t, "")
	ctx := testContext(t)

	setup := serving.client(t).Session()
	if _, err := setup.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(900), slate.String("counter"), slate.Int(0)}); err != nil {
		t.Fatalf("seeding: %v", err)
	}

	var barrier sync.WaitGroup
	barrier.Add(2)
	var reached sync.Once
	release := make(chan struct{})

	var wg sync.WaitGroup
	attempts := make([]int, 2)
	failures := make([]error, 2)
	for who := 0; who < 2; who++ {
		wg.Add(1)
		go func(who int) {
			defer wg.Done()
			session := serving.client(t).Session()
			_, err := slate.Transact(ctx, session, slate.DefaultRetry,
				func(ctx context.Context, tx *slate.Transaction) (struct{}, error) {
					attempts[who]++
					row, found, err := tx.Get(ctx, "docs", []slate.Value{slate.Uint(900)})
					if err != nil {
						return struct{}{}, err
					}
					if !found {
						return struct{}{}, errors.New("the counter row vanished")
					}
					size, ok := row[2].(slate.Int)
					if !ok {
						return struct{}{}, errors.New("size is not an i64")
					}
					if attempts[who] == 1 {
						// Only on the first attempt: waiting again would
						// deadlock once the two are no longer in step.
						barrier.Done()
						reached.Do(func() { go func() { barrier.Wait(); close(release) }() })
						<-release
					}
					_, err = tx.Update(ctx, "docs", []slate.Value{
						slate.Uint(900), slate.String("counter"), slate.Int(int64(size) + 1),
					})
					return struct{}{}, err
				})
			failures[who] = err
		}(who)
	}
	wg.Wait()

	for who, err := range failures {
		if err != nil {
			t.Fatalf("writer %d: %v", who, err)
		}
	}
	// Both increments landed: 0 -> 2. A lost update would leave 1, which is
	// exactly what a Transact that swallowed the conflict would produce.
	row, found, err := setup.Get(ctx, "docs", []slate.Value{slate.Uint(900)})
	if err != nil || !found {
		t.Fatalf("reading back: %v (found %v)", err, found)
	}
	if size, _ := row[2].(slate.Int); size != 2 {
		t.Fatalf("expected both increments to land, got %v", row[3])
	}
	// And somebody actually retried, or the barrier did not do its job and
	// this test is passing for the wrong reason.
	if attempts[0]+attempts[1] < 3 {
		t.Fatalf("no retry happened: attempts %v — the conflict was not forced", attempts)
	}
}

// A duplicate key fails identically forever; retrying it is a hang.
//
// `RecordStore::transact`'s rule, restated on this side.
func TestTransactDoesNotRetryWhatCanNeverSucceed(t *testing.T) {
	serving := start(t, "")
	ctx := testContext(t)
	session := serving.client(t).Session()

	if _, err := session.Insert(ctx, "docs",
		[]slate.Value{slate.Uint(901), slate.String("taken"), slate.Int(1)}); err != nil {
		t.Fatalf("seeding: %v", err)
	}

	tries := 0
	_, err := slate.Transact(ctx, session, slate.DefaultRetry,
		func(ctx context.Context, tx *slate.Transaction) (struct{}, error) {
			tries++
			_, err := tx.Insert(ctx, "docs",
				[]slate.Value{slate.Uint(901), slate.String("again"), slate.Int(2)})
			return struct{}{}, err
		})
	if err == nil {
		t.Fatal("a duplicate key must fail")
	}
	var failure *slate.Error
	if !errors.As(err, &failure) || failure.Kind != slate.KindAlreadyExists {
		t.Fatalf("expected already-exists, got %v", err)
	}
	if tries != 1 {
		t.Fatalf("a permanent failure was retried %d times", tries)
	}
}

// Attempts below one is refused rather than quietly treated as one.
func TestTransactRefusesZeroAttempts(t *testing.T) {
	serving := start(t, "")
	ctx := testContext(t)
	session := serving.client(t).Session()
	_, err := slate.Transact(ctx, session, slate.RetryPolicy{Attempts: 0},
		func(ctx context.Context, tx *slate.Transaction) (int, error) { return 1, nil })
	if err == nil || !strings.Contains(err.Error(), "at least 1") {
		t.Fatalf("expected a refusal naming the minimum, got %v", err)
	}
}
