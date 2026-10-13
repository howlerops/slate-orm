# Releasing

Five things ship from this repository. This is what publishes each of them,
what it costs, and what has to be decided before it can happen.

**Four releases have been cut. The second exercised every job; the third is
the first that published all five artefacts; the fourth moved them onto
SlateDB 0.17.**

`v0.0.1`, on 2026-09-14, attached two `slate-serverd` binaries to a GitHub
prerelease and did nothing else — the image, the Go module tag and the two
registry jobs did not exist yet; they were added on 2026-09-29 along with this
page.

`v0.1.0`, on 2026-10-02, ran every job below for the first time. What it
shipped, verified by asking the registries rather than by reading the run:

<!-- not a measurement -->

| artefact | result |
| --- | --- |
| binaries | attached — x86_64 24.1 MB, aarch64 22.3 MB |
| `ghcr.io/howlerops/slate-serverd` | `0.1.0` and `latest`, one OCI index, both resolving to `sha256:52e2cd8e…` |
| the Go module | `clients/go/v0.1.0` tagged, and `proxy.golang.org` answered for it |
| npm | not published — `vars.PUBLISH_NPM` unset, reported and succeeded |
| PyPI | not published — `vars.PUBLISH_PYPI` unset, reported and succeeded |

The index carries `linux/amd64` and `linux/arm64` plus two attestation
manifests, which are the `provenance: true` and `sbom: true` below. An
anonymous `GET` against `ghcr.io` resolves it, so the package is public.

`v0.2.0`, on 2026-10-13, is the first with npm and PyPI turned on. Again
verified against the registries, and the two client packages also installed
from them into empty directories and imported:

<!-- not a measurement -->

| artefact | result |
| --- | --- |
| binaries | attached — x86_64 24.3 MB, aarch64 22.4 MB |
| `ghcr.io/howlerops/slate-serverd` | `0.2.0` and `latest`, both `sha256:ac6ba82a…`, `linux/amd64` and `linux/arm64` plus two attestations; `0.1.0` still `sha256:52e2cd8e…` |
| the Go module | `clients/go/v0.2.0`, listed by `proxy.golang.org` |
| npm | `@slate-orm/client@0.2.0`, `latest`, with a provenance statement in the Sigstore log; not served until some minutes after the job reported success |
| PyPI | `slate-client` 0.2.0, a wheel and an sdist, through the Trusted Publisher |

The npm job's success and the package being installable are two moments, not
one: `npm view` answered 404 at 17:29:49, 22 seconds after the job logged
`+ @slate-orm/client@0.2.0` and "may take a few minutes to become available".
It was serving the package within a few minutes after that; when exactly was
not timed. A check run straight after the job would have reported a release
that had in fact shipped as missing.

`v0.2.1`, also on 2026-10-13, exists to move every artefact onto SlateDB 0.17,
whose descending-scan fix is in
`ledger/2026-10-13-slatedb-0-17-and-the-descending-scan.md`. Verified the same
way, including the two client packages installed into empty directories and
imported:

<!-- not a measurement -->

| artefact | result |
| --- | --- |
| binaries | attached — x86_64 24.5 MB, aarch64 22.6 MB |
| `ghcr.io/howlerops/slate-serverd` | `0.2.1` and `latest`, both `sha256:1b1137c9…`, `linux/amd64` and `linux/arm64` plus two attestations; `0.2.0` still `sha256:ac6ba82a…`. The amd64 binary inside names `slatedb-0.17.0` and no other version |
| the Go module | `clients/go/v0.2.1`, listed by `proxy.golang.org` |
| npm | `@slate-orm/client@0.2.1`, `latest`, with provenance |
| PyPI | `slate-client` 0.2.1, a wheel and an sdist |

The whole run took 8m17s, from 21:44:37 to 21:52:54; the image job was
7m57s of it. The npm package was already being served when this table was
checked, a few minutes after its job ended.

This page said "nothing here has been published yet" for three days after that
was false, which is worth more than an apology: it was written *about* the
registry jobs, which genuinely had not run, and the binaries that had shipped
two weeks earlier were simply not in the author's head. A sentence about
"nothing" is the easiest kind to get wrong, because checking it means looking
everywhere rather than at the thing you are writing about.

## What ships

