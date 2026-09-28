# Two reads, counted rather than argued

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `crates/slate-orm/tests/read_counts.rs`, `docs/caveat-status.json`
- **Kind:** test

## What changed

`crates/slate-orm/tests/read_counts.rs`: a `Counting` wrapper implementing
`Records` by forwarding all 22 methods, incrementing a counter on the four
read ones, and five tests that count what a relation load actually does.

| loader | reads at 2 parents | at 24 |
| --- | --- | --- |
| `load_related` (has-many) | 1 | 1 |
| `load_through` (many-to-many) | 2 | 2 |
| `load_related_through` (keeping the middle) | — | 2 |
| `load_nested` | 2 | 2 |
| `load_through` with no parents | 0 | — |

Two `open` caveats are narrowed against it, and their residual is now the
honest one: these are reads, not round trips over a network.

## Why

Three entries claim a batched relation load is "two reads rather than N" and
all three say so from the code's shape. The caveat in
`ledger/2026-09-16-many-to-many-is-a-composition-not-a-third-relationship.md`
is precise about why that is not enough:

> the `IN`-deduplication assertion in `parents_sharing_a_tag_share_one_read_of_it`
> counts the values in the filter, which is the request's size and not its
> latency.

It is worse than that sentence says. The assertion calls `related_filter`
*itself* and counts the values in the `Expr::In` it gets back, so it never
observes the loader at all: a `load_related_through` that built exactly that
filter and then read once per parent anyway would pass it unchanged. The test
is about a function the loader also happens to call.

Counting at `Records` is counting the thing the claim is about. And the claim
is about `N`, which is why every case runs at two parent counts: at 24 parents
a per-parent loader reads 24 times and a per-join-row one 48, so the assertion
`reads() == 2` is a statement about the shape of the growth and not a snapshot
that a fixture change could satisfy by accident.

The fixture is built so the numbers cannot coincide. One tag is shared by every
article and each article has its own, so 24 parents produce 48 join rows naming
25 distinct tags — four numbers, none of them 2.

## Alternatives rejected

**Put an `AtomicUsize` in `MemoryStore`.** Every test in the workspace would get
read counting for free and the instrument would live in one place. Rejected
because it counts the wrong layer: one `find_records` over an `IN` of 25 keys
may be one index range or 25 point gets depending on what the planner chooses,
so a `MemoryStore` counter measures the planner's decision and not the loader's.
That decision is a real question with its own tests in `crates/slate-kernel`,
and conflating the two would give a number that moves when the planner is tuned
and reads as a relation-loading regression.

**Measure latency instead of counting.** It is what the word "round trip"
suggests, and it is the wrong instrument here: on a loopback memory store the
difference between one read and 24 is microseconds against microseconds, and
the run-to-run spread would swallow it at the parent counts a unit test can
afford. A count has no spread. The latency question is real and is what
`examples/deployed` is for; it is now the stated residual rather than the
implied claim.

**Assert `>= 1 && <= 2` and call it robust.** Rejected: a range is what you
write when you do not know the answer, and the answer here is exact. An exact
assertion that goes red on a deliberate change is the point — somebody who adds
a third read should have to come here and say why.

**Wait for `examples/deployed` to settle all four unmeasured claims at once.**
It needs MinIO, which this container does not have, so "measured" would have
meant "measured in CI by a job I cannot run". Two of the four are answerable
here today at the layer the claim is actually about, and answering two now
beats deferring four.

## Evidence

- `cargo test -p slate-orm --test read_counts`: 5 passed, 0 failed. The counts
  in the table above are the assertions; each is exact, and each of the four
  N-dependent ones runs at both 2 and 24 parents.
- `python3 scripts/mutate.py`, record
  `ledger/mutations/20260928T214923-crates-slate-orm-src-relation-rs.json`:
  deleting the `parents.is_empty()` early return in `load_related` — whose
  comment argues for it and which nothing tested — is caught by
  `no_parents_is_no_read_at_all`.
- `python3 scripts/mutate.py`, record
  `ledger/mutations/20260928T214944-crates-slate-orm-tests-read-counts-rs.json`:
  removing `self.counted()` from `Counting::find_records` is caught by all four
  counting tests, which is the instrument proving it is wired. The second case
  in that run is an **expected survivor**, recorded with its reason: narrowing
  the `[2, 24]` loop to `[2]` removes a case rather than changing behaviour, so
  nothing can fail, and guarding it would mean a test asserting another test's
  source.
- `cargo clippy -p slate-orm --all-targets`: clean.
- `python3 scripts/caveats.py`: 1567 caveats, 114 open, 63 narrowed, 372
  closed, 885 deliberate, 0 untriaged. Two verdicts moved from `open` to
  `narrowed`; this entry's own three are triaged with it.

## What this does not do

**It does not measure a round trip.** Two reads on a loopback memory store says
nothing about what two reads cost across a network to a head node on object
storage, which is the number a reader sizing a page would want. That is the
residual now recorded against both caveats and it is `examples/deployed`'s to
answer; the harness still does not exercise a relation load.

**It leaves the other two unmeasured claims where they were.** Batching from a
client (`ledger/2026-09-16-batch-in-three-clients-and-a-token-that-only-survives-batched.md`)
and keyset paging on the wire
(`ledger/2026-09-16-keyset-pagination-on-the-wire-and-the-page-that-failed-second.md`)
are both claims about a *client*, so counting inside `slate-orm` cannot reach
them: the instrument would have to sit in the Python, Go and TypeScript
clients, or the server would have to expose a per-connection request count the
test could read. The second is the cheaper design and is not built.

**It counts calls, not work.** A loader that made two `find_records` calls each
scanning the whole table would pass every assertion here. That is the right
division — what a scan costs is the planner's question and has its own tests —
but it means "two reads" is now demonstrated and "two *cheap* reads" is still
not.
