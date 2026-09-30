# Two conventions became checks: where a read came from, and whether a refusal fires

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `scripts/caveats.py`, `scripts/check_draws.py`, `scripts/test_check_draws.py`, `scripts/test_prebuilt.py`, `ledger/draws/`, `clients/go/slate/`, `clients/typescript/test/`, `clients/python/tests/conftest.py`, `docs/caveat-status.json`
- **Kind:** process

## What changed

Two things written down this morning as conventions, each with a caveat saying
so, now have something that checks them.

**A read against the tree names the draw it came out of.** `caveats.py --draw
<count> <seed>` samples the unchecked frame and writes a record to
`ledger/draws/<date>-<seed>.json` holding the seed, the frame's size and
digest, the indices and the keys. A `deliberate` verdict carrying `checked`
must carry `draw`; `scripts/check_draws.py` re-runs
`random.Random(seed).sample(range(frame), n)` and refuses a record whose
indices do not come back, a stamp naming a draw that did not draw it, and a
draw naming a key that is not a caveat.

**Every harness that takes `SLATE_SERVERD` is driven, not grepped.**
`scripts/test_prebuilt.py`'s roster now carries a drive per row. The three
`examples/*/run.sh` and `clients/python/tests/conftest.py` are executed here,
every run, with a binary timestamped in 2000; each must exit non-zero saying
`was built before`. The Go and TypeScript harnesses need their own toolchains,
so their drive is a named test in that client's own suite —
`clients/go/slate/prebuilt_test.go` and
`clients/typescript/test/prebuilt.test.ts` — and the roster checks the test is
there. `harness_test.go`'s `SLATE_SERVERD` branch was split out as `prebuilt`
so it can be called outside the `sync.Once` that would otherwise poison the
suite's one build, and `harness.ts` exports `binary` and `ROOT`.

## Why

Both caveats named the same shape: a rule that holds because whoever wrote it
meant it.

`ledger/2026-09-30-a-sample-that-pools-with-the-next-one.md` built
`--unchecked` so that successive sampling passes would pool — a row read today
leaves the frame — and recorded that nothing made anyone draw from it: *"the
next pass could sample the whole bucket again and stamp what it read; the
count would move by the same amount while pooling nothing."* Four passes before
today had already done exactly that, not out of carelessness but because the
frame did not exist yet, and the cost was that 246 reads could not be added up.

`ledger/2026-09-30-the-exemptions-i-wrote-without-reading.md` built the roster
after a mutation of `crates/slate-server/src/status.rs` survived the
three-SDK conformance runner against a binary built before it, and recorded
that the roster is a grep: *"it can tell that a file mentions a refusal, not
that the refusal works — a harness that imports `prebuilt` and never calls it
passes."* Three of the six harnesses were in precisely that state that morning:
they read the variable and refused nothing.

## Alternatives rejected

**A `checked` date and an honour system, for the draw.** What was there. It
costs nothing and buys nothing the next pass can rely on, and the failure it
allows is not dishonesty — it is a session that never learns the frame exists
and re-draws blind, which has now happened four times in this repository.

**Storing the frame itself in each draw record** rather than its size and
digest. 1013 keys is about 80 KB per draw, and it would let a later reader
re-derive the sample exactly. Rejected because the indices plus the size
already pin the sample — `random.Random(seed).sample(range(1013), 30)` is one
list — and the digest catches two records disagreeing about what the frame
held. 80 KB per pass, in a directory meant to grow by one file per pass, for a
check the cheap version already makes.

**An exemption list for the rows stamped before `--draw` existed.** Thirty-one
rows read today with `random.seed(329)` over the whole bucket, which cannot be
re-derived because five rows have since been added to it. An `EXEMPT_BECAUSE`
entry would have been the repository's usual idiom and is exactly what
`ledger/2026-09-30-a-premise-nobody-here-can-falsify.md` warns about: a
hand-written line nothing re-checks. Instead the draw is recorded with the keys
it actually read and a `recovered` field naming the entry that wrote the seed
down, and the guard refuses a `recovered` record drawn after 2026-09-30 — so
the hatch is closed by the calendar rather than by anybody remembering.

