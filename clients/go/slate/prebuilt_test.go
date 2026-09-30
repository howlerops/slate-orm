package slate_test

// The refusal in `harness_test.go`, executed.
//
// `scripts/test_prebuilt.py` holds all six harnesses that read `SLATE_SERVERD`
// or `SLATE_TESTSERVER` to refusing a binary older than the source it was
// built from. It did so by grep, which
// `ledger/2026-09-30-the-exemptions-i-wrote-without-reading.md` recorded as
// the roster's own limit: it can tell that a file mentions a refusal, not that
// the refusal works, so a harness that defines one and never calls it passes.
//
// This is the Go half of the answer. `prebuilt` is the branch `binary` takes
// when the variable is set, so driving it with a stale path exercises the same
// code a real run does — and the shell and Python harnesses are driven
// directly by `scripts/test_prebuilt.py`, which cannot run these two without a
// Go and a Node toolchain.

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// A path under the repository, so `repoRoot` finds the checkout and the walk
// has something to compare against. A binary in `os.TempDir()` would make
// `refuseIfStale` return nil for the legitimate reason it documents — not in
// a checkout, nothing to compare — and the test would pass for the wrong one.
func stale(t *testing.T) string {
	t.Helper()
	root, err := repoRoot()
	if err != nil {
		t.Skip("not in a checkout, so there is no source to be stale against")
	}
	// `t.TempDir()` is outside the repository, where `refuseIfStale` returns
	// nil for the legitimate reason above. Inside `target/` instead, which the
	// walk skips as build output and which every checkout has room for.
	dir := filepath.Join(root, "target", "prebuilt-test")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	path := filepath.Join(dir, "slate-serverd")
	if err := os.WriteFile(path, []byte("not a real binary"), 0o755); err != nil {
		t.Fatal(err)
	}
	old := time.Date(2000, 1, 1, 0, 0, 0, 0, time.UTC)
	if err := os.Chtimes(path, old, old); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestAStalePrebuiltBinaryIsRefused(t *testing.T) {
	path, err := prebuilt(stale(t))
	if err == nil {
		t.Fatalf("a binary from 2000 was accepted as %q", path)
	}
	if !strings.Contains(err.Error(), "was built before") {
		t.Fatalf("refused for the wrong reason: %v", err)
	}
}

func TestAPrebuiltPathThatIsNotThereIsRefused(t *testing.T) {
	// Set and missing is a hard error everywhere in this repository, never a
	// silent fall back to `cargo build`: the fallback tests a different binary
	// from the one the caller named.
	if _, err := prebuilt(filepath.Join(t.TempDir(), "absent")); err == nil {
		t.Fatal("a path that is set and missing was accepted")
	}
}

// The never-fires half. Both cases above pass if `prebuilt` refuses
// everything, which is also what a broken `refuseIfStale` would do.
func TestAFreshPrebuiltBinaryIsAccepted(t *testing.T) {
	root, err := repoRoot()
	if err != nil {
		t.Skip("not in a checkout")
	}
	dir := filepath.Join(root, "target", "prebuilt-test-fresh")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	path := filepath.Join(dir, "slate-serverd")
	if err := os.WriteFile(path, []byte("not a real binary"), 0o755); err != nil {
		t.Fatal(err)
	}
	soon := time.Now().Add(time.Hour)
	if err := os.Chtimes(path, soon, soon); err != nil {
		t.Fatal(err)
	}
	if got, err := prebuilt(path); err != nil || got != path {
		t.Fatalf("a binary newer than every source was refused: %v", err)
	}
}
