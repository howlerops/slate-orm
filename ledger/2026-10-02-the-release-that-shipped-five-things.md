# The release that ran four jobs nobody had ever run, and the 77 minutes one of them took

- **Date:** 2026-10-02
- **Author:** Claude Code (session: cut a release)
- **Touches:** `docs/releasing.md`, `README.md`
- **Kind:** docs

## What changed

`v0.1.0` is published. `release.yml` ran end to end for the first time — four
of its six jobs had never executed at all — and all nine jobs passed. This
entry records what shipped, verified against the registries rather than read
off the run, and the one number worth keeping: **the container image took 77
minutes against 2m27s for everything else.**

`docs/releasing.md` and `README.md` now say what happened instead of what was
planned.

## Why this is its own entry

`ledger/2026-10-02-the-version-nobody-moved.md` is the bump, and it ends by
saying the result belongs in an entry written from the run rather than from
the plan. This is that entry, and the reason for the split is the thing it
found: a release workflow's behaviour is not knowable from reading it.
`release.yml`'s own closing comment says as much —

> Unverified, and deliberately so: everything that needs a real `v*` tag […]
> That is the one part that has to be exercised by a person who means it.

Four jobs were in that state. Writing them up before the tag would have been
describing a plan in the past tense, which is the failure the page it
corrects had already committed once.

## What shipped

Checked by asking, not by reading the run summary. The distinction matters:
a green job means the step exited zero, and the question is whether the
artefact is there.

| artefact | evidence |
|---|---|
| binaries | the release carries `slate-serverd-x86_64-unknown-linux-gnu` (24.1 MB) and `-aarch64-` (22.3 MB), prerelease, not draft |
| the image | an anonymous token against `ghcr.io` resolves `0.1.0` **and** `latest` to the same OCI index, `sha256:52e2cd8e…` |
| its architectures | that index lists `linux/amd64` and `linux/arm64`, plus two `attestation-manifest` entries — the `provenance` and `sbom` flags, present rather than assumed |
| the Go module | `refs/tags/clients/go/v0.1.0` exists, and the job's own `proxy.golang.org` probe answered for it |
| npm | *not published*, reported in the summary, job green — `vars.PUBLISH_NPM` unset |
| PyPI | the same, `vars.PUBLISH_PYPI` |

The two "not published" results are the design working rather than a
shortfall. That tri-state — unset says so and succeeds, set-without-a-token
**fails** — exists because a skip is green, and this release is the first
evidence that the middle branch is reachable from a real run rather than from
reading the `if:`.

`0.0.1` is not a tag in the registry, which confirms from the other side that
the image job never ran for the first release.

## The 77 minutes

| job | wall clock |
|---|---|
| the tag and the tree agree | 5s |
| build / x86_64 | 1m47s |
| build / aarch64 | 2m02s |
| attach the binaries | 11s |
| the Go module, for the proxy | 14s |
| npm / PyPI | 5s, 7s |
| **the head node, as an image** | **76m36s** |
| what this release actually shipped | 3s |

One `docker/build-push-action` step, 22:21:13 to 23:37:16. The whole rest of
the release finished at 22:23:12.

**The cause is not a mystery and the two numbers above state it.** The
aarch64 *binary* takes 2m02s because `release-build.yml` cross-compiles it on
an amd64 host with the aarch64 toolchain. The aarch64 *image* compiles the
same workspace under QEMU, so every instruction of `rustc` is emulated. Same
code, same profile, ~37× the time.

Nothing is broken, and it is still worth writing down, because for 77 minutes
a release that had already published four of five artefacts looked
indistinguishable from one that had hung. I said so at the time rather than
guessing a finish time, which is the only thing that made the wait readable.

## Alternatives rejected

**Cross-compiling in the `Dockerfile`** — `--platform=$BUILDPLATFORM` on the
builder stage, the aarch64 target added, the linker configured, and the
runtime stage staying `TARGETPLATFORM`. This is the fix, it would turn 77
minutes into something near the 2m02s the binary already takes, and it is not
in this change because the release was the task and a build-strategy change
to the one file that produces the shipped artefact is not a footnote to it.
It wants its own entry, its own CI run and its own `--check` against the
started container. Recorded as a caveat rather than done.

**Adding `linux/arm64` to `ci.yml`'s image build**, so the multi-arch path is
not release-day-only. Correct in principle and wrong at this price: it adds
~75 minutes to *every push to every branch*, for a path that changes when the
`Dockerfile` does and almost never otherwise. If the cross-compile above
lands, this becomes cheap and should be reconsidered then — which is the
honest order, since the objection is entirely about cost.

**Saying nothing about the duration.** It is not a defect: the job succeeded
and the artefact is correct. Written up because the next person to cut a
release will watch it sit for an hour, and the choice is between them finding
this paragraph and them cancelling the run.

**Re-reading the run's own summary table instead of the registries.** Cheaper
and strictly weaker. `softprops/action-gh-release` and `docker/build-push-action`
report what they attempted; the index digest reports what is there. The two
agreed, which is the result, not the assumption.

## Evidence

**Run 2 of `release.yml`**, on `419378d`, nine jobs, all `success`, 22:20:45
to 23:37:39 UTC.

**CI run 552 on `main`** was green on the same commit before the tag was
pushed, which is what made the tag safe to cut.

**`scripts/check_versions.py v0.1.0`** agreed with the tree before the tag
existed, which is the check that cannot be undone afterwards.

**No mutation testing**, and that is the correct answer rather than a gap:
this change is two prose files. The code it describes — the version guard and
its inheritance rule — was mutated five ways in the entry beside this one.

**Not measured, and worth naming as such.** Nothing here timed the image at a
second architecture count, or on a second runner, or twice. 76m36s is one
observation of one job, so it is a number to budget against and not a
benchmark. The 2m02s cross-compiled binary beside it is what makes the
*ratio* believable rather than the absolute.

## What this does not do

**The image is still only built two-architecture on a tag.** Argued above;
the exposure is that a `Dockerfile` change which breaks only the arm64 leg is
invisible until somebody releases.

**Nothing pulls the published image and runs it.** `ci.yml` builds an image
and starts it with `--check`, which is a different artefact from the one in
the registry — same `Dockerfile`, different build, and the arm64 half of the
published index has never been started anywhere. The attestations say how it
was built, not that it works.

**npm and PyPI remain unexercised.** Both jobs ran and both correctly did
nothing. The first real publish to either is still a first run, with the
same cannot-be-undone property, and the tri-state's *failing* branch — set
with no credential — is still only reachable by reading the `if:`.

**The 77 minutes is recorded, not fixed.**
