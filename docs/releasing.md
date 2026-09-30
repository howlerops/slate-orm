# Releasing

Five things ship from this repository. This is what publishes each of them,
what it costs, and what has to be decided before it can happen.

Nothing here has been published yet. The machinery exists and is exercised on
every push as far as it can be without a tag; the parts that need a registry
are described below as what they will do, not as what they have done.

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
and publishing them would commit to an API that changes every week. Four of the
thirteen already carry `publish = false`; the others are unpublished by having
no `CARGO_REGISTRY_TOKEN` rather than by declaration, which is a weaker
statement than it should be and is written up as a gap in the entry that added
this page.

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

So: bump the three files, commit, *then* tag.

```sh
python3 scripts/check_versions.py v0.2.0   # refuses until the tree agrees
```

## Cutting one

```sh
# 1. Bump. Three files, one number.
#    Cargo.toml                          workspace.package.version
#    clients/typescript/package.json     version
#    clients/python/pyproject.toml       version
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

Both are off, and off *explicitly* rather than by omission. `vars.PUBLISH_NPM`
and `vars.PUBLISH_PYPI` are repository variables, and the workflow reads three
states from each:

- **unset** — the job says so in the run summary and succeeds. This is today.
- **`true`, no credential** — the job **fails**. That is the point of the
  variable: a workflow that merely skips when a token is missing cannot tell
  "we have not decided to publish this" from "we decided to and the token
  expired", and the second reads as a green release that shipped one artefact
  fewer than it claims. A skip is green, and this repository has already been
  bitten by that once, in a Python harness that skipped its whole suite when
  `cargo` was absent.
- **`true`, with a credential** — it publishes.

To turn npm on: claim `@slate-orm` on npm, add `NPM_TOKEN` as a repository
secret, set `PUBLISH_NPM` to `true`. The publish uses `--provenance`, so a
consumer can check the tarball against the workflow run that built it.

To turn PyPI on: claim `slate-client`, configure a Trusted Publisher on PyPI
for this repository and the `pypi` job, set `PUBLISH_PYPI` to `true`. No token
is stored — the `id-token: write` permission is the whole credential.

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
  ghcr.io/<owner>/slate-serverd:0.2.0 --config /etc/slate/head.toml
```

The release pushes `linux/amd64` and `linux/arm64`, matching the binaries.

**`ci.yml` builds the image on every push**, starts it against the
configuration on the quickstart page with `--check`, and asserts the base still
has no shell. A Dockerfile first exercised on release day is exactly the shape
this repository has been burned by before — `ci.yml` itself was active,
plausible, and had run zero times.
