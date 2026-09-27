# `actions/setup-go@v6` sets `GOTOOLCHAIN=local`, and the pinned protobuf plugin needs a Go newer than CI installs. The Go client job went red on an action bump.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `clients/go/scripts/generate_proto.py`, `CLAUDE.md`
- **Kind:** correctness

## What changed

`ensure_plugins()` passes `GOTOOLCHAIN=auto` to the two `go install` calls that
fetch the protobuf generators, alongside the `GOFLAGS=-mod=mod` already there.

## Why

`ledger/2026-09-27-three-red-jobs-nobody-was-watching.md` bumped every `actions/*` still
on Node 20 to its first Node 24 major, `setup-go@v5` to `@v6` among them. Run
387 is the first completed run since — 385 and 386 were both cancelled by the
next push — and the **Go client** job was red in it and had been green in 384,
the last completed run before the bump.

`setup-go` sets `GOTOOLCHAIN=local` when `go-version` is pinned, which is the
right default: it stops a module quietly pulling down a different compiler
than the job asked for. `protoc-gen-go-grpc@v1.6.2` declares `go >= 1.25.0`
and this workflow installs 1.24, the version `clients/go/go.mod` claims, so
the install became a hard error:

```
go: ...protoc-gen-go-grpc@v1.6.2: requires go >= 1.25.0
    (running go 1.24.7; GOTOOLCHAIN=local)
```

No Go changed, no plugin version changed. The pin and the floor had been
incompatible since the pin landed in `bf125bb`; `GOTOOLCHAIN` defaulting to
`auto` had been silently downloading a 1.26 toolchain to build it, and the
action bump took that away.

This is the toolchain-gap class `CLAUDE.md` already documents for clippy,
`rustfmt` and `ruff`, arriving from a new direction: not a newer tool being
stricter, but an *action* changing what the tool is allowed to do.

## Alternatives rejected

**Raise CI's `go-version` to 1.25.** The obvious fix and it works. Rejected
because `clients/go/go.mod` says `go 1.24`, and CI building with 1.25 stops
CI testing the version the module claims to support — the support matrix
would then be an assertion nothing runs. The plugin is a build tool, not part
of that matrix, and its toolchain requirement should not drag the library's.

**Un-pin the plugin, or move it back to a version 1.24 can build.**
`CLAUDE.md` says to pin anything that generates committed code, for a defect
this repository already met with `grpcio-tools`. Moving to `v1.5.x` would also
change the committed stubs' generated-by header and their contents, which is a
real diff to justify with "CI's action changed".

**Set `GOTOOLCHAIN` in the workflow rather than in the script.** It would fix
CI and leave a laptop with `GOTOOLCHAIN=local` in the same hole, and the
reason lives with the pin rather than with the job. The script already carries
`GOFLAGS` for the same reason.

## Evidence

**The failure, reproduced on this container.** Go 1.24.7 here, the same major
CI installs:

```
$ GOTOOLCHAIN=local go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2
go: ...requires go >= 1.25.0 (running go 1.24.7; GOTOOLCHAIN=local)
EXIT=1
```

and the job's actual failure, the same way, with the plugin removed from
`$GOPATH/bin` first so the `if (binaries / name).exists(): continue` fast path
could not hide it:

```
$ git stash push scripts/generate_proto.py
$ GOTOOLCHAIN=local go test ./slate -run TestStubsAreFresh -count=1
subprocess.CalledProcessError: Command '['go', 'install',
  'google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2']' returned non-zero
  exit status 1.
FAIL	github.com/howlerops/slate-orm/clients/go/slate	0.104s
EXIT=1
```

**And fixed, under the same `GOTOOLCHAIN=local`**, plugin removed again:

```
$ GOTOOLCHAIN=local go test ./... -run TestStubsAreFresh -count=1
ok  	github.com/howlerops/slate-orm/clients/go/slate	2.783s
```

`-count=1` throughout, for the reason `scripts/mutate.py`'s `go` dialect
refuses a `(cached)` line.

**The other plugin does not need it.** `protoc-gen-go@v1.36.12` installs
clean under `GOTOOLCHAIN=local` on 1.24.7, so exactly one of the two pins
carries the floor. The variable is set for both anyway: a second floor
arriving later should not need this entry read again.

**The Node 24 bump itself is verified.** Run 387 is the first completed run
carrying it. Nineteen of twenty-one jobs passed; the only remaining Node 20
deprecation warning names `actions/download-artifact@v4`, which has no Node 24
major and was deliberately left.

**The seventh invented citation, and the first no guard could catch.** The
first draft of this entry cited a `2026-09-26-read-the-runs-conclusion.md`
under `ledger/`. No such file has ever existed; the entry is
`ledger/2026-09-27-three-red-jobs-nobody-was-watching.md`. What is new is that
`check_cited_docs.py` printed `ok` over it: `source_files()` skips `docs/` and
`ledger/` **as sources**, deliberately, so a ledger entry citing a ledger entry
is read by nothing. Every earlier one in this class was caught because the
claim happened to sit in a tree something opened. Found by `ls`, which is not a
guard. Recorded below.

## What this does not do

**It does not stop the next action default from doing this.** The class is an
action changing what a pinned tool may do, and nothing here watches for it.
`CLAUDE.md`'s note is the whole mitigation, and it is a note.

**Nothing checks that a pinned generator's Go floor is met.** The pin and the
`go-version` in `.github/workflows/ci.yml` are two numbers in two files with a
relation between them that only a failing run states. A guard reading the
plugin's `go.mod` off the proxy would be a network call in a static check,
which `scripts/check.sh` does not make; reading it off a vendored copy would
be a third number to keep true.

**A ledger entry citing a ledger entry is checked by nothing.**
`check_cited_docs.py` reads `.rs`, `.py`, `.go` and `.ts` and skips `docs/` and
`ledger/`; `check_caveat_citations.py` reads one JSON file. The exemption's
stated reason is that an entry's citations are provenance — dated, and allowed
to point at something since moved — which is true of a citation that resolved
when it was written and false of one that never did. Separating them needs a
roster of the genuinely historical ones, in the `EXPECTED_REFUSALS` idiom, and
a count of how many dead citations the two trees hold today. Neither is done
here: this entry is about a red CI job, and widening a citation guard in the
same commit is how the red job waits.

**The MinIO job is still the other failure in run 387**, unchanged and for the
reason already recorded: `quay.io` refuses an anonymous pull of
`minio/minio:RELEASE.2025-09-07T16-13-09Z`. Nothing in this entry touches it.
