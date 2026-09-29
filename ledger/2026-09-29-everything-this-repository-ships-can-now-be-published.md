# Five artefacts ship; three needed nothing, two needed a decision made visible

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `.github/workflows/{release,ci}.yml`, `Dockerfile`,
  `.dockerignore`, `scripts/{check_versions,test_check_versions}.py`,
  `crates/*/Cargo.toml`, `clients/python/testserver/Cargo.toml`,
  `scripts/{check.sh,mutations.json,test_check_sh.py}`, `docs/releasing.md`,
  `CLAUDE.md`
- **Kind:** feature

## What changed

`release.yml` publishes five things where it published one:

| artefact | where | what it needed |
| --- | --- | --- |
| two `slate-serverd` binaries | the GitHub release | nothing; was already there |
| `ghcr.io/<owner>/slate-serverd` | GHCR, amd64 and arm64 | a `Dockerfile` |
| `github.com/howlerops/slate-orm/clients/go` | the Go proxy | a `clients/go/vX.Y.Z` tag |
| `@slate-orm/client` | npm | a token and a claimed name |
| `slate-client` | PyPI | a Trusted Publisher and a claimed name |

New: `Dockerfile` and `.dockerignore`; `scripts/check_versions.py` and its
suite; a `versions` job every other release job waits on; a `shipped` job that
writes what each artefact did and **fails the run** if any did not succeed; and
`docs/releasing.md`. `ci.yml` grew an `image` job so the Dockerfile is built,
started and checked on every push. Twenty-two jobs became twenty-three, and
`CLAUDE.md`'s two copies of that count were corrected.

## Why

The previous version of this file refused to publish the client packages, with
a reason that was right at the time:

> Each needs a credential this repository does not hold, a name nobody has
> claimed, and a decision about stability that has not been made. The landing
> page's install lines used to name packages that do not exist anywhere; the
> fix for that was to reword the page, not to wire up a token and find out.

Two of the three obstacles are still there and are not waved away here. What
changed is that "we have not decided" is now a *state the repository holds*
rather than a paragraph in a comment: `vars.PUBLISH_NPM` unset means the job
says so in the run summary, and set-without-a-credential means the job **fails**.

That middle state is the whole design. The obvious spelling is
`if: secrets.NPM_TOKEN != ''`, and it cannot tell "we have not decided to
publish this" from "we decided to and the token expired" — the second reads as
a green release that shipped four artefacts and reported five. **A skip is
green** is a defect this repository has already met, in a Python harness that
skipped its whole suite whenever `cargo` was absent, and the lesson was written
into `CLAUDE.md`: *prefer a hard error to a skip whenever the thing being
skipped is the point.*

The other three needed no decision at all, which is worth saying plainly
because the old comment lumped all four together. GHCR takes `GITHUB_TOKEN`,
which the workflow already has. The Go module needs a git tag and nothing else
— there is no registry, no account and no name to claim, and `go get` on the
module has been failing for want of a tag somebody could have pushed at any
time. Those two were not blocked on a credential; they were blocked on nobody
having looked.

**And the thing that had to exist before any of it.** Five artefacts carry a
version number and four of them carry it by hand. A tag reading `v0.2.0` on a
tree whose `package.json` still says `0.0.1` publishes `0.0.1` — successfully,
to registries that refuse a reused number. There is no correction after the
fact, only an explanation. `scripts/check_versions.py` is the first job of a
release and runs in `check.sh`, so the tree is checked against itself on every
push and against the tag on every release.

## Alternatives rejected

**`if: secrets.NPM_TOKEN != ''`.** Covered above: it collapses two states that
must stay apart. It is also subtly worse than it looks — the `secrets` context
is not reliably available in a job-level `if`, so the natural spelling of the
wrong idea does not even work, and the version that does work is a step-level
expression that reads as if it were a guard.

**Publish on a tag with no version check.** What every project does, and it is
fine right up until the day it is not. The failure is silent, immediate and
irreversible; a guard that costs one `tomllib.load` is not a trade worth
thinking about.

