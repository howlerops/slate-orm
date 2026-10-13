# v0.2.0 shipped all five artefacts, the first release to reach npm and PyPI

- **Date:** 2026-10-13
- **Author:** Claude Code (session: "ok, let's do those", and the owner's account steps)
- **Touches:** `docs/releasing.md`, `docs/agent-handoff.md`, `README.md`
- **Kind:** docs

## What changed

The docs that describe what has shipped now describe `v0.2.0`:
- `docs/releasing.md` has a table of what reached each registry, the image
  step's measured time, and the npm-visibility lag;
- `docs/agent-handoff.md` no longer says the names are unclaimed and the
  publish jobs off;
- the README's release bullet says the third release shipped all five.

The release itself is not in the diff. The repository variables are not, the
tag is a ref, and the account steps were the owner's.

## Why

The owner asked whether every client was available for use. npm and PyPI were
not (`ledger/2026-10-05-version-0-2-0.md`). In this session the owner:
- created the `@slate-orm` npm org and an automation token, stored as
  `secrets.NPM_TOKEN`;
- added a pending Trusted Publisher for `slate-client` on PyPI.

Then I set `PUBLISH_NPM` and `PUBLISH_PYPI` to `true`, checked the tree
against the tag with `check_versions.py v0.2.0`, and pushed `v0.2.0` from
`7b0f4b4`, which is `main`.

The docs had to follow at once. `docs/releasing.md` records that it said
"nothing here has been published yet" for three days after binaries had
shipped. Leaving it saying npm and PyPI are off would be the same mistake
again.

## Alternatives rejected

- **Tag before the account steps, and publish npm and PyPI at 0.2.1.** That
  was offered, and the owner chose the walkthrough. One tag shipped everything
  at one number, and the release's last job reported on all five together.
- **Trust the run summary for the docs' table.** Every row comes from asking
  the registry: the GHCR tag list and digests, the Go proxy's `@v/list`,
  `npm view`, PyPI's JSON API, and `gh release view`. The v0.1.0 run had
  shown npm and PyPI jobs green while shipping nothing, so a green job is not
  evidence of a package.

## Evidence

Release run 38438705962: all 10 jobs `success`.
- **npm:** `@slate-orm/client@0.2.0`, `dist-tags.latest = 0.2.0`. The job log
  shows "Signed provenance statement" and a Sigstore log index. `npm view`
  answered 404 at 17:29:49, 22 s after the publish line, and served it
  minutes later (not timed).
- **PyPI:** `slate-client` 0.2.0, files `slate_client-0.2.0-py3-none-any.whl`
  and `slate_client-0.2.0.tar.gz`.
- **Go:** `proxy.golang.org` lists `v0.1.0` and `v0.2.0` for
  `github.com/howlerops/slate-orm/clients/go`.
- **Image:** GHCR tags `0.1.0`, `0.2.0` and `latest`. `0.2.0` and `latest`
  share `sha256:ac6ba82a9f…`, and `0.1.0` is unchanged at `sha256:52e2cd8ea1…`.
  The index has amd64, arm64 and two attestation manifests. The image job ran
  6m48s (17:27:01–17:33:49), against 76m36s for v0.1.0.
- **Binaries:** x86_64 24,318,920 bytes and aarch64 22,446,432 bytes on the
  release, which is a prerelease.
- **Installed as a user would:** `npm install @slate-orm/client@0.2.0` in an
  empty directory, then importing both `@slate-orm/client` and
  `@slate-orm/client/edge` (each exports `Client`, and the edge one
  `Client.connectWeb`). `pip install slate-client==0.2.0` in a fresh
  virtualenv imports `slate` and reports version 0.2.0.

**CI caught one thing in this entry that I did not run first.**
`scripts/check_outside_premises.py` refused the token-expiry caveat: it is a
fact about a registry, recorded as deliberate, with no way given to recheck
it. Nothing in the repository can falsify it, so without a recheck nobody
would. The verdict now carries a manual recheck. I had not run
`scripts/check.sh` before that commit, which is exactly what it exists to
catch.

## What this does not do

- **No installed package was run against a head node.** The checks import
  the published code. Making a call is what the client suites do against the
  source tree.
- **The npm token expires.** Its lifetime is whatever the owner chose. When
  it lapses, `PUBLISH_NPM=true` with a dead credential fails the release by
  design rather than skipping, and it needs replacing then.
