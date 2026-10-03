# The image cross-compiles, and both architectures are started rather than assumed

- **Date:** 2026-10-03
- **Author:** Claude Code (session: what's next, after the release)
- **Touches:** `Dockerfile`, `.github/workflows/ci.yml`, `scripts/test_check_sh.py`
- **Kind:** performance

## What changed

**The builder stage is pinned to `$BUILDPLATFORM` and cross-compiles.** It ran
under QEMU once per target before, which is why `v0.1.0`'s image job took
**76m36s** against 2m27s for the rest of the release put together.

**CI builds both architectures and starts both.** The image job was
single-architecture — the runner's — while `release.yml` pushed two, so the
multi-arch path was exercised on release day and nowhere else. It is a matrix
now, and each leg runs the same two checks the amd64 one already ran, plus a
third: that the binary inside is built for the architecture on the tin.

## Why

Two caveats from `ledger/2026-10-02-the-release-that-shipped-five-things.md`,
and they were one piece of work in the order they are written there.

The first is a measurement that was recorded and not acted on:

| | |
|---|---|
| the whole of `v0.1.0`, minus the image | 2m27s |
| the image | 76m36s |
| the aarch64 **binary**, cross-compiled, same run | 2m02s |

The third row is what makes the first two a defect rather than a cost. Same
workspace, same `--release --locked`, ~37× — the only difference is that
`release-build.yml` cross-compiles on an amd64 host and the `Dockerfile` ran
`rustc` through an emulator.

The second caveat is the coverage gap the first one was blocking. Adding
`linux/arm64` to CI was rejected on 2026-10-02 at a price of ~75 minutes on
every push to every branch, and that entry named this change as the condition
that changes the answer. It does, so the rejection is withdrawn here rather
than left standing.

**And the gap it closes is real, not tidiness.** Nothing had ever started the
arm64 half of a published index. CI starts an image it built itself, for the
runner's architecture; the attestations on the published one say how it was
built, not that it runs. `v0.1.0` worked first time and that was luck.

## The four environment variables, and why three of them fail late

`cargo` cross-compiles Rust from the target triple alone. A C build script
does not, and four dependencies here are C: `aws-lc-sys`, `ring`, `lz4-sys`
and `zstd-sys`, with `aws-lc-sys` additionally driving `cmake`. Left to
itself, the `cc` crate invokes the **host** compiler and emits x86-64 objects
that fail at link with `incompatible with aarch64` — several minutes in, with
a message about an archive member rather than about the toolchain.

So `CC_<triple>`, `CXX_<triple>`, `AR_<triple>` and
`CARGO_TARGET_<TRIPLE>_LINKER` are all set, and `strip` is taken from the
cross binutils too, because the host `strip` does not know aarch64 and says so
only after a successful build.

**The triple is spelled with underscores in those names, and that is the one
thing here that would not have survived a first run.** `cc` accepts either
spelling, so `CC_aarch64-unknown-linux-gnu` reads correctly — but it is not a
valid shell identifier, and `/bin/sh` on this base is `dash`, which rejects
the `export` outright. I wrote the hyphenated form first; `sh -n` over the
extracted `RUN` body is what caught it.

## Alternatives rejected

**Leaving it emulated and documenting the 80 minutes**, which is what
2026-10-02 did. Defensible for one release and not for a second: the cost is
paid by every future release, the fix is known, and a "budget 80 minutes" note
is the kind of documentation that trains people to ignore a hung-looking job.

**Switching rustls from `aws-lc-rs` to the `ring` backend**, which would have
removed the hardest crate to cross-compile and the `cmake` dependency with
it. Rejected because it is a *cryptography* change made for build-time
convenience, and the two should never be traded against each other casually.
If the toolchain route had not worked this would be the next thing to try,
and it would want its own entry arguing the security side on its own terms.

**One job building both platforms rather than a matrix.** Shorter, and it
cannot run either one: `load: true` takes a single platform, because a
multi-platform build produces a manifest list and no local image. Running each
image is the entire point of this change, so the matrix is forced.

**Installing QEMU on both legs.** One less conditional. Rejected because the
`if: matrix.emulated` is documentation: it says out loud that the build no
longer needs an emulator and only the *run* does. Installing it
unconditionally would hide exactly the distinction this change is about.

**Trusting a green build as proof the cross-compile worked.** It is not. A
`Dockerfile` that quietly lost its `--platform` handling still builds, still
tags the result `arm64`, and still contains an x86-64 binary — and the push
would succeed. Hence the third step, which reads the architecture back out of
the built image and compares it to the matrix entry.

**A shared buildx cache across the two legs.** The default, and wrong here:
the two builds share no compiled artefacts at all, so one `scope` means they
evict each other. Keyed per platform.

## Evidence

**Honest first: none of this was built here, and it could not be.** This
container has a `docker` binary and no daemon (`dial unix
/var/run/docker.sock: no such file or directory`), no `aarch64-linux-gnu-gcc`,
and no aarch64 rustup target. The verification is CI, which is the point of
doing T1 and T2 as one change — the thing that proves the cross-compile is the
arm64 leg that this change adds.

What *was* checked here:

| check | result |
|---|---|
| the `RUN` body parses as `sh` | passes — and caught the `export CC_aarch64-unknown-linux-gnu` bug |
| `ci.yml` parses, and the matrix is the two platforms | 24 jobs by the guard's own count, unchanged |
| `scripts/test_check_sh.py` | the new named run-block was unaccounted for; now in `ELSEWHERE` with its reason |
| `scripts/check.sh` | 95 passed |

**The job count did not move**, which is worth stating because it looks as
though it should have: `A_JOB` counts top-level `jobs:` keys, and a matrix is
one key. The *run* will show one more job than before.

**No mutation testing, and the reason is the first paragraph.** `mutate.py`
scores a mutation by running a suite; the suite for this change is a CI job
that cannot run here. The nearest honest substitute is the architecture
assertion added above, which is a permanent version of the mutation I would
have run — "build the arm64 tag from an amd64 binary and see if anything
notices". Something does now.

**No new number yet.** The predicted image time is "near the 2m02s the binary
takes", and predicting is not measuring. The measurement is this branch's CI
run and belongs in this entry's place once it exists, not before.

## What this does not do

**It does not change what `release.yml` builds.** That job already asked for
both platforms; it inherits the speedup from the `Dockerfile` and needed no
edit. Which also means the release path is still only exercised on a tag —
what changed is that CI now builds the same two images the release will.

**The arm64 image is started under emulation, not on arm64 hardware.** QEMU
running the binary is a much weaker statement than an arm64 runner would make:
it catches a wrong-architecture binary and a broken entrypoint, and it would
not catch a genuine aarch64 codegen or atomics problem. An arm64 runner is the
real answer and costs money.

**`--version` may not exist.** The architecture check runs the binary and
tolerates failure, because the assertion that matters is on the image's
declared architecture; if `slate-serverd` grows a `--version` the step starts
printing something useful and nothing else changes.

**Nothing pulls the *published* image.** Still true, and still the narrower
version of the same gap: CI now starts both architectures of an image it built
itself.