**Independent versions per package.** The grown-up arrangement, and unearned
here: the three clients are generated from one proto, tested against one
server by one conformance runner, and released together or not at all. One
number means a reader pairs any client with any server by looking, and it means
the check is a comparison rather than a compatibility matrix. When that stops
being true the answer is a matrix and a different guard, not a version that
drifts quietly.

**`debian:slim` or `scratch` for the image.** `scratch` needs a static binary
this repository does not build — the release targets are `*-linux-gnu` and
nothing has been built or tested against musl. `debian:slim` ships a shell and
a package manager to a container whose only job is to listen on one port.
`gcr.io/distroless/cc-debian12` is the glibc runtime and its CA certificates;
the certificates are the part that matters, because the object store is HTTPS
and a container without them fails at the first `PutObject` with a message
about a certificate rather than about a bucket. There is a CI assertion that
the base still has no shell, because "distroless" is a property of the image
and not of the `FROM` line somebody wrote once.

**Build the image only on a release.** Rejected for the reason this repository
has the most direct evidence for: `ci.yml` was active, plausible and had run
**zero times**, and turning it on found eleven defects in a morning. A
Dockerfile first exercised on release day is that shape exactly. It costs a few
minutes a push and it is one architecture, not two — the release builds both,
and the aarch64 *binary* is already built by `release-build.yml`.

**Publish the Rust crates to crates.io.** Rejected, and written into
`docs/releasing.md` rather than left silent: they are libraries this repository
consumes by path, and publishing them commits to an API that changes weekly.
Four of the thirteen already said `publish = false`; the other nine were
unpublished only by there being no `CARGO_REGISTRY_TOKEN`, which is not a
decision — somebody adding a token for one crate would publish nine. All
thirteen say it now, each with the reason above.

**Add `docker pull` and `npm install` lines to the site.** Tempting once the
machinery exists. Rejected because it would repeat the exact defect the old
comment cites: the landing page once named packages that did not exist. Nothing
has been published, so nothing on the site says it has. `docs/releasing.md`
describes what *will* publish, in those words.

## Evidence

- `sh scripts/check.sh`: **87 passed, all of them**, exit 0 — from 85, the two
  new steps being `versions` and `versions-guard`.
- `python3 scripts/check_versions.py`: `0.0.1` in `Cargo.toml`,
  `clients/python/pyproject.toml` and `clients/typescript/package.json`. With
  `v0.0.1`: passes. With `v0.2.0`: refuses, naming both numbers.
- `python3 scripts/test_check_versions.py`: **9 passed, 0 failed.**
- **The Dockerfile's `COPY` set, verified without a daemon.** There is no
  docker daemon in this container — the CLI is there and
  `/var/run/docker.sock` is not — so the image could not be built here. What
  *was* checked is the failure most likely to bite: the build context was
  reconstructed exactly as the `COPY` lines describe it, and
  `cargo metadata --locked --no-deps` resolved the workspace inside it. A
  missing member manifest or an unsatisfiable lockfile would have failed there.
  The image itself is unbuilt and is recorded below as such.
- **Nine mutations, all caught**, in four runs:
  - `ledger/mutations/20260929T172120-scripts-check-versions-py.json` — six
    against the fixture suite: a manifest with no version accepted; the tag no
    longer compared; anything accepted as a v-tag; an empty manifest set
    reading as nothing wrong; and each client manifest dropped in turn.
  - `ledger/mutations/20260929T172111-clients-typescript-package-json.json` —
    the real `package.json`'s version drifted from the workspace. Caught. This
    is the one that matters: the fixtures prove the comparison refuses, and
    only the real manifest proves the guard reads the file a release publishes.
  - `ledger/mutations/20260929T172655-claude-md.json` — `CLAUDE.md`'s job count
    put back to twenty-two. Caught by `test_check_sh.py`, which is the guard
    that exists because that sentence said *seventeen* for months.
  - `ledger/mutations/20260929T172656-github-workflows-ci-yml.json` — the image
    job's shell assertion renamed, so nothing rosters it. Caught.
