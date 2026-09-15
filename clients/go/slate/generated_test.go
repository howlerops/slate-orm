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
// of the kind CLAUDE.md warns about — "a skip is green" — so CI installs
// `protoc` and this runs there for real; the skip exists for a laptop.
func TestStubsAreFresh(t *testing.T) {
	if _, err := exec.LookPath("protoc"); err != nil {
		t.Skip("protoc is not installed; CI runs this for real")
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
