# CI that had never run, and the eight things it found

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `.github/workflows/{ci,pages,release}.yml`,
  `scripts/check_workspace.py` (new), the three client harnesses,
  `examples/explorer/run.sh`, `site/check/quickstarts.py`, `site/index.html`,
  `crates/slate-server/src/fingerprint.rs`
- **Kind:** repair

## What changed

`.github/workflows/ci.yml` existed, was `state: active`, and had **run zero
times** — 0 runs, ever, since it was added months ago. It triggered on `main`
and on pull requests, and every branch since has been a feature branch with no
pull request open. Three ledger entries recorded "the repository has no CI",
which was wrong about the file and right about everything that matters.

It now triggers on every push and runs eleven jobs instead of two: the Rust
workspace, the three client suites, the demo frontend's unit tests, the
pre-commit hook's own tests, a workspace-layout guard, the landing page's
quickstarts, and the conformance runner and browser e2e over all three SDKs.

Two deploy workflows: Pages publishes `site/` from `main`, and a tag builds
`slate-serverd` for two targets and attaches them to a release.

## Why the trigger mattered more than the coverage

A check that has never fired has never been debugged. Everything below was
already broken, in a file everybody could see, and none of it could be found by
reading — three of the eight were environment differences that do not exist on
a developer's machine.

## What the first four runs found

**1. `bitnami/minio:latest` does not exist.** Bitnami withdrew those tags. The
very first run in this workflow's life failed on `manifest unknown`, five
seconds in, before checking anything out.

**2. Nor does `minio/minio` on Docker Hub** — `pull access denied ... repository
does not exist`. The image lives on quay.io, MinIO's own registry. Now pinned
to a release: the lesson of (1) is that an unpinned tag is one upstream
decision away from vanishing.

**3. `cargo build -p a -p b --bin x` applies `--bin` across both packages.** One
command asking for `--bin slate-serverd` built the daemon and silently skipped
the Python fixture. `upload-artifact` only errors when it finds *no* files, so
the artifact shipped half full and a job two minutes downstream failed on a
missing path. Two invocations, and a step that checks both exist.

**4. The quickstart checker depended on ambient installs.** `No module named
'grpc'`; `Cannot find module '@slate-orm/client'`. Both had been supplied by
whatever happened to be on the machine where the checker was written — which is
precisely the reader's experience it exists to model.

**5. `run.sh` waited for vite by grepping its log for `Local:`.** Vite prints
`Local` and `:` with an ANSI reset between them, so that literal never appears
when colour is on. Colour was off locally and on in CI. It polls the port over
HTTP now, which is the question the browser is about to ask anyway, and which
no amount of terminal formatting can break.

**6. All three quickstart snippets insert the same primary key into the same
node.** They are three renderings of one example, so of course they do. The
checker started them against one shared node, and the second and third got
`already_exists`. It passed locally because the three had only ever been run
one at a time with `--only`; the first run of all three together, in CI, failed
twice over. Each snippet gets its own node now — which is also the more
faithful thing, since a reader has an empty database rather than one two other
languages have written to.

**7. The Python harness *skipped* when cargo was absent.** A skip is green. In
CI that is a suite reporting success having started no server and exercised
nothing, which is the failure this client is least able to notice on its own.

**8. The landing page's install lines named packages that exist nowhere.**
`pip install slate-client`, `npm install @slate-orm/client`. The checker
executed the *code* and was satisfied, because it installs the packages by path
itself. The page now says `pip install ./clients/python` and "not on PyPI yet",
and the checker verifies that an install line naming a path names one that is
really there.

**9. `clippy::indexing_slicing`, which the workspace denies.** A
`tables[position]` written after the last local clippy run. Trivial, and the
point is the timing: the lint exists precisely so a panic cannot be introduced
by an index, and the only thing between it and `main` was a command somebody
remembered to type.

**10. Vite bound the wrong stack.** The demo's frontend printed `Local:
http://localhost:60087/` and the readiness poll timed out against
`127.0.0.1:60087` for ninety seconds. `localhost` resolves to `::1` first on
the runner, so vite listened on IPv6 only while the poll and the browser both
asked for IPv4. `--host 127.0.0.1` binds the address everything else uses.

This is the *second* bug in the same six lines: (5) was the log grep, and
fixing it uncovered this one, which the grep had been hiding by failing earlier
for an unrelated reason. A check that is wrong in two ways reports the first.

## Alternatives rejected

**Leaving the trigger on `main` and opening a pull request to test it.** Would
have proved the workflow runs on pull requests and left the eight failures to
be discovered by whoever merged first. Triggering on every push means a branch
is checked before it is proposed, which is the point.

**A `services:` container for MinIO.** The official image needs an explicit
`server /data` argument and the `services:` syntax cannot express a command —
which is why the withdrawn Bitnami image was there in the first place. `docker
run` in a step can, and it can also wait on `/minio/health/live` properly
instead of on a health-check string.

**Creating the bucket with `mc`.** Not every MinIO image bundles it, and
signing a bucket-creation request by hand in shell is worse. MinIO takes each
top-level directory under its data volume as a bucket, so `mkdir` is the whole
of it.

**Four jobs each building the daemon.** `SLATE_SERVERD` and `SLATE_TESTSERVER`
let one job build both binaries and hand them over as an artifact. That is not
mainly about time: it also means a contributor working on the Go client can run
its suite with no Rust toolchain at all, which was impossible before because
every harness shelled out to `cargo`. A path that is set and missing is a hard
error, never a fall back to building — the fallback would quietly test a
different binary from the one the caller named.

