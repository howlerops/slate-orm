# The MinIO job has been red since quay.io stopped serving the image anonymously. Third registry in three moves, none of them caused by anything here.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** correctness

## What changed

The `integration against MinIO` job pulls `chainguard/minio:latest` instead of
`quay.io/minio/minio:RELEASE.2025-09-07T16-13-09Z`. Nothing else: same
`server /data`, same credentials, same health probe, same suite.

## Why

It was the only red job left, and it had been red since 2026-09-26:

```
Unable to find image 'quay.io/minio/minio:RELEASE.2025-09-07T16-13-09Z' locally
docker: Error response from daemon: unauthorized: access to the requested
resource is not authorized
##[error]Process completed with exit code 125
```

An earlier entry recorded this as "nothing verified to port" and left it,
which was honest and was also the third time the job's image had gone away.
The pattern is worth naming: `bitnami/minio` was withdrawn, Docker Hub's
`minio/minio` never served anonymously, and quay.io — MinIO's own registry,
chosen precisely because it did — stopped. Every move was a registry decision;
none was caused by a line in this repository. A CI job whose dependency is one
vendor's anonymous-access policy is a job that goes red on somebody else's
schedule.

## Alternatives rejected

**Authenticate to quay.io or Docker Hub.** The direct fix and the one I cannot
make: it needs a repository secret, and an agent adding one would be adding a
credential nobody reviewed. If the maintainer wants MinIO's official image
back, that is the route, and it is a one-line change to the `docker run`.

**Swap MinIO for Garage or SeaweedFS** (`dxflrs/garage:v1.0.1` and
`chrislusf/seaweedfs:3.80`, both pullable anonymously and both *pinnable*,
measured below). Rejected because the job is named for what it tests: a real
third-party S3 implementation, and specifically the one this project's README
names first. Trading MinIO for a different vendor to regain pinning changes
what the job asserts in order to make the workflow tidier.

**`adobe/s3mock` or `localstack`.** Both pullable and pinned. Both are
emulators, and the repository already has an in-process one
(`crates/slate-slatedb/tests/common/s3server.rs`) that every other suite uses.
A second emulator in CI would be a job that runs and proves nothing new — the
"a skip is green" shape, with more steps.

**Delete the job.** It has found real defects and it is the only thing here
that talks to an S3 server this project did not write. Deleting a check
because its registry moved is how the branch ended up with a workflow that had
never run.

## Evidence

**Measured 2026-09-27, from this container, by asking each registry
directly** — a manifest GET with an anonymous pull token, which is what
`docker pull` does first:

| image | tag | answer |
| --- | --- | --- |
| `quay.io/minio/minio` | `RELEASE.2025-09-07T16-13-09Z` | 401 |
| `quay.io/minio/minio` | `latest` | 401 |
| `minio/minio` (Docker Hub) | `latest` | 401 |
| `bitnami/minio` | `latest` | 404 |
| `chainguard/minio` | `latest` | **200** |
| `dxflrs/garage` | `v1.0.1` | 200 |
| `chrislusf/seaweedfs` | `3.80` | 200 |
| `adobe/s3mock` | `3.12.0` | 200 |
| `localstack/localstack` | `3.8.1` | 200 |

The `chainguard/minio` 200 is not the proxy being permissive: `bitnami/minio`
answers 404 and three MinIO-official paths answer 401 through the same client,
in the same minute.

**It is MinIO, not a wrapper.** The image config blob for the amd64 manifest:

```
Entrypoint: ['/usr/bin/minio']
Cmd: None
User: 65532
```

So `docker run ... chainguard/minio:latest server /data` is the same command
line the quay image took, and the non-root uid is already accommodated by the
`chmod -R 777 /tmp/minio` that was there for MinIO's own non-root user.
Annotations: built 2026-09-24, source
`https://github.com/chainguard-images/images/tree/main/images/minio`.

**The pin was reversed on purpose and the reasoning is in the workflow.**
Chainguard's free tier keeps one tag, so a digest pin would be
garbage-collected and fail exactly as the Bitnami tag did. The index digest is
recorded in the comment instead:
`sha256:bd014394a80898e68c149f2311fdf8d5a2c2f3bb2c33b9327ae6d02b4b065ae1`.

**`scripts/test_check_sh.py`**: 4 passed, `104 steps, 6 blocks and 2 env vars,
all accounted for` — the step count is unchanged, which is the check that this
edited a command rather than adding or removing one.

## What this does not do

**CI is the first run.** There is no Docker on this container, so the image was
verified by reading its manifest and config over the registry API and not by
starting it. Whether MinIO comes up, answers `/minio/health/live`, and takes
the pre-made bucket directory the same way is unknown until the job runs —
which is the "a check that never fires is a check nobody has debugged" risk
`CLAUDE.md` names, taken knowingly because the alternative is a job that is
certainly red.

**It is unpinned, and `latest` will move under it.** A MinIO release that
breaks something this suite relies on arrives without a commit here. The
digest in the comment is the only way to tell afterwards, and reading it is
manual.

**Nothing watches whether the image is still pullable.** The failure is loud —
exit 125 on the first step — and it took a day to be noticed last time,
because nobody read the run. `CLAUDE.md`'s note about reading a run's
conclusion is the whole mitigation, and it is a note.

**The fourth move is not prevented, only made cheaper.** Three registries in
three months says there will be a fourth. What would actually fix it is
building a MinIO image into this repository's own registry, or mirroring one,
and neither is a line of YAML.