| artefact | where | what it needs |
| --- | --- | --- |
| `slate-serverd-x86_64-unknown-linux-gnu`, `…-aarch64-…` | the GitHub release | nothing |
| `ghcr.io/<owner>/slate-serverd` | GHCR | nothing — `GITHUB_TOKEN` is enough |
| `github.com/howlerops/slate-orm/clients/go` | the Go proxy | nothing — a git tag |
| `@slate-orm/client` | npm | `vars.PUBLISH_NPM` and `secrets.NPM_TOKEN` |
| `slate-client` | PyPI | `vars.PUBLISH_PYPI` and a Trusted Publisher |

The Rust crates are **not** published to crates.io. `slate-orm`, `slate-kernel`
and the rest are libraries this repository consumes through path dependencies,
and publishing them would commit to an API that changes every week. All
fourteen members now carry `publish = false` and `scripts/check_workspace.py`
refuses one that does not say so — when this page was written four of them
said it and the rest were unpublished only by having no
`CARGO_REGISTRY_TOKEN`, which is a weaker statement than it looks:
`publish` defaults to **true**, so silence is the publishable answer.
`ledger/2026-10-01-a-default-nobody-chose.md` is the entry that closed it.

## One version, everywhere

`Cargo.toml`'s `workspace.package.version` is the number. Every crate inherits
it, `clients/typescript/package.json` and `clients/python/pyproject.toml`
repeat it, and the container image and the Go module tag take it from the tag.

`scripts/check_versions.py` holds them together, and runs in
`scripts/check.sh` and as the first job of a release. It is there because of
the one release failure that cannot be undone: a tag named `v0.2.0` on a tree
whose `package.json` still says `0.0.1` publishes **`0.0.1`** — successfully,
and to registries that refuse a reused number. There is no fix after the fact,
only an explanation.

It also holds the Rust side together from the other direction: every
workspace member must say `version.workspace = true` rather than write a
number of its own. Two of fourteen did — `slate-wasm` and
`clients/python/testserver` — and would have sat at `0.0.1` inside a `0.1.0`
release. Neither is published, so nothing would have broken; the workspace
would simply have reported two numbers and the next crate would have copied
whichever manifest its author opened.

So: bump the three files, commit, *then* tag.

```sh
python3 scripts/check_versions.py v0.2.0   # refuses until the tree agrees
```

## Cutting one

```sh
# 1. Bump. Three files, one number — and the things that track it:
#    Cargo.toml                          workspace.package.version, and the
#                                        `path = ..., version = ...` requirements
#                                        beside it (cargo refuses a path whose
#                                        version does not satisfy them)
#    crates/slate-serverd/Cargo.toml     its own `slate-server` requirement,
#                                        the one member that pins a path
#                                        dependency outside the workspace table
#    clients/typescript/package.json     version, and package-lock.json's two
#    clients/python/pyproject.toml       version
#    Cargo.lock                          `cargo metadata` rewrites it
python3 scripts/check_versions.py        # the tree agrees with itself

# 2. Commit it, with a ledger entry like any other change.

# 3. Tag, and check the tag against the tree before pushing it.
python3 scripts/check_versions.py v0.2.0
git tag v0.2.0 && git push origin v0.2.0
```

`release.yml` then runs, and its last job writes a table into the run summary
saying what each artefact did. That job **fails the run** if any of them did
not succeed, because a release that shipped four of five things and reported
success is the failure this is all arranged around.

## Turning npm and PyPI on

Both have been **on since 2026-10-13**, and before that were off *explicitly*
rather than by omission. `vars.PUBLISH_NPM` and `vars.PUBLISH_PYPI` are
repository variables, and the workflow reads three states from each:

- **unset** — the job says so in the run summary and succeeds. This was every
  release before `v0.2.0`.
- **`true`, no credential** — the job **fails**. That is the point of the
  variable: a workflow that merely skips when a token is missing cannot tell
  "we have not decided to publish this" from "we decided to and the token
  expired", and the second reads as a green release that shipped one artefact
  fewer than it claims. A skip is green, and this repository has already been
  bitten by that once, in a Python harness that skipped its whole suite when
  `cargo` was absent.
- **`true`, with a credential** — it publishes.

**Neither can be switched on from inside this repository.** Both need an
account action and a credential, so the exact steps are written out rather
than described.

To turn npm on:

1. Claim the `@slate-orm` scope on npm. `curl -s -o /dev/null -w '%{http_code}'
   https://registry.npmjs.org/@slate-orm/client` answers `404` while it is
   free.
2. Create an **automation** token (classic, or a granular token with publish
   on that scope) and add it as the repository secret `NPM_TOKEN`.
3. Set the repository variable `PUBLISH_NPM` to `true`.

