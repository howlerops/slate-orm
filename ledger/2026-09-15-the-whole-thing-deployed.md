# The whole stack, running: a head node on SlateDB on an S3 bucket, loaded with the workbench's own data and checked against a fold

- **Date:** 2026-09-15
- **Author:** Claude (Opus 5), with jacob.beck.018@gmail.com
- **Touches:** new `examples/deployed/` (`run.sh`, `head.toml`, `taxi.py`,
  `load.py`, `check.py`, `README.md`), new
  `crates/slate-slatedb/examples/s3_server.rs`, `tests/common/s3server.rs`,
  `.github/workflows/ci.yml`, `CLAUDE.md`
- **Kind:** process, and the first end-to-end exercise of the deployed path

## What changed

A runnable deployment. `examples/deployed/run.sh` starts a real S3 server, a
head node on SlateDB on that bucket, loads the workbench's 100,000 January 2024
taxi trips through gRPC, and checks the answers against a fold done
independently in Python.

```
check.py ──gRPC──▶ slate-serverd ──▶ SlateDB ──S3──▶ s3_server ──▶ a temp dir
```

No Docker, every port picked by the kernel, one command. The S3 server is the
one `slate-slatedb`'s tests have used since the backend was written, lifted out
of `tests/` into an example so a person can run it rather than only a test.

It is in CI as `deployed against object storage`, over a 20,000-trip prefix.

## Why

Everything the last three changes built was exercised in a process or in a
browser tab. `Chain::compute`, `JoinQuery.compute`, the Go and TypeScript
`Scalar` surfaces, the joined-space resolution — each has tests, and every one
of those tests runs the kernel directly, through an in-memory head node, or in
wasm. A computed column that works in a tab and not through a socket is a
computed column that does not work, and there was no single thing that would
have said so.

The suggestion was to "consider a proper deployment not just wasm to actually
test this all proper", and that is what this is. What makes it worth having is
not that it runs — it is that the answers are checked against something that
did not come from the kernel. `taxi.py` decodes the same packed file the browser
reads, from the format's own description, and folds it in plain Python. The
database's answers arrive over gRPC from SSTs in a bucket. A disagreement is a
real defect in a path the workbench never touches.

## Alternatives rejected

**MinIO in Docker.** What CI's `minio` job already does, and unrunnable on a
machine without a daemon — which is this one. `tests/common/s3server.rs` records
the same reasoning for the test suite: "a Docker service container makes the
suite unrunnable on a machine without one". The cost is that `s3s-fs` is a
filesystem pretending to be a bucket and does not have MinIO's conditional-write
semantics, multipart thresholds or error shapes. The `minio` job covers that
layer; this covers everything above it, and the README says so rather than
implying the stack is production-identical.

**Comparing the deployed answers against the wasm kernel.** The obvious oracle,
and a bad one: both run the same `slate-kernel`, so the two would agree
perfectly on a kernel bug. Folding in Python is a second implementation of the
question, which is the only kind of agreement worth having.

**Reusing `crates/slate-wasm/src/taxi.rs` through a binding.** One decoder, no
duplication — and the loader and the checker would then share the kernel's
reading of the file, so a decoder bug would move both sides invisibly. Two
readings of one file is the point. It has a cost, recorded below.

**Putting the checks in the Rust test suite instead.** They would run on every
`cargo test` and be much faster. But the client under test would be
`slate-server`'s own types rather than a published SDK, and the point is partly
that a *client* can ask these questions — the Python SDK's `JoinQuery.compute`
and `join.computed(0)` reach the wire here for the first time outside a unit
test.

**Loading row by row.** 100,000 round trips, and a measurement of gRPC rather
than of the database. Batched at 2,000.

## Evidence

**It runs, and every check passes.** Nine checks over the full 100,000 trips:
the row count survives the wire and the bucket; trips per hour of day match the
Python fold; the sample reads as New York wall clock with its trough at 04:00
and its peak at 18:00; day of week matches Python's own `datetime`; the hour and
the average fare — *both from `trips`* — across a join to `zones`; a group key
on the right side of that join; a covering index scan that touches no row; the
indexed count; and a five-hour offset that rotates the hours and keeps every
trip.

**The load, measured:** 100,000 rows in 10.7–10.9 s across three runs, about
9,100–9,600 rows/s, through gRPC, the WAL, SST flushes and signed S3 requests.
One machine, one process, a filesystem pretending to be S3 — the number is
there so the wait is legible, not as a benchmark. `slate-headbench` is where
measurements live.

**The check is load-bearing, falsified two ways.** Loading only 50,000 of the
100,000 trips fails 7 of the 9 checks, with the two survivors being the ones
about the data's *shape* rather than its contents — which is correct. Shifting
`taxi.py`'s epoch by an hour fails exactly one: the wall-clock assertion.

**That second falsification is the finding.** It shows the differential checks
are blind to a common-mode error: `load.py` and the fold share `taxi.py`, so a
decoder bug moves both sides and every comparison still agrees. The absolute
assertion — trough at 04:00, peak at 18:00 — is the only thing standing between
that and a green run, and it exists because this was tried rather than assumed.

## What this does not do

**One client.** Python only. The Go and TypeScript SDKs have their own suites
against an in-memory head node, and the three-SDK comparison lives in
`examples/explorer`. Neither runs against object storage.

**One writer, no replicas.** No lease handover, no reader pool, no failover.
Those have their own suites in `slate-slatedb` and `slate-server`; this is about
the read and write path being correct through the whole stack, not about the
topology.

**No restart.** The bucket is a temporary directory that goes away with the
process, so nothing here proves durability across a restart —
`slate-slatedb`'s `restart.rs` does, and over the S3 path.

**CI runs a prefix, not the whole sample.** 20,000 trips, because the load is
about 9,000 rows/s through the full path and the differential checks say the
same thing at either size. The wall-clock assertion is skipped there and says
so: it is a claim about the whole of January, and the first 20,000 trips are the
first few days.

**Nothing here is measured as performance.** The rate is printed, not recorded
as a result, and the README says why.
