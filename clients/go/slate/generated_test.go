package slate_test

import (
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

// TestStubsAreFresh regenerates the protobuf stubs and requires the result to
// be byte-identical to what is committed.
//
// The Python client has had this from the start and this one did not, and the
// difference showed: the committed Go stubs were generated before
// `Scalar.round` and `Scalar.calendar_part` were added to the protocol, and
// stayed stale through two rounds of work. Nothing caught it because no Go code
// referenced the missing fields — a stale generated file is invisible until
// someone needs the field that is not there, and then it reads as "undefined:
// pb.Scalar_Round", which looks like a typo rather than like drift.
//
// The generated code is a build artifact whose freshness is asserted, rather
// than one that is trusted.
//
// Skipped where `protoc` is absent, which is the one case where a hard failure
// would be about the machine rather than about the repository. That is a skip
// of the kind CLAUDE.md warns about — "a skip is green" — and the warning was
// earned: the sentence above used to say "CI installs `protoc` and this runs
// there for real", and no CI job installed `protoc`. This test had therefore
// never run anywhere but a laptop, and the stubs went stale again — missing
// `Assignment`, `DeleteWhereRequest` and `UpdateWhereRequest` — with the Go
// job green, exactly the drift it was written to stop.
//
// So the skip is no longer allowed to be silent where it matters.
// `SLATE_REQUIRE_PROTOC=1` turns it into a failure, and CI sets it beside the
// step that installs `protoc`. Installing `protoc` alone would have fixed
// today and left the same hole open for the next workflow edit that dropped
// the step; a guard that announces its own absence cannot be switched off by
// accident.
func TestStubsAreFresh(t *testing.T) {
	if _, err := exec.LookPath("protoc"); err != nil {
		if os.Getenv("SLATE_REQUIRE_PROTOC") != "" {
			t.Fatal(
				"SLATE_REQUIRE_PROTOC is set and protoc is not installed, so this " +
					"check would have skipped silently — which is how the committed " +
					"stubs went stale twice. Install protoc, or unset the variable " +
					"if this is a laptop.",
			)
		}
		t.Skip("protoc is not installed; set SLATE_REQUIRE_PROTOC=1 to make this fatal")
	}
	root, err := filepath.Abs("../../..")
	if err != nil {
		t.Fatal(err)
	}
	staging := t.TempDir()

	script := filepath.Join(root, "clients", "go", "scripts", "generate_proto.py")
	cmd := exec.Command("python3", script)
	// `--into` does not exist: the script writes where the stubs live. So it is
	// run against a copy of the tree's output directory instead — the committed
	// files are read first, the script overwrites them, and they are restored
	// whatever happens. Comparing in place is what makes this test check the
	// bytes that ship rather than bytes generated a second way.
	out := filepath.Join(root, "clients", "go", "internal", "pb", "slate", "v1")
	names := []string{"records.pb.go", "records_grpc.pb.go"}

	before := map[string][]byte{}
	for _, name := range names {
		data, err := os.ReadFile(filepath.Join(out, name))
		if err != nil {
			t.Fatalf("reading the committed %s: %v", name, err)
		}
		before[name] = data
		if err := os.WriteFile(filepath.Join(staging, name), data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	t.Cleanup(func() {
		for name, data := range before {
			_ = os.WriteFile(filepath.Join(out, name), data, 0o600)
		}
	})

	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("regenerating the stubs failed: %v\n%s", err, output)
	}

	for _, name := range names {
		after, err := os.ReadFile(filepath.Join(out, name))
		if err != nil {
			t.Fatal(err)
		}
		if string(after) != string(before[name]) {
			t.Errorf(
				"%s is stale: regenerating it from slate/v1/records.proto produced "+
					"different bytes (%d committed, %d generated). Run "+
					"`python3 clients/go/scripts/generate_proto.py` and commit the result.",
				name, len(before[name]), len(after),
			)
		}
	}
}
