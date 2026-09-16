package slate_test

// A per-call deadline, which in Go is the context and nothing else.
//
// The Python and TypeScript clients had to grow one: neither passed a deadline
// on any RPC, so a head node that accepted a connection and then stopped
// answering blocked the caller for ever. Go never had that gap, because every
// method here already takes a context.Context and grpc-go honours its deadline.
//
// That is a claim, and it was not tested. These two tests are the difference:
// one shows a context deadline ending a call against a listener that accepts
// and says nothing, the other shows the same call not returning without one.

import (
	"context"
	"net"
	"testing"
	"time"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

// Long enough that a loopback round trip could not explain the failure.
const grace = time.Second

// silentListener accepts connections and never speaks.
//
// Not a slow server and not a closed port: either produces a different error.
// This is the one shape where the caller's own deadline is the only thing that
// can end the call — gRPC completes the TCP connect and then waits for a server
// preface that never comes.
func silentListener(t *testing.T) string {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listening: %v", err)
	}
	done := make(chan struct{})
	go func() {
		defer close(done)
		var held []net.Conn
		defer func() {
			for _, conn := range held {
				conn.Close()
			}
		}()
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			// Held open, so the kernel does not close it and hand gRPC a
			// reset — which would be Unavailable, not a deadline.
			held = append(held, conn)
		}
	}()
	t.Cleanup(func() {
		listener.Close()
		<-done
	})
	return listener.Addr().String()
}

func TestAContextDeadlineEndsACallToASilentServer(t *testing.T) {
	client, err := slate.Dial(silentListener(t), slate.Identity{Principal: "u64:1"})
	if err != nil {
		t.Fatalf("dialing: %v", err)
	}
	t.Cleanup(func() { client.Close() })

	ctx, cancel := context.WithTimeout(context.Background(), grace)
	defer cancel()

	started := time.Now()
	_, _, err = client.Session().Get(ctx, "docs", []slate.Value{slate.Uint(1)})
	elapsed := time.Since(started)

	if err == nil {
		t.Fatal("a call to a server that never answers must not succeed")
	}
	if !slate.IsKind(err, slate.KindDeadlineExceeded) {
		t.Errorf("want KindDeadlineExceeded, got: %v", err)
	}
	// Bounded both ways. Too fast would mean something else failed the call and
	// the deadline proved nothing; too slow would mean the context is not what
	// ended it.
	if elapsed < grace || elapsed > grace+5*time.Second {
		t.Errorf("came back after %v, want about %v", elapsed, grace)
	}
}

func TestACallWithNoDeadlineDoesNotReturn(t *testing.T) {
	// The hang the other two clients had, and the reason this file exists: it
	// demonstrates that the deadline is doing the work, rather than asserting
	// only that the version with one succeeds.
	client, err := slate.Dial(silentListener(t), slate.Identity{Principal: "u64:1"})
	if err != nil {
		t.Fatalf("dialing: %v", err)
	}
	t.Cleanup(func() { client.Close() })

	returned := make(chan struct{})
	go func() {
		defer close(returned)
		// context.Background() has no deadline. The goroutine outlives the
		// test and ends when Cleanup closes the client, which is the point:
		// nothing inside the call can stop it.
		_, _, _ = client.Session().Get(
			context.Background(), "docs", []slate.Value{slate.Uint(1)})
	}()

	select {
	case <-returned:
		t.Fatal("a call with no deadline came back")
	case <-time.After(2 * grace):
	}
}
