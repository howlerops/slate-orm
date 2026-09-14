# A test harness that handed out ports the kernel was about to reuse

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `examples/explorer/run.sh`
- **Kind:** fix

## What changed

`run.sh` picks its five free ports from **below the kernel's ephemeral range**
rather than by asking the kernel for one.

## Why

The demo job failed in CI with `EADDRINUSE` on port 38721 — a port `run.sh`
had chosen moments earlier and declared free. The same commit passed on the
feature branch, so it read as a flake.

It is not a flake, it is a race with a mechanism. `bind(("127.0.0.1", 0))`
allocates from the ephemeral range, 32768–60999 on this kernel, and 38721 is
inside it. So does every **outgoing connection** every service in the stack
makes. Between `run.sh` closing its probe socket and the node adapter calling
`listen`, the Go adapter dialling the head node can be assigned that exact
number — and the adapter dies on a port that was free when it was handed out.

Ports below the ephemeral range are never assigned automatically, so nothing
can take one from under the service that was given it. Each is still probed
before being offered, because something may already be *listening* there.

## The theory I had first, and withdrew

The initial diagnosis was that the five sequential picks collided with each
other: each probe socket was closed before the next was opened, so the kernel
could hand the same number out twice. That is a real weakness in the old code
and it is **not what happened**.

Measured before claiming it: 400 rounds of five picks, closing each socket
before the next — **0 duplicates**. The same 400 rounds holding all five open —
also 0. The collision theory predicts failures at a rate the measurement does
not show, and the fix it implies would not have helped, because the ephemeral
grab happens after all five are chosen regardless of how they were chosen.

Recorded rather than quietly replaced, because "I fixed the thing I first
thought of" is how a race survives a fix.

## Alternatives rejected

**Retrying on `EADDRINUSE`.** The standard mitigation, and it treats a
harness that hands out unusable numbers as a fact of life. It also makes the
failure intermittent rather than absent, which is the state this started in.

**Passing listening file descriptors into each service.** Closes the window
completely and correctly. It also means every one of four services in three
languages growing a way to accept an inherited socket, which is far more
machinery than a demo runner earns.

**Holding all five sockets open until the services bind.** Impossible: the
services bind the ports themselves, so the harness must release them first.
Holding them until the last one is *chosen* — the first fix attempted — is
free and slightly better hygiene, and is not what was wrong.

**Fixed ports for CI.** Reintroduces the problem the free-port code exists to
solve: a stack already running, or two jobs on one runner, and the failure
looks like the SDKs disagreeing.

## Evidence

The mechanism is confirmed rather than assumed: `/proc/sys/net/ipv4/
ip_local_port_range` reads `32768 60999` on this machine, and the failing port
38721 is inside it. The range is read at runtime rather than hard-coded,
because a container configured with a lower floor would reintroduce the bug
exactly.

`./run.sh --conformance`: 34 cases, the three SDKs agree. `./run.sh --e2e`: 17
passed. Sampled allocations are all below 32768.

## What this does not do

It does not make the window zero. A process deliberately binding a low port
between the probe and the service's `listen` still wins, and nothing here
retries. What it removes is the *automatic* reuse, which is the only mechanism
with evidence behind it.

Nothing tests the allocator itself. There is no case asserting "every port is
below the ephemeral floor", so a future edit could move it back into the range
and only CI would notice, intermittently, which is where this started.

The orphaned `go` and `node` processes the job's cleanup reported are
unexplained. They are from the `--conformance` run that precedes `--e2e` in the
same job, so `run.sh`'s teardown is leaving something behind. That is a second
defect, visible in the same log, and it is not fixed here.
