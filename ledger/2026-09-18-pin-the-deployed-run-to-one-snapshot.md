# Every read in the deployed harness is pinned to the sequence the load reached

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5), working the second six of `docs/orm-comparison.md`
- **Touches:** `examples/deployed/` (`check.py`, `go/check.go`,
  `node/src/check.ts`, `README.md`)
- **Kind:** fix

## What changed

`check.py` reads the sequence the writer reports for its first count and pins
every later read to it with `Freshness.at_least(loaded)`, then writes that
sequence into `expected.json`. The Go and TypeScript arms read it back and fold
it into their session watermark with `Observe` / `observe`, which is what those
two clients have instead of a per-read freshness argument. All three assert the
sequence is greater than zero before using it.

## Why

One read in `check.py` already demanded `Freshness.latest()`, and its comment
says why: on 2026-09-16 an unpinned count reported **18,000 trips against
20,000** in CI — a replica one 2,000-row chunk behind the load, read as data
loss. That read was fixed and the other fourteen were left alone. Every one of
them compares against a fold of the whole file, so any of them could have
reported the same phantom failure. The Go and TypeScript arms were worse: they
have no freshness anywhere, and they run *after* the Python one, so their luck
holds more often and their failure would be rarer and more confusing.

Fixing one read and leaving its neighbours is the shape of bug this repository
keeps finding, so the fix is the whole run rather than the next read.

## Alternatives rejected

**`Freshness.latest()` everywhere**, which is what the one fixed read uses and
the obvious generalisation. It would have quietly gutted this example. `latest`
is the writer and only the writer, so every read in all three arms would go to
the writer — and the three checks below that assert a replica served something,
that the names come back, and that a replica's count matches the writer's would
all still print `ok` while asking a replica nothing. A check that passes by not
performing the test is the failure mode `CLAUDE.md` names as "a skip is green".
`at_least` is satisfied by any view that has polled past the load, which is the
actual requirement.

**A per-read `freshness` argument in the Go and TypeScript clients**, to match
Python. That is the right long-term answer to a real asymmetry — Python has
`Client.query(freshness=…)` on every read and the other two have nothing but
the session watermark — but it is three client APIs, a conformance corpus entry
and a docs pass, which is a different item. `Observe` is documented in both
clients for "carrying a position between sessions, or between processes", which
is exactly this, so the harness uses the mechanism that exists.

**Passing the sequence as a command-line flag** rather than through
`expected.json`. Fewer moving parts, and it would put the pin somewhere other
than where the run's other cross-process facts already live. `expected.json` is
already the channel from the Python fold to the other two arms; a second
channel for one number is a second thing to keep in step.

**Having each arm derive its own pin** with its own `latest` read. It would
work and it would be three pins rather than one, so the three arms could be
reading three different snapshots — which is the opposite of the item.

## Evidence

Three runs, all green: `./run.sh --python --trips 20000 --no-restart`,
`./run.sh --trips 20000 --no-restart` (all three SDKs), and `./run.sh --trips
20000` (with the restart phase). The pin prints the sequence it chose — 12 at
20,000 trips, 52 at 100,000.

**The pin is wired, not decorative.** Mutating it to `at_least(loaded +
1_000_000)` fails every read with `unavailable: replica 'writer' is at sequence
12, behind the required 1000012`. The sequence travels and is enforced.

**A null result, stated plainly: the staleness could not be reproduced.** The
obvious way to provoke it is to make the replicas poll slowly, so
`[[replicas]] poll_interval` was set to `"55s"` and `[routing] catch_up` to
`"60s"` — a poll 27× longer than this example's configured one — and the pin
removed. It passed. Both replicas reported the writer's sequence and the full
count on all eight unpinned reads. Repeated at the full 100,000 trips: passed
again, both replicas at sequence 52.

So this change fixes a fault observed once in CI and not summonable here. That
is worth stating rather than dressing up: the pin is correct and the reasoning
for it is sound, and the claim "these reads were failing" is not one this
session can make.

**And the null result contradicts something already written down.**
`crates/slate-serverd/src/storage.rs` records a measurement where a 10-second
poll made **64 of 64** read-your-writes reads fall through to the writer at
251.87 ms [251.66–252.27] — lag, unambiguously, and the reason `poll_interval`
derives from `catch_up` and a poll at or above it is refused. A 55-second poll
producing no observable lag at all does not fit that. Either the window is far
narrower than the configuration implies, or `manifest_poll_interval` no longer
governs what a `DbReader` sees. Not resolved here; written into
`examples/deployed/README.md` as an open question with the experiment that
would settle it, because it bears on a refusal the daemon currently makes.

`gofmt -l` clean, `tsc --noEmit` clean.

## What this does not do

- **It does not make the harness observe a lagging replica.** The README's
  caveat to that effect stands and now carries the failed reproduction.
- **It does not resolve the poll-interval discrepancy above**, which is a
  question about SlateDB's `DbReader` and needs an experiment against it
  directly, not through this example.
- **It does not give Go or TypeScript a per-read freshness.** The asymmetry is
  recorded, in the example's README and in the comments at both call sites,
  and left for its own item.
- **It does not pin `probe.py` or the restart phase's writes.** Those write
  rather than read, and `check.py` re-derives the pin after the restart because
  a new head node is at a new sequence — so the pin is per-phase, not per-run.
- **It is not a snapshot in the strict sense.** `at_least(n)` is a floor, not
  an exact version: a view further ahead also satisfies it. Nothing writes
  between the load and the checks, so the two coincide here; a harness that
  wrote mid-run would need something the protocol does not offer.
