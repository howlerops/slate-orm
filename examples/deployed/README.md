# The whole thing, deployed

Everything else in this repository tests the database in a process or in a
browser tab. This runs it the way a deployment runs it — a gRPC head node, on
SlateDB, on an S3 bucket — loads the workbench's own 100,000 trips through the
socket, and checks the answers against a fold done independently in Python.

```sh
./run.sh                 # start, load, check, tear down
./run.sh --keep          # leave it running and print the address
./run.sh --trips 20000   # a quick pass over a prefix
```

Nothing needs installing. Every port is picked by the kernel, so this runs twice
at once, and it does not need Docker.

## What is actually running

```
  check.py ──gRPC──▶ slate-serverd ──▶ SlateDB ──S3──▶ s3_server ──▶ a temp dir
  (Python SDK)        (the kernel)       (LSM)          (s3s)
```

Four processes' worth of real: a real socket, a real protobuf, a real WAL, real
SSTs, real signed S3 requests. The only thing standing in for production is the
S3 *implementation* — `s3s-fs` is a filesystem pretending to be a bucket, and it
does not have MinIO's or R2's conditional-write semantics, multipart thresholds
or error shapes. CI's `minio` job covers that layer against the real thing; this
covers everything above it.

## Why it exists

The browser binding and the head node run the same kernel by two entirely
different routes. The workbench proves the kernel answers; it proves nothing
about the wire, the storage or the client. A computed column that works in a tab
and not through a socket is a computed column that does not work, and until this
existed there was no single thing that would have said so.

So the check asks the questions the workbench asks — trips per hour of day, the
hour and the average fare across a join, a group key on the right side of that
join, a covering index scan, a timezone offset — and compares them to
`taxi.py`, which decodes the same packed file and folds it with nothing from the
kernel in it.

## What it does not cover

**A common-mode bug in `taxi.py`.** The loader and the fold share that decoder,
so an error in it moves both sides and every differential check still agrees.
Shifting its epoch by an hour was tried: only the absolute assertion — the
sample's quietest hour is 04:00 and its busiest 18:00 — noticed. That check is
the reason there is an absolute one at all.

**The Go and TypeScript clients.** Python only. The three-SDK comparison lives in
`examples/explorer`, over an in-memory head node.

**Replicas, leases and failover.** One writer, no readers, no handover. Those are
`slate-slatedb`'s and `slate-server`'s own suites.

**Anything about how fast it is.** The loader prints a rate because watching
100,000 rows go by in silence is unpleasant, not because the number means
something: it is one machine, one process, a filesystem pretending to be S3.
`slate-headbench` is where measurements live.
