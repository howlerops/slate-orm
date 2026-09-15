package slate_test

// Starting a real head node, and connecting to it.
//
// Every test here runs against `slate-serverd` built from this repository and
// started as a subprocess. No mock and no in-process fake, for the reason the
// Python client's conftest gives: a mock is a second statement of what the
// server does, written by whoever wrote the client, so it agrees with the
// client's misunderstandings. Finding those is the point of a second client.
//
// The daemon prints `LISTENING <addr>` once its listener is bound and before
// it serves, so the harness waits for that line rather than polling the port —
// which closes the race where a connection arrives between bind and accept.

import (
	"bufio"
	"context"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/howlerops/slate-orm/clients/go/slate"
)

const config = `
[listen]
address = "127.0.0.1:0"

[auth]
mode = "trusted-header"

[storage]
backend = "memory"

[[tables]]
name = "docs"
id = 1
columns = [
  { name = "id",   type = "u64" },
  { name = "kind", type = "str" },
  { name = "size", type = "i64" },
]
primary_key = ["id"]

[[security.grants]]
role = "app"
tables = ["docs"]
actions = ["everything"]
`

var (
	buildOnce  sync.Once
	binaryPath string
	buildErr   error
)

// binary builds slate-serverd once per test binary, or takes one already built.
//
// `cargo build` by default rather than a prebuilt path, so the suite tests the
// daemon in this working tree — the only version whose protocol this client was
// written against.
//
// `SLATE_SERVERD` overrides it with a path. Two reasons, and neither is speed:
// CI builds the daemon once and hands the same binary to all three client
// suites rather than three checkouts of the Rust toolchain building it three
// times, and a contributor working only on this client can run the suite with
// no Rust installed at all. A path that is set and does not exist is a hard
// error — falling back to `cargo` there would quietly test a *different*
// binary from the one the caller named.
func binary(t *testing.T) string {
	t.Helper()
	buildOnce.Do(func() {
		if named := os.Getenv("SLATE_SERVERD"); named != "" {
			info, err := os.Stat(named)
			if err != nil {
				buildErr = fmt.Errorf("SLATE_SERVERD=%s: %w", named, err)
				return
			}
			if err := refuseIfStale(named, info); err != nil {
				buildErr = err
				return
			}
			binaryPath = named
			return
		}
		root, err := repoRoot()
		if err != nil {
			buildErr = err
			return
		}
		cmd := exec.Command("cargo", "build", "-p", "slate-serverd", "--bin", "slate-serverd")
		cmd.Dir = root
		if out, err := cmd.CombinedOutput(); err != nil {
			buildErr = fmt.Errorf("building slate-serverd: %v\n%s", err, out)
			return
		}
		binaryPath = filepath.Join(root, "target", "debug", "slate-serverd")
	})
	if buildErr != nil {
		t.Fatal(buildErr)
	}
	return binaryPath
}

// refuseIfStale rejects a prebuilt binary older than the source it was built
// from.
//
// This is here because it happened, in the Python suite: a full run reported
// 153 passing tests against a `slate-testserver` built before that session's
// server changes, so every test of the new behaviour was checking the old
// server and passing, because the client asked for something the old binary
// politely ignored. Three new tests failing after a rebuild is what found it,
// which is luck rather than a process.
//
// Modification times are crude and catch the whole of the real failure: a
// binary CI just handed over is minutes old, and one built last week is not.
func refuseIfStale(path string, info os.FileInfo) error {
	root, err := repoRoot()
	if err != nil {
		// Not in a checkout, so there is no source to compare against. That
		// is a legitimate way to run this — a released binary and the client
		// from a module cache — so it is not an error.
		return nil
	}
	var newest time.Time
	var newestPath string
	for _, dir := range []string{"crates", "clients/python/testserver"} {
		walkErr := filepath.WalkDir(filepath.Join(root, dir), func(p string, d fs.DirEntry, err error) error {
			if err != nil {
				return nil
			}
			// `target/` is build output, and the biggest directory in the tree.
			if d.IsDir() && d.Name() == "target" {
				return filepath.SkipDir
			}
			if d.IsDir() {
				return nil
			}
			switch filepath.Ext(p) {
			case ".rs", ".toml", ".proto":
			default:
				return nil
			}
			stat, err := d.Info()
			if err != nil {
				return nil
			}
			if stat.ModTime().After(newest) {
				newest, newestPath = stat.ModTime(), p
			}
			return nil
		})
		if walkErr != nil {
			return nil
		}
	}
	if newestPath == "" || !info.ModTime().Before(newest) {
		return nil
	}
	relative, err := filepath.Rel(root, newestPath)
	if err != nil {
		relative = newestPath
	}
	return fmt.Errorf(
		"SLATE_SERVERD=%s was built before %s was last changed, so the suite "+
			"would test a server this tree did not produce. Rebuild it, or unset "+
			"SLATE_SERVERD to build from source",
		path, relative,
	)
}

func repoRoot() (string, error) {
	dir, err := os.Getwd()
	if err != nil {
		return "", err
	}
	for {
		if _, err := os.Stat(filepath.Join(dir, "Cargo.toml")); err == nil {
			if _, err := os.Stat(filepath.Join(dir, "crates")); err == nil {
				return dir, nil
			}
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return "", fmt.Errorf("no repository root above %s", dir)
		}
		dir = parent
	}
}

// serving is a running head node.
type serving struct {
	addr string
	cmd  *exec.Cmd
}

// start runs a daemon on an ephemeral port and waits for it to be listening.
func start(t *testing.T, extra string) *serving {
	t.Helper()
	dir := t.TempDir()
	path := filepath.Join(dir, "head.toml")
	if err := os.WriteFile(path, []byte(config+extra), 0o600); err != nil {
		t.Fatalf("writing the configuration: %v", err)
	}

	cmd := exec.Command(binary(t), "--config", path)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatalf("capturing stdout: %v", err)
	}
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		t.Fatalf("starting slate-serverd: %v", err)
	}

	addrCh := make(chan string, 1)
	go func() {
		scanner := bufio.NewScanner(stdout)
		for scanner.Scan() {
			line := scanner.Text()
			if rest, ok := strings.CutPrefix(line, "LISTENING "); ok {
				addrCh <- strings.TrimSpace(rest)
				// Keep draining so the daemon never blocks on a full pipe.
				continue
			}
		}
	}()

	var addr string
	select {
	case addr = <-addrCh:
	case <-time.After(30 * time.Second):
		_ = cmd.Process.Kill()
		t.Fatal("slate-serverd never said it was listening")
	}

	s := &serving{addr: addr, cmd: cmd}
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
	})
	return s
}

// client dials the node as an identity holding the `app` role.
func (s *serving) client(t *testing.T) *slate.Client {
	t.Helper()
	c, err := slate.Dial(s.addr, slate.Identity{
		Principal: "u64:1",
		Tenant:    "u64:1",
		Roles:     []string{"app"},
	})
	if err != nil {
		t.Fatalf("dialling %s: %v", s.addr, err)
	}
	t.Cleanup(func() { _ = c.Close() })
	return c
}

func testContext(t *testing.T) context.Context {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	t.Cleanup(cancel)
	return ctx
}
