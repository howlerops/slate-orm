# The whole thing, deployed

Everything else in this repository tests the database in a process or in a
browser tab. This runs it the way a deployment runs it — a gRPC head node, on
SlateDB, on an S3 bucket — loads the workbench's own 100,000 trips through the
socket, and checks the answers against a fold done independently in Python.

```sh
./run.sh                 # start, load, check, restart, check, tear down
./run.sh --keep          # leave it running and print the address
./run.sh --trips 20000   # a quick pass over a prefix
./run.sh --python        # only the Python check, for a quick pass
./run.sh --no-restart    # skip the restart phase
```

Nothing needs installing. Every port is picked by the kernel, so this runs twice
at once, and it does not need Docker.

## What is actually running

```
  check.py    ─┐
  go/check.go ─┼─gRPC─▶ slate-serverd ──▶ SlateDB ──S3──▶ s3_server ──▶ temp dir
  node/check  ─┘        writer + 2 replicas   (LSM)         (s3s)
```

Four processes' worth of real: a real socket, a real protobuf, a real WAL, real
SSTs, real signed S3 requests. The only thing standing in for production is the
S3 *implementation* — `s3s-fs` is a filesystem pretending to be a bucket, and it
does not have MinIO's or R2's conditional-write semantics, multipart thresholds
or error shapes. CI's `minio` job covers that layer against the real thing; this
covers everything above it.

**All three SDKs, over the same socket.** `check.py` folds the sample in Python
and writes what it computed; `go/check.go` and `node/src/check.ts` ask the same
questions through their own client and assert the same answers. The fold stays
in one language on purpose — three decoders of one packed file would be three
places to be wrong, and a common-mode error in them would agree with itself.

**Two read replicas.** Every read goes to the pool, which round-robins over
them and falls back to the writer. The check does eight reads and requires at
least one to come back named `reader-`: a pool that declared two replicas and
used neither would be invisible from the answers alone. A read asking for
`Freshness.latest()` is checked separately, because that one has nowhere to go
but the writer.

**A restart.** The head node is killed with `SIGKILL`, the replacement waits
out the dead node's writer lease — three seconds, set in `head.toml`, and the
run fails loudly if the new node comes up read-only instead — and every check
runs again against SSTs this process did not write. A single acknowledged write
straddles the kill, so what is proved is that it was in the bucket and not in
the dead process's memory.

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

**Failover under load.** The restart here is a kill and a cold start: nothing
is reading while the lease changes hands, and no second node is campaigning.
Overlapping writers across a lease change are `slate-server`'s own suite.

**Which durability setting is which.** The probe narrows the window between an
acknowledgement and the kill as far as a shell can, and it does not separate
`durable` from `visible`: setting `visible` and killing immediately was tried
three times and the row survived every time. The claim is not made — see
`probe.py`. That distinction needs a fault injector, not a stopwatch.

**A replica actually lagging.** The replicas poll fast enough on a local
filesystem bucket that nothing here observes stale data, so the catch-up path
is exercised in the sense that a read asking for a sequence is served, and not
in the sense that it had to wait.

This was pushed on, and the result is a **null result worth recording**. Every
read in all three arms is now pinned to the sequence the load reached, because
until recently only one of them was and the rest were correct-by-luck in the
same way — one unpinned read did report 18,000 trips against 20,000 in CI on
2026-09-16, which is a replica one chunk behind, not data loss. Trying to
reproduce that on demand *failed*: with `[[replicas]] poll_interval = "55s"`
and `[routing] catch_up = "60s"` — a poll 27× longer than the configured one —
both replicas still reported the writer's sequence and the full count on every
one of eight unpinned reads, at 20,000 trips and again at the full 100,000.

So the pin is defence against a failure that has been observed once and cannot
be summoned here. What *is* demonstrated is that it is wired rather than
decorative: pinning to `loaded + 1_000_000` fails every read with
`unavailable: replica 'writer' is at sequence 12, behind the required 1000012`,
so the sequence really does travel and really is enforced.

The discrepancy is the interesting part and is left open: `storage.rs` records
a measurement where a 10-second poll made 64 of 64 read-your-writes reads fall
through to the writer, which is lag, and is why `poll_interval` derives from
`catch_up` and why a poll at or above it is refused. A 55-second poll producing
no observable lag at all does not fit that. Either the window is much narrower
than the configuration suggests, or `manifest_poll_interval` no longer governs
what a `DbReader` sees. Worth an experiment against SlateDB directly; not one
this example can run.

**Anything about how fast it is.** The loader prints a rate because watching
100,000 rows go by in silence is unpleasant, not because the number means
something: it is one machine, one process, a filesystem pretending to be S3.
`slate-headbench` is where measurements live.