- `python3 scripts/check_mutations_roster.py`: **30 guards, 28 with a real-tree
  mutation, 1 with a written reason for having none.**

## What this does not do

**Nothing has been published.** Not one of the five. Everything above is
machinery and every check on it is a check on the machinery; the first real
release is the first time `softprops/action-gh-release`, the GHCR push, the
module tag and the two registry uploads run at all. That is unavoidable — a tag
that publishes cannot be rehearsed — and it is why as much as possible was
moved into `ci.yml`, where it runs on every push.

~~**The image has never been built.** The `COPY` set resolves and the workspace
is `--locked`-satisfiable, which is a real check and is not the same as an
image. `rust:1-bookworm` may not have the toolchain the workspace's
`rust-version = "1.90"` wants; the distroless base may lack something
`slate-serverd` links; `strip` may not be on the build image.~~ **Answered by
the first push**, which is the soonest anything here could find out: CI run
496, twenty-three jobs green, the `container image` job building the file,
running the image against the quickstart page's TOML with `--check`, and
confirming the base still has no shell. All three guesses were wrong in the
comfortable direction. It runs on every push, so this is re-taken rather than
asserted.

**The image has never been built.** The `COPY` set resolves and the workspace
is `--locked`-satisfiable, which is a real check and is not the same as an
image. `rust:1-bookworm` may not have the toolchain the workspace's
`rust-version = "1.90"` wants; the distroless base may lack something
`slate-serverd` links; `strip` may not be on the build image. All three would
show as a red `image` job on the first push of this branch, which is the
soonest anything here can find out, and none of them is a guess this container
could have settled.

> The paragraph above is the original, kept standing under its own strike —
> the convention three other entries here use. It was true for about forty
> minutes and it is the honest record of what was and was not known when the
> work was pushed, which is the part worth keeping. The tracker reads this
> copy; the strike says what happened to it.

**Nothing checks that `check.sh` and `ci.yml` cover the same guards.**
`test_check_sh.py` reads one direction — every workflow step is in the script
or rostered as one it cannot run — and nothing reads the other. Found the same
way the line above was: `check_versions.py` was added to `check.sh` and not to
`ci.yml`, and it took reading run 496's step list to notice that the release's
new guard was not running in CI at all. It is there now; the missing rule is
not.

**`latest` points at a prerelease.** Deliberate — there are no stable releases
and a `latest` that resolves to nothing is worse than one that resolves to a
prerelease the README describes as such — but it is a promise a reader may not
expect, and nothing enforces that the README keeps saying so.

**Nothing stops the *next* crate defaulting to publishable.** All thirteen
members now declare `publish = false` — the nine that were held back only by
the absence of a token say so themselves — but a fourteenth added tomorrow
defaults to publishable, and the only thing between it and crates.io is that
nobody has added a token. A guard over `scripts/check_workspace.py`'s member
list would settle it; the declaration was the cheap half and is what was done.

**The version guard reads two manifests.** `Cargo.toml`, `package.json` and
`pyproject.toml`. If a fourth publishable thing arrives with its own version —
a Helm chart, a second npm package, a Homebrew formula — it is unchecked, and
the guard's never-fires case only catches *both* of the current two
disappearing. This is the roster problem again, and unlike
`check_proto_copies.py` there is no tree to walk: the three manifests have
three different formats and are found by name.

**Nothing checks the release workflow's own shape.** `test_check_sh.py` knows
that its steps exist and are rostered; nothing asserts that the `shipped` job
lists every publishing job, so a sixth artefact added without a row would
publish and go unreported. The table is hand-written, which is the failure mode
this repository keeps finding in other files.

**The go tag is created by the release, so a re-run is not idempotent in the
useful direction.** It skips a tag that already exists, which is right for a
re-run of the same release — but it means a tag pointing at the wrong commit
has to be deleted by hand before a corrected release can push the right one,
and nothing says so at the point of failure.