**Running `go test` and `npm test` from `test_prebuilt.py`** so all six drives
live in one place. `scripts/check.sh` deliberately needs no toolchain, no
built binary and no network, which is what makes it worth running before every
commit; a drive that skipped when `go` was absent would be green, and *a skip
is green* is the failure this repository has already met in the Python
harness. Putting those two drives in their own suites means they run where the
toolchain is — and CI runs both — at the cost that this file can only see the
test exists, which the roster now says in as many words.

**Extracting the Go and TypeScript refusals into a shared implementation**, so
there would be one refusal and three call sites rather than three refusals.
Rejected on the same grounds the original entry gives: both would have to shell
out to `python3` or reimplement the walk, and shelling out makes a Go test
suite depend on a Python interpreter for no gain. Three implementations, each
driven, is worse than one and better than three undriven.

## Evidence

Seven mutations, all caught, all recorded under `ledger/mutations/`:

- `ledger/mutations/20260930T152908-scripts-check-draws-py.json` — the reproduction check never
  fires; a stamp may name a draw that did not draw it; the recovery hatch never
  closes. Caught by three named cases in `test_check_draws.py`.
- `ledger/mutations/20260930T152918-scripts-caveats-py.json` — the frame is the whole bucket
  rather than the unread part; a checked deliberate row need not name its draw;
  a draw wider than the frame is padded. Caught by five named cases.
- `ledger/mutations/20260930T152925-examples-retention-run-sh.json` — **the one that matters.**
  `python3 "$root/scripts/prebuilt.py" >/dev/null || { … exit 1; }` replaced by
  `… || true`: the file still names `scripts/prebuilt.py`, so the roster's grep
  half stays green, and the refusal no longer refuses. Caught by
  `examples/retention/run.sh refuses a stale prebuilt binary when driven`, and
  by nothing else in the report.
- `ledger/mutations/20260930T153044-clients-python-tests-conftest-py.json` — the same shape in
  Python: `_refuse_if_stale(path, …)` replaced by `_ = _refuse_if_stale`, so
  the import and the mention survive and the call does not. Caught by the
  Python drive.
- `ledger/mutations/20260930T153102-clients-go-slate-harness-test-go.json` — the Go harness
  stats the binary and forgets to refuse a stale one. Caught by
  `TestAStalePrebuiltBinaryIsRefused`.
- `ledger/mutations/20260930T153111-clients-typescript-test-harness-ts.json` — the same in
  TypeScript, `refuseIfStale(named)` replaced by `void refuseIfStale`. Caught
  by `a prebuilt binary older than the source is refused`.

The two run-driven refusals were also confirmed by hand before the roster was
changed: a binary timestamped 2000-01-01 makes all three `run.sh` exit 1 with
`was built before crates/slate-server/tests/server.rs was last changed`, in
under a second each, before any port is bound.

`python3 scripts/test_prebuilt.py` reports 25 passed, 0 failed, up from 19.
`python3 scripts/check_draws.py` and `python3 scripts/test_check_draws.py` are
in `scripts/check.sh` and in the `guards` job of `ci.yml`, which
`scripts/test_check_sh.py` holds in both directions.

## What this does not do

**It cannot tell a bad read from a good one.** The draw record proves the rows
came out of the frame; what somebody did with them is still a claim about
attention. `ledger/2026-09-25-the-open-caveats-nobody-re-reads.md` measures
that claim at right about six times in seven, and nothing here moves it.

**A determined hand can still write both halves.** Run `--draw`, throw the
result away, read other rows, stamp them — the keys would not match, so that
much is caught; run `--draw` and read its rows badly is not. The failure this
catches is the accidental one, which is the one that has happened.

**The Go and TypeScript drives are not run by `check.sh`.** They run in
`clients/go` and `clients/typescript`, which CI runs and a local session may
not. A change to `harness.ts` that breaks its refusal passes every check
`check.sh` makes.

**The roster still cannot read what a drive asserts.** A drive that exists,
runs and checks the wrong thing passes, which is the same cost
`scripts/check_retired_claims.py` states about its registry and has the same
answer: nothing classifies a sentence for you.

**`prebuilt.py` still compares one binary against the newest source in two
trees**, unchanged by this. The deployed example starts a head node and a
replica from one verdict, which is right while they come from the same build.
