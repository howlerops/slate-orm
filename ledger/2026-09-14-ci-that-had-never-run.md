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

Run 4 (`09c543d`): **rust, minio, binaries, go, python, typescript, frontend,
hooks, layout** all green; the conformance runner reported `34 cases: the three
SDKs agree on all of them` inside CI. Runs 1–3 are the failures above, each
fixed and re-run rather than reasoned about.

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
