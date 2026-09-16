# A durability check that cried data loss, and a cap nothing tested

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `examples/deployed/check.py`, `crates/slate-server/tests/{batch.rs,common/mod.rs}`, `crates/slate-serverd/src/main.rs`, client batch tests
- **Kind:** fix

## What changed

Three things, all of them tests or the thing a test found:

1. The deployed harness's first check — *"every trip survived the wire, the WAL
   and the bucket"* — now demands `Freshness.latest()`. It did not, and failed
   CI reporting **18,000 against 20,000** rows that had not been lost.
2. `max_batch_operations` has tests: one in the server for the refusal, three in
   the daemon for the config. It had none at all.
3. Five tests across the three clients, from four surviving mutations.

## Why

**The durability check could not fail honestly.** This deployment declares two
read replicas that poll on an interval, and the count was issued with no
freshness demand, so any replica could serve it. A replica one poll behind the
loader's last 2,000-row chunk answers 18,000 — and the check whose name is a
durability claim reports data loss that did not happen.

The decisive evidence is in the same CI run: the count further down, which does
pass `Freshness.latest()`, saw all 20,000 and passed. The rows were there. The
first check was reading a stale snapshot and calling it corruption.

It also passes locally, twice, before and after the change — which is what says
it is a race rather than a defect, and is why it survived until a loaded runner
hit the window. A check that fails only under load, about data loss, is worse
than a flake: it is a false alarm on the one subject where a false alarm costs
the most attention.

**The cap had no test.** `max_batch_operations` shipped in the previous commit
as a `Limits` field, a daemon config key and an `if`. Nothing exercised any of
the three. That is the same shape as Go's `protoc` guard earlier on this
branch — a check that never runs — except this one was born that way.

## Alternatives rejected

**Retry the count until it agrees.** It would make CI green and would keep the
check meaningless: a loop that waits for a replica to catch up tests replica
lag, not durability, and it would hide a real loss by waiting for it.

**Read from the writer explicitly rather than demanding freshness.**
`Freshness.latest()` is the protocol's way of saying "a snapshot at least this
new", and it is satisfiable by a replica that has caught up. Pinning the read
to the writer would also work and would test less: it would stop proving that
the rows are visible through the routing layer at all.

**Leave it and note the flake.** The failure text is `18000 against 20000` under
a heading about surviving the bucket. The next person to see that spends an hour
on SlateDB before noticing the check never asked for a fresh read.

**Set the batch cap test's limit to the real 1,000.** Honest, and slow: a
thousand-and-one-operation request to reach a ceiling pins nothing that two and
three do not, and costs a second of CI on every run.

## Evidence

The deployed harness passes locally, run twice after the change. `cargo fmt
--all` and `clippy --workspace --all-targets` with `-D warnings` clean; 19
`slate-server` test binaries green, `slate-serverd` green.

The cap test was checked by breaking the cap: with the `if` forced false,
`a_batch_over_the_cap_is_refused` fails by name.

**Fourteen mutations across the three clients' batch code, four survivors, all
now killed:**

| mutation | result |
| --- | --- |
| python: the atomicity is always independent | killed |
| python: a failed outcome is reported as a success | killed |
| python: the reason token is dropped | killed |
| python: every operation decodes against the first table | **survived** → killed |
| python: the schema claim is dropped | **survived** → killed |
| python: an unset atomicity is accepted | killed |
| python: a failure always becomes the base error | killed |
| go: an unset atomicity is sent anyway | killed |
| go: the schema claim is dropped | **survived** → killed |
| go: a failed outcome is reported as a success | killed |
| go: the reason token is dropped | killed |
| ts: the atomicity is always independent | killed |
| ts: the schema claim is dropped | **survived** → killed |
| ts: a failed outcome is reported as a success | killed |

**Three of the four survivors were the same defect in three languages: nothing
asserted that a batched write carries its schema claim.** That is the fourth
time this exact class has survived a mutation run on this branch — the server's
predicate writes, Python's predicate writes, and now all three clients' batches.
It is a blind spot rather than an oversight, and each client now has a
`the schema claim rides on a batched write` test. The fourth survivor was
narrower: the Python cross-table test used a `delete` that returns no rows, so
the per-operation table mapping was never exercised; it now deletes with
`returning` from a second table and reads a column that exists only there.

One "survivor" was my own harness rather than the code: the Go mutation run
filtered tests by `-run "TestBatch|..."`, which does not match
`TestTheSchemaClaimRidesOnABatchedWrite`. Widened to `-run Batch`, and the
mutation dies. A mutation harness that does not run the test it needs reports a
survivor that is not one, which is the same failure as a skip reading green.

## What this does not do

The deployed harness's *other* unfreshened reads are untouched. Several later
checks compare against a Python fold without demanding a snapshot, and they are
correct-by-luck in the same way — they pass because the replicas have caught up
by the time they run. Only the one that failed is fixed, because only that one
is a durability claim; the rest are testing query semantics and would be better
served by pinning the whole run to one snapshot, which is a bigger change than
this.

Nothing measures how far behind a replica can get, so "one chunk" is inferred
from 18,000 against 20,000 with a 2,000-row batch rather than observed.

The batch cap is still enforced only server-side, and no client checks a
batch's length before sending — noted in the previous entry and still true.
