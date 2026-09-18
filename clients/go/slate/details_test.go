package slate

import (
	"context"
	"encoding/hex"
	"errors"
	"testing"

	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
	"google.golang.org/protobuf/proto"

	"google.golang.org/genproto/googleapis/rpc/errdetails"
	rpcstatus "google.golang.org/genproto/googleapis/rpc/status"
	"google.golang.org/protobuf/types/known/anypb"
)

// blobHex is a real grpc-status-details-bin, captured from a running head node
// rather than built here: a fixture this file encoded itself would agree with
// this file's own idea of the wire format, and agreeing with yourself proves
// nothing. The Python and TypeScript suites decode these same bytes, so the
// three clients are checked against one artefact rather than three
// transcriptions of it.
//
// It came from a delete_where with returning that matched more rows than
// max_returned_rows allowed.
const blobHex = "08081290016120707265646963617465207772697465206d617463686564206d6f7265207468616e203320726f77732c20616e642069747320726f777320776572652061736b656420666f723b206e6172726f7720746865207072656469636174652c2064726f7020746865207265717565737420666f722074686520726f77732c206f7220726169736520746865206c696d69741a520a28747970652e676f6f676c65617069732e636f6d2f676f6f676c652e7270632e4572726f72496e666f12260a195052454449434154455f57524954455f544f4f5f4c415247451209736c6174652d6f726d"

// statusFrom rebuilds a *status.Status the way a call would deliver one, so
// these tests exercise reasonOf through the same Details() path fromRPC uses
// rather than through a shortcut of their own.
func statusFrom(t *testing.T, blob []byte) *status.Status {
	t.Helper()
	var raw rpcstatus.Status
	if err := proto.Unmarshal(blob, &raw); err != nil {
		// A blob that is not a Status at all still has to reach reasonOf as
		// something, and an empty status is what the caller would see.
		return status.New(codes.ResourceExhausted, "")
	}
	return status.FromProto(&raw)
}

func TestTheTokenComesOutOfARealBlob(t *testing.T) {
	blob, err := hex.DecodeString(blobHex)
	if err != nil {
		t.Fatalf("the fixture is not hex: %v", err)
	}
	if got := reasonOf(statusFrom(t, blob)); got != "PREDICATE_WRITE_TOO_LARGE" {
		t.Fatalf("reason = %q, want PREDICATE_WRITE_TOO_LARGE", got)
	}
}

func TestAStatusWithNoDetailsHasNoToken(t *testing.T) {
	if got := reasonOf(status.New(codes.ResourceExhausted, "no details here")); got != "" {
		t.Fatalf("reason = %q, want the empty token", got)
	}
}

func TestATruncatedBlobDoesNotPanic(t *testing.T) {
	blob, err := hex.DecodeString(blobHex)
	if err != nil {
		t.Fatalf("the fixture is not hex: %v", err)
	}
	// Details returns an error value in place of anything it could not
	// unmarshal, and reasonOf has to skip those rather than assert on them.
	for cut := range blob {
		got := reasonOf(statusFrom(t, blob[:cut]))
		if got != "" && got != "PREDICATE_WRITE_TOO_LARGE" {
			t.Fatalf("prefix of %d decoded to %q", cut, got)
		}
	}
}

// decoy is a well-formed Status whose one detail is packed under url.
//
// Built with proto.Marshal rather than by hand, and deliberately shaped like an
// ErrorInfo — a string in field 1 — so that a decoder which took the first
// detail without checking its type would hand back the reason rather than "".
// Go's type assertion is what stops that here, and this is what proves the
// assertion is load bearing rather than decorative.
func decoy(t *testing.T, url, reason string) *status.Status {
	t.Helper()
	info := &errdetails.ErrorInfo{Reason: reason}
	packed, err := proto.Marshal(info)
	if err != nil {
		t.Fatalf("marshalling the decoy payload: %v", err)
	}
	raw := &rpcstatus.Status{
		Code:    int32(codes.ResourceExhausted),
		Details: []*anypb.Any{{TypeUrl: url, Value: packed}},
	}
	return status.FromProto(raw)
}

func TestADetailOfAnotherTypeIsSkippedNotMisread(t *testing.T) {
	got := reasonOf(decoy(t, "type.googleapis.com/google.rpc.RetryInfo", "DECOY"))
	if got != "" {
		t.Fatalf("reason = %q, want the empty token", got)
	}
}

func TestTheDecoyWouldBeReadUnderTheRightURL(t *testing.T) {
	// The negative control: without it the test above passes either because the
	// type check works or because the decoy decodes to nothing regardless, and
	// only one of those is the property being asserted.
	got := reasonOf(decoy(t, "type.googleapis.com/google.rpc.ErrorInfo", "DECOY"))
	if got != "DECOY" {
		t.Fatalf("reason = %q, want DECOY", got)
	}
}

// TestFromRPCCarriesTheTokenOntoTheError is the wiring test, and it is a
// different test from the ones above on purpose.
//
// Those call reasonOf directly, so all of them keep passing if fromRPC simply
// stops asking for a token — which is exactly the state this change fixed, and
// a mutation that removed the call survived every decoder test here. The
// conformance runner caught it, but a cross-language suite is a slow and
// indirect way to learn that one client stopped populating one field.
func TestFromRPCCarriesTheTokenOntoTheError(t *testing.T) {
	blob, err := hex.DecodeString(blobHex)
	if err != nil {
		t.Fatalf("the fixture is not hex: %v", err)
	}
	var e *Error
	if !errors.As(fromRPC(context.Background(), statusFrom(t, blob).Err()), &e) {
		t.Fatal("fromRPC did not produce a *slate.Error")
	}
	if e.Reason != "PREDICATE_WRITE_TOO_LARGE" {
		t.Fatalf("Reason = %q, want PREDICATE_WRITE_TOO_LARGE", e.Reason)
	}
	// The rest of the error is unchanged by carrying a token.
	if e.Kind != KindResourceLimit {
		t.Fatalf("Kind = %v, want KindResourceLimit", e.Kind)
	}
}