The publish uses `--provenance`, which requires the manifest's `repository`
field to name this repository — it does, and that is not decoration: without
it the publish fails rather than publishing unattested.

To turn PyPI on:

1. Claim `slate-client` on PyPI.
2. Add a Trusted Publisher under the project's *Publishing* settings, with
   exactly these four fields — they are matched against the OIDC token and a
   mismatch reads as `invalid-publisher`:

   | field | value |
   | --- | --- |
   | Owner | `howlerops` |
   | Repository name | `slate-orm` |
   | Workflow name | `release.yml` |
   | Environment name | *(leave empty — the `pypi` job declares none)* |

3. Set the repository variable `PUBLISH_PYPI` to `true`.

No token is stored for PyPI: the `id-token: write` permission on the job is
the whole credential.

**Then cut a tag.** Both publish jobs are gated on `startsWith(github.ref,
'refs/tags/v')`, so a `workflow_dispatch` run builds and checks and publishes
nothing. `0.1.0` was released without them, and a version number cannot be
reused on either registry, so the first run to ship to npm and PyPI was the
next tag, `v0.2.0`. The owner's account steps for both, done that day, are the
ones written above.

**What the package actually contains is checked before it is sent.**
`scripts/check_npm_package.py` asks npm what the tarball holds and refuses if
a declared entry point is missing from it. It runs on every push and again as
the last step before `npm publish`, because the npm package would have
shipped with no JavaScript in it at all until 2026-10-03 — see
`ledger/2026-10-03-the-package-that-would-have-shipped-empty.md`.

## The Go module, which is only a tag

`clients/go` is a module in a subdirectory, so
`github.com/howlerops/slate-orm/clients/go@v0.2.0` resolves from a tag named
**`clients/go/v0.2.0`** and from nothing else. The `v0.2.0` tag that triggers
the release does not resolve it.

The release creates that tag and then asks `proxy.golang.org` for the version,
retrying while the proxy catches up. The request is not politeness: it is the
only available check that the tag has the shape the proxy wants, and a wrong
one answers 404 in the release rather than in somebody's terminal a week later.

## The image

`Dockerfile` builds `slate-serverd` with `--locked` and copies it onto
`gcr.io/distroless/cc-debian12` — the glibc runtime and its CA certificates and
nothing else. The certificates are load-bearing: the object store is reached
over HTTPS, and a container without them fails at the first `PutObject` with a
message about a certificate rather than about a bucket.

There is no `CMD`. `slate-serverd` requires `--config`, so the whole interface
is

```sh
docker run --rm -v "$PWD/head.toml:/etc/slate/head.toml:ro" \
  ghcr.io/<owner>/slate-serverd:0.2.1 --config /etc/slate/head.toml
```

The release pushes `linux/amd64` and `linux/arm64`, matching the binaries.

**It used to take 77 minutes. It does not any more, and the history is worth
keeping.** `v0.1.0`'s image job took **76m36s** against 2m27s for the whole
rest of the release, because the builder stage ran `rustc` under QEMU once per
target. The `Dockerfile` cross-compiles now — `--platform=$BUILDPLATFORM` on
the builder, the C toolchain for the target, `strip` from the cross binutils
— and CI run 556 measured the arm64 build at **4m31s**, cold-cached. Seventeen
times faster, and the two legs run in parallel, so a release's image step is
about five minutes rather than an hour and a quarter. `v0.2.0` confirmed it on
a real release: the image job ran **6m48s**, from 17:27:01 to 17:33:49.

`ledger/2026-10-03-the-image-stops-emulating-and-starts-being-run.md` has the
measurement and the four environment variables it turns on.

**`ci.yml` builds the image on every push**, starts it against the
configuration on the quickstart page with `--check`, and asserts the base still
has no shell. A Dockerfile first exercised on release day is exactly the shape
this repository has been burned by before — `ci.yml` itself was active,
plausible, and had run zero times.

**And it builds both architectures now, and starts both.** That was not true
until 2026-10-03: CI built the host's amd64 while the release pushed two, so
the multi-arch push was a release-day-only path — the very shape the paragraph
above is about, one level in. It worked first time on `v0.1.0` and that was
luck rather than coverage. The image job is a matrix now; each leg starts its
image against the quickstart's TOML and asserts the binary inside is built for
the architecture on the tin, because a `Dockerfile` that lost its `--platform`
handling would still build, still tag the result `arm64`, and still contain an
x86-64 binary.
