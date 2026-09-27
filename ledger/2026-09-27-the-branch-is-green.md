# Every CI job on this branch passes. Run 389 is the first all-green run in the workflow's life, and the three failures it took to get there were each somebody else's decision.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `docs/caveat-status.json`, `ledger/2026-09-27-the-minio-image-moved-a-third-time.md`
- **Kind:** correctness

## What changed

The MinIO entry's "CI is the first run" caveat is answered in place with what
run 389 reported, and its verdict in `docs/caveat-status.json` carries the
answer rather than the open question.

Nothing else. This entry exists because the answer is a measurement and the
repository's rule is that a measurement gets written down.

## Why

`ledger/2026-09-27-the-minio-image-moved-a-third-time.md` changed the image
without being able to start it — no Docker on this container — and recorded
that as its own caveat:

> **CI is the first run.** [...] Whether MinIO comes up, answers
> `/minio/health/live`, and takes the pre-made bucket directory the same way
> is unknown until the job runs.

Leaving that caveat open once the job had run would be the staleness this
repository treats as worse than no documentation: a reader would find an open
question that has an answer.

## Alternatives rejected

**Close the caveat rather than answer it in place.** `closed` means something
in the tree now makes the caveat false, and what makes this one false is a
green job in a run that will scroll out of GitHub's retention. The verdict
reads `moment` — a statement about one run — and the honest treatment is to
record which run and what it said, in the entry, where somebody reading the
image change will find it.

**Say nothing, since the job going green is the expected outcome.** The
expected outcome is exactly what does not get written down and therefore
exactly what nobody can check later. The manifest predicted the entrypoint and
the uid; that the prediction held is the evidence the method was sound, and it
is worth one paragraph.

**Wait for a second green run before claiming it.** A single run is a single
run, and the caveat about `latest` moving under the job stands. But the
question this answers — does this image work at all — is answered by one run,
and holding the answer back to look careful would be its own dishonesty.

## Evidence

**Run 389, `integration against MinIO`, conclusion `success`**, on commit
`4f4941e`, started 2026-09-27T01:16:44Z. Steps, from the job's own record:

| step | conclusion | elapsed |
| --- | --- | --- |
| Start MinIO | success | 5s (01:16:52 → 01:16:57) |
| `cargo test -p slate-slatedb --test s3` | success | 18s |

The five-second `Start MinIO` is the informative number: the previous image
failed that step in about three seconds with `exit code 125` on the pull. Five
seconds is a pull, a container start and a health probe that answered.

**Twenty-one jobs, and `failed_only` returns none.** That is the whole
workflow: formatting, the Rust workspace, three clients, the demo, the hook
suite, the layout guard, the other Python, the quickstarts, the conformance
runner, the browser e2e, MinIO, the deployed stack, and both release targets.

**It took three fixes and each was somebody else's decision.** Worth recording
together, because the shape repeats:

| red job | cause | not caused by |
| --- | --- | --- |
| Go client | `setup-go@v6` sets `GOTOOLCHAIN=local`, and a pinned plugin needs a newer Go | any Go in this repository |
| MinIO | quay.io stopped serving the image anonymously | any line in this repository |
| `check_caveat_citations` | a cited path was a build output, present here and absent in the checkout | the citation, which was correct |

Two of the three arrived through an action or a registry changing under a
workflow that had not been edited. The first was introduced by this session's
own Node 24 bump and found by reading the run rather than by any local check.

## What this does not do

**Green once is not green.** `chainguard/minio:latest` is unpinned and will
move; `actions/*` majors will keep changing their defaults; the registry
question has now been answered three different ways in three months. The
entries for each of those carry their own open caveats and this does not close
any of them.

**Two suites in the workflow are smoke runs, not full ones.** `storage
examples, smoke` and `head-node benchmarks, smoke` pass the `--smoke` flag,
which is the smallest fixture each example takes. A green there means the
examples run and refuse a bad argument; it does not mean the numbers they
print are right, which is what `check_table_provenance.py` and
`check_cost_prose.py` are for and what neither can fully do.

**Nothing here watches the branch.** The next red job is found by somebody
running the two calls in `CLAUDE.md`, exactly as before. Three runs went by
unnoticed the last time, and the mitigation is still a note in a file.
