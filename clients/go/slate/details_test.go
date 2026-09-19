package slate

import (
	"bytes"
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

	pb "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
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

// checksBlobHex is a real grpc-status-details-bin from a write refused by three
// checks, captured the same way blobHex was — printed by
//
//	cargo test -p slate-server --test status -- --ignored --nocapture \
//	    emit_a_check_violation_blob
//
// which exists to produce exactly this. The Python and TypeScript suites decode
// the same bytes.
//
// Three failures on purpose, and the third with neither a column nor a message:
// discount_under_price spans two columns, so naming one would be a lie a form
// renders beside the wrong field. A fixture with one failure, or three
// identical ones, would not separate "reads the list" from "reads the first" or
// "assumes every failure has a column".
const checksBlobHex = "0803129b01726f772076696f6c61746573203320636865636b73206f6e207461626c" +
	"652060646f6373603a20607469746c655f6c656e677468603a205469746c65206d75" +
	"7374206265203120746f20383020636861726163746572732e3b206073697a655f70" +
	"6f736974697665603a2053697a652063616e6e6f74206265206e656761746976652e" +
	"3b2060646973636f756e745f756e6465725f7072696365601ae1020a28747970652e" +
	"676f6f676c65617069732e636f6d2f676f6f676c652e7270632e4572726f72496e66" +
	"6f12b4020a0f434845434b5f56494f4c4154494f4e1209736c6174652d6f726d1a0f" +
	"0a06636f6c756d6e12057469746c651a180a07636865636b2e31120d73697a655f70" +
	"6f7369746976651a1f0a07636865636b2e321214646973636f756e745f756e646572" +
	"5f70726963651a110a08636f6c756d6e2e3012057469746c651a2e0a096d65737361" +
	"67652e3012215469746c65206d757374206265203120746f20383020636861726163" +
	"746572732e1a100a08636f6c756d6e2e31120473697a651a250a096d657373616765" +
	"2e31121853697a652063616e6e6f74206265206e656761746976652e1a0d0a057461" +
	"626c651204646f63731a0f0a0a76696f6c6174696f6e731201331a170a0763686563" +
	"6b2e30120c7469746c655f6c656e6774681a150a05636865636b120c7469746c655f" +
	"6c656e677468"

func mustDecode(t *testing.T, blob string) []byte {
	t.Helper()
	out, err := hex.DecodeString(blob)
	if err != nil {
		t.Fatalf("the fixture is not hex: %v", err)
	}
	return out
}

func TestEveryFailingCheckComesBackTyped(t *testing.T) {
	// The payoff of publishing column and message: no prose to parse. A form
	// reads Column to pick the field and Message to fill it; the alternative a
	// caller has without this is a regular expression over the status text,
	// which lasts until somebody rewords a sentence.
	got := violationsOf(statusFrom(t, mustDecode(t, checksBlobHex)))
	want := []CheckViolation{
		{Check: "title_length", Column: "title", Message: "Title must be 1 to 80 characters."},
		{Check: "size_positive", Column: "size", Message: "Size cannot be negative."},
		// The cross-column one: a name and nothing to hang it on.
		{Check: "discount_under_price"},
	}
	if len(got) != len(want) {
		t.Fatalf("got %d violations, want %d: %+v", len(got), len(want), got)
	}
	for at := range want {
		if got[at] != want[at] {
			t.Fatalf("violation %d = %+v, want %+v", at, got[at], want[at])
		}
	}
}

func TestTheOrderIsTheSchemasAndNotTheMaps(t *testing.T) {
	// check.10 must not sort between check.1 and check.2. The metadata is a
	// string-keyed map and the server sends the index in the key, so anything
	// walking the map in key order would be right for nine failures and wrong
	// for eleven. Reading violations and counting up is what makes that
	// unreachable. Go's map iteration is randomised, which catches a different
	// half of the same mistake: a decoder that appended in range order would
	// fail this run to run rather than at eleven.
	blob := mustDecode(t, checksBlobHex)
	// The captured bytes really do carry the keys out of declaration order,
	// which is what makes this worth asserting rather than assuming.
	if bytes.Index(blob, []byte("check.1")) > bytes.Index(blob, []byte("check.0")) {
		t.Fatal("the fixture no longer carries its keys out of order; it proves less now")
	}
	for range 20 {
		got := violationsOf(statusFrom(t, blob))
		if got[0].Check != "title_length" {
			t.Fatalf("first violation = %q, want title_length", got[0].Check)
		}
	}
}

// violationDecoy is an ErrorInfo with a chosen reason and a chosen metadata map.
//
// It exists because two properties cannot be reached with a blob the server
// would actually send: a *non*-check failure carrying check-shaped keys, and a
// metadata map whose count and keys disagree. Both are what the decoder's
// guards are for.
func violationDecoy(t *testing.T, reason string, metadata map[string]string) *status.Status {
	t.Helper()
	packed, err := proto.Marshal(&errdetails.ErrorInfo{
		Reason: reason, Domain: "slate-orm", Metadata: metadata,
	})
	if err != nil {
		t.Fatalf("marshalling the decoy payload: %v", err)
	}
	return status.FromProto(&rpcstatus.Status{
		Code:    int32(codes.InvalidArgument),
		Details: []*anypb.Any{{TypeUrl: "type.googleapis.com/google.rpc.ErrorInfo", Value: packed}},
	})
}

func TestTheDecoyIsReadWhenItSaysItIsACheckViolation(t *testing.T) {
	// The negative control. Without it the two tests below pass for two
	// different reasons — the guard working, or the decoy being unreadable
	// either way — and only one of those is the property.
	got := violationsOf(violationDecoy(t, "CHECK_VIOLATION", map[string]string{
		"violations": "1", "check.0": "only", "column.0": "a",
	}))
	if len(got) != 1 || got[0].Check != "only" || got[0].Column != "a" {
		t.Fatalf("violations = %+v, want one named only on column a", got)
	}
}

func TestCheckShapedMetadataUnderAnotherReasonIsIgnored(t *testing.T) {
	// The real blob cannot show this: it carries no violations key, so a
	// decoder missing the reason check falls through to the same empty answer
	// by accident. This one carries the keys and the wrong reason, so only the
	// check itself can produce the empty list.
	got := violationsOf(violationDecoy(t, "UNIQUE_VIOLATION", map[string]string{
		"violations": "1", "check.0": "not_a_check", "column.0": "email",
	}))
	if got != nil {
		t.Fatalf("violations = %+v, want none", got)
	}
}

func TestACountTheKeysDoNotMatchYieldsNothing(t *testing.T) {
	// Three promised, two present: the prefix would be a quiet lie. A caller
	// shown two failures for a row that broke three fixes two fields,
	// resubmits and is refused again — the round-trip-per-field behaviour this
	// whole feature exists to remove.
	got := violationsOf(violationDecoy(t, "CHECK_VIOLATION", map[string]string{
		"violations": "3", "check.0": "one", "check.1": "two",
	}))
	if got != nil {
		t.Fatalf("violations = %+v, want none", got)
	}
}

func TestAServerSendingOnlyTheUnindexedPairYieldsOne(t *testing.T) {
	// One failure is the honest reading of what such a server said, and it is
	// what this client sent before V2 collected them all.
	got := violationsOf(violationDecoy(t, "CHECK_VIOLATION", map[string]string{
		"check": "title_length", "column": "title", "message": "Too long.",
	}))
	want := CheckViolation{Check: "title_length", Column: "title", Message: "Too long."}
	if len(got) != 1 || got[0] != want {
		t.Fatalf("violations = %+v, want just %+v", got, want)
	}
}

func TestAFailureThatIsNotACheckViolationHasNone(t *testing.T) {
	// blobHex is a predicate write that matched too many rows.
	if got := violationsOf(statusFrom(t, mustDecode(t, blobHex))); got != nil {
		t.Fatalf("violations = %+v, want none", got)
	}
}

func TestATruncatedCheckBlobDoesNotPanic(t *testing.T) {
	blob := mustDecode(t, checksBlobHex)
	for cut := range blob {
		// Never a throw: a client that raised while building an error object
		// would replace the server's failure with its own, and the caller
		// would stop learning why the call failed at all.
		violationsOf(statusFrom(t, blob[:cut]))
	}
}

func TestFromRPCCarriesTheViolationsOntoTheError(t *testing.T) {
	// The wiring test, and a different test from the ones above on purpose.
	// Those call violationsOf directly, so all of them keep passing if fromRPC
	// simply stops asking — which is the reasoning the token's wiring test
	// gives, and the mutation it caught.
	var e *Error
	err := fromRPC(context.Background(), statusFrom(t, mustDecode(t, checksBlobHex)).Err())
	if !errors.As(err, &e) {
		t.Fatal("fromRPC did not produce a *slate.Error")
	}
	if e.Reason != "CHECK_VIOLATION" {
		t.Fatalf("Reason = %q, want CHECK_VIOLATION", e.Reason)
	}
	if len(e.Violations) != 3 || e.Violations[0].Column != "title" {
		t.Fatalf("Violations = %+v, want three starting at title", e.Violations)
	}
}

func TestAnOrdinaryFailureCarriesNoViolations(t *testing.T) {
	var e *Error
	if !errors.As(fromRPC(context.Background(), statusFrom(t, mustDecode(t, blobHex)).Err()), &e) {
		t.Fatal("fromRPC did not produce a *slate.Error")
	}
	if len(e.Violations) != 0 {
		t.Fatalf("Violations = %+v, want none", e.Violations)
	}
}

// --- a batch's per-operation failure -----------------------------------------

func TestABatchedRefusalCarriesTheSameViolations(t *testing.T) {
	// The field a batched failure could not have. An independent batch reports
	// each failure as data inside a *successful* response, so there are no
	// trailers and no `grpc-status-details-bin` — a form submitted as a batch
	// got the token and the prose and nothing to put beside a field. The
	// server puts the same blob in the message body now.
	//
	// Asserted against the same fixture the lone path uses, which is the
	// point: one blob, one decoder, and a batched refusal that cannot come to
	// disagree with an unbatched one.
	var e *Error
	failed := &pb.BatchError{
		Code:    int32(codes.InvalidArgument),
		Message: "row violates 3 checks on table `docs`",
		Reason:  "CHECK_VIOLATION",
		Details: mustDecode(t, checksBlobHex),
	}
	if !errors.As(fromBatchError(failed), &e) {
		t.Fatal("fromBatchError did not produce a *slate.Error")
	}
	if e.Reason != "CHECK_VIOLATION" {
		t.Fatalf("Reason = %q", e.Reason)
	}
	if len(e.Violations) != 3 {
		t.Fatalf("Violations = %+v, want three", e.Violations)
	}
	if e.Violations[0].Column != "title" || e.Violations[2].Column != "" {
		t.Errorf("Violations = %+v", e.Violations)
	}
}

func TestABatchedFailureWithNoDetailsHasNoViolations(t *testing.T) {
	// Which is most of them: the blob is empty for every failure the server
	// does not attach one to, and an empty `bytes` field must not become a
	// decode attempt that answers something.
	var e *Error
	failed := &pb.BatchError{
		Code: int32(codes.NotFound), Message: "no such row", Reason: "",
	}
	if !errors.As(fromBatchError(failed), &e) {
		t.Fatal("fromBatchError did not produce a *slate.Error")
	}
	if len(e.Violations) != 0 {
		t.Fatalf("Violations = %+v, want none", e.Violations)
	}
	if e.Kind != KindNotFound {
		t.Fatalf("Kind = %v", e.Kind)
	}
}

func TestABatchedFailureWithRubbishDetailsDoesNotPanic(t *testing.T) {
	// The reasoning `reasonOf` gives: losing the violations is a degradation,
	// and raising here would replace the server's failure with this client's.
	var e *Error
	failed := &pb.BatchError{
		Code:    int32(codes.InvalidArgument),
		Message: "refused",
		Reason:  "CHECK_VIOLATION",
		Details: []byte{0xff, 0xff, 0xff, 0xff},
	}
	if !errors.As(fromBatchError(failed), &e) {
		t.Fatal("fromBatchError did not produce a *slate.Error")
	}
	if len(e.Violations) != 0 {
		t.Fatalf("Violations = %+v, want none", e.Violations)
	}
}
