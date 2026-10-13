# v0.2.1 shipped all five artefacts on SlateDB 0.17, verified at each registry, and the live explorer runs it too

- **Date:** 2026-10-13
- **Author:** Claude Code (session: "I give you permission to run them")
- **Touches:** `docs/releasing.md`, `README.md`, `docs/agent-handoff.md`,
  `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** docs

## What changed

The owner tagged nothing by hand. They gave explicit permission in this
session, and the tag `v0.2.1` was pushed on `db0d21d`, the green commit
`main` had been fast-forwarded to. `release.yml` succeeded on all nine jobs.
This entry records what it shipped, checked at the registries rather than
read from the run:
- `docs/releasing.md` gains a `v0.2.1` table, and its docker example names
  the current tag.
- The README's release bullet and the handoff's "`0.2.0` is out" line now
  say `0.2.1`.
- The last caveat from the 0.17 upgrade is `closed`, with the release record
  as its witness.

## Why

The owner asked to address both decisions the upgrade entry left open, and
to get everything onto the newer SlateDB. The tag was the step only they could
authorise. Until it shipped, every published artefact embedded a head node
that returned descending scans over compacted data out of order.

A release recorded from its own run summary is the shape `docs/releasing.md`
warns about: the npm job reported success 22 seconds before `npm view`
could see 0.2.0. So every row below was asked of the registry itself.

## Alternatives rejected

- **Record the run's summary table and stop.** It says what each job
  reported, not what a user gets. The image row is the clearest case. A
  successful image job does not show that the binary inside was built on
  0.17, so the binary was copied out of the published image and its strings
  were read.
- **Leave the 0.16 caveat `moment` instead of closing it.** `moment` was
  right while the release was pending. Now that the claim has been answered
  by a verifiable event, `closed` with a witness is the stronger record: a
  later edit that removed the release row would turn the guard red.

## Evidence

- **`release.yml` run 38472852795:** success. Nine jobs, 21:44:37 to
  21:52:54. The image job took 7m57s, against 6m48s for 0.2.0.
- **npm:** `npm view @slate-orm/client@0.2.1` gave version 0.2.1, `latest`
  0.2.1, and the registry document carries `dist.attestations`. Installed
  into an empty directory and imported as ESM: 110 exports. The first
  attempt used `require()` and failed, which was my mistake: the package is
  `"type": "module"` and always has been.
- **PyPI:** `slate-client` 0.2.1 has a `bdist_wheel` and an `sdist`.
  `pip install slate-client==0.2.1` into a fresh venv, then `import slate`,
  gave 104 names in `__all__`.
- **Go:** `proxy.golang.org` lists `v0.2.1` for the module.
- **GHCR:** `0.2.1` and `latest` both resolve to `sha256:1b1137c9…`, and
  `0.2.0` is still `sha256:ac6ba82a…`. The index holds `linux/amd64`,
  `linux/arm64` and two attestation manifests. `docker run … --version`
  prints `slate-serverd 0.2.1`. The binary copied out of the amd64 image
  contains the string `slatedb-0.17.0` 91 times and no other SlateDB
  version.
- **Binaries:** attached to the prerelease. x86_64 is 24,524,272 bytes and
  aarch64 is 22,628,640.
- **The live explorer**, redeployed earlier in this session from the same
  tree (Worker version `b004e97a`, image `…9edd73d2`):
  - Both container instances report application version 3.
  - Node `a` leads and `b` refuses writes with `not-leader`, naming the
    leader.
  - Reads on both are served by `reader-2` (16 rows).
  - A colliding author is refused with `UNIQUE_VIOLATION`, and a fresh one
    lands and is cleaned up (`left: false`).
  - A descending books query returned 9201, 9103, 9102, 9101, 9100, 20, …
    in order.
- **The new witness:** a mutation that rewrote its sentence in
  `docs/releasing.md` was caught by `check_closed_caveats: FAIL every
  witness is still in the tree`.

## What this does not do

**The live explorer's deploy reused the Worker's existing secrets.** The
tokens file from the first deploy is no longer in the scratchpad, so
`deploy.sh` was run step by step without its `wrangler secret put` lines.
The next full `deploy.sh` needs a fresh tokens file.

**No installed package was run against a head node.** That is the same as
for 0.2.0, and for the same reason: the suites in CI run each client against
a real server built from the tree the release came from.

**The live data still has never been shown to be compacted.** The
descending query above came back in order, but on 0.16 it probably would
have too, at this size. The proof that 0.17 fixes the bug is the test in
`crates/slate-slatedb/tests/descending.rs`, not this deployment.