**Publishing to npm and PyPI on a tag.** Each needs a credential this
repository does not hold, a name nobody has claimed, and a decision about
stability that has not been made. The right response to "the install lines name
packages that do not exist" is to publish on purpose or to reword the page, not
to wire up a token and find out. The page was reworded; `release.yml` says so
where somebody would go looking.

## Evidence

Ten findings across six runs, each fixed and re-run rather than reasoned about.
By run 5 (`2bc9e01`) nine of the eleven jobs were green — including the
conformance runner reporting `34 cases: the three SDKs agree on all of them`
from inside CI, and the quickstarts job running all three snippets against
three separate nodes.

`scripts/check_workspace.py` refuses a crate the root workspace does not list,
a nested `[workspace]` marker, and a member whose manifest has gone. All three
demonstrated by making each mistake and watching it fail, then restoring.

`fingerprint.rs` gained the tests it never had — the alias product, the
ordering `of` depends on, and the `MAX_SPELLINGS` cliff past which a renamed
column stops answering to its old names. Three mutations, each killed:
never enumerating previous names, removing the cap, and dropping the current
name from the front of the list.

## What this does not do

Pages needs **Settings → Pages → Source: GitHub Actions** turned on by hand
once; without it the deploy step fails with a 404. Nothing deploys from a
feature branch, so that is a decision for whoever merges rather than a thing
this commit does.

`release.yml` has never run — no tag has been pushed. Its build and `--check`
steps are reachable with `workflow_dispatch` and its upload step is not, which
is deliberate, and which also means the upload is the one part of all this
still unverified. It is the same class of thing as the workflow that had never
fired, and it is worth saying so rather than implying otherwise.

The `rust` job runs `cargo test --workspace`, which this container cannot do —
the disk allowance is smaller than the build. Every crate has been verified
here in groups, and in CI as one command; those are different things and only
the CI one is the real answer.

Nothing checks the *aarch64* release binary beyond compiling it: a
cross-compiled binary cannot be run on the runner that built it, and adding an
emulator to find out is more machinery than a `--check` on an unreleased
project earns.

## One more, found by the hook while writing this entry

The pre-commit hook refused this file, reporting that it "has `## Why` with
nothing under it". The section is headed `## Why the trigger mattered more than
the coverage` and has four sentences under it.

The hook looked for a heading with an unanchored `grep -F` — substring, so it
found it — and then extracted the body with `awk '$0 == h'` — exact, so it
found nothing. Two different notions of "the heading is here", one line apart,
and the failure surfaces as a message describing a state the file is not in.

Both are prefix matches now, and the suite gained two cases: a heading that
says more than the bare word is accepted, and reverting the `awk` half fails
them. The `^` anchor on the `grep` half is not covered and cannot be: without
it a line of *prose* mentioning `## Why` passes the presence check, the body
extraction finds nothing anyway, and the commit is refused either way — the
same outcome with a worse message. Recorded rather than dressed up as a
mutation that was killed.

## The release workflow, and a tag this session could not push

The user asked for the release to be verified end to end by pushing `v0.0.1`.
It could not be done from here: `git push origin refs/tags/v0.0.1` is refused
with **HTTP 403**, and `workflow_dispatch` on `release.yml` is refused the same
way. The session's credentials are scoped to its designated branch; tags and
dispatches are not in scope. The agent proxy reports healthy with no relay
failures, so this is the remote refusing the ref rather than a transport
problem. The local tag was deleted rather than left behind to imply a release
exists.

What was done instead, because "verify it" is the request and the tag was only
the means: `ci.yml` **calls** `release.yml` as a reusable workflow on every
push, so the build half runs continuously. The `release` job — the one that
publishes — stays gated on `refs/tags/v*`, and `github.ref` inside a called
workflow is the caller's ref, so on a branch push it is skipped.

The first attempt at this was `on: push: paths:
[".github/workflows/release.yml"]`, and it is worth recording why that is
wrong, because it looked right and did nothing. A `push` block with `tags:`
set matches *only* tags, so adding `paths:` next to it adds no branch pushes —
the run simply never appeared. Worse, the filter would then have applied to the
tag push itself, so tagging a commit that happened not to touch that file would
have built and published nothing. A release workflow that silently does not run
is strictly worse than one that has never run, which is the thing this entry is
about.

This is the same lesson the rest of this entry is about, applied before it
bites. A release workflow otherwise runs for the first time on the day somebody
cuts a release, which is the worst possible day to find out the aarch64 linker
is not installed. `ci.yml` was in exactly that state — active, plausible, never
run — and its first real run died in five seconds. Now the half that can be
checked without publishing is checked continuously.

Calling it from CI then failed a third time, and this one is worth recording
because the failure mode is silent by design: `startup_failure`, **zero jobs**,
no logs to read. A reusable workflow's jobs may not request more permission
than the job calling them was granted, and the publish job asks for `contents:
write` while the CI job calling it had the default. GitHub rejects the run
before it starts rather than skipping the offending job — even though that job
is gated off on a branch push and would never have run.

The fix is not to hand every CI run a repository-write token. The build moved
into `release-build.yml`, which needs no write at all and which both `ci.yml`
and `release.yml` call; `release.yml` keeps the `publish` job and its
`contents: write`. The split is real rather than a workaround — "how the
binaries are built" and "how a release is published" are different questions,
and only the first is safe to answer on every branch push.

**What remains unverified, precisely:** the `softprops/action-gh-release` step.
Everything before it — both targets compiling, the cross toolchain, the
TOML-extraction from `site/index.html`, `--check` against the shipped binary,
and the artifact naming — runs on a real runner on every push. Pushing `git tag -a v0.0.1 -m
... && git push origin v0.0.1` from a checkout with tag permission is the one
command that closes the gap, and it publishes a public prerelease, which is why
it is a person's decision rather than this session's.
