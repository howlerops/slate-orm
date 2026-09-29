# The day's own thirty-three caveats, read — and "cheaper on the wire" did not survive being counted

- **Date:** 2026-09-29
- **Author:** Claude Code, task K1
- **Touches:** `clients/python/tests/test_round_trips.py`,
  `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** docs

## What changed

The census finished yesterday read the 99 `open` caveats that predated
2026-09-28. It did not read the 33 the day's own entries added, on the
explicit argument that "a caveat written hours ago cannot have been overtaken
by work". Yesterday's own finding contradicts that argument — **all four stale
caveats it found were overtaken by a commit from the same session within eighty
minutes** — so this reads the 33 as well, which completes the population.

**Thirty-one hold. Two had been overtaken**, both by work done later the same
day, both in the round-trip family:

- `2026-09-28-the-invisible-backlog-part-two.md` grouped four claims as "one
  piece of work, not four" and said none was measured. All four are measured:
  a many-to-many's two reads and a nested load's two reads by
  `crates/slate-orm/tests/read_counts.rs`, batching from a client and keyset
  paging by the three clients' round-trip tests.
- `2026-09-28-two-reads-counted-rather-than-argued.md` said the client-side
  claims were left where they were because "an instrument inside `slate-orm`
  cannot reach them", naming a per-connection server counter as the cheaper
  design. The more expensive design — an interceptor in each of the three
  clients — was built forty minutes later.

Closing the first meant checking its fourth claim, and the phrase did not hold.
`clients/python/tests/test_round_trips.py` gains
`test_paging_by_offset_costs_the_same_calls_as_paging_by_cursor`: **four offset
pages cost four `Query` calls, exactly as four keyset pages do.**

Six further caveats ask, in six wordings, for the read this entry finishes, and
close with it.

## Why

Keyset paging being "cheaper on the wire" is written in three entries and was
grouped as measurable. Counting it makes it falsifiable, and it is false as
stated: a page is one request either way, because the client asks for one page
either way. What keyset paging saves is *store* reads — the README's 495
key-value pairs by offset against 5 by cursor, which is a kernel measurement of
what the server does to answer, not of what the caller sends.

The two are easy to conflate and the phrase conflates them. The offset test is
the control that makes the keyset one mean something, in the same way the
one-row-at-a-time write is the control for the batch: without it, "four pages,
four calls" reads as a saving, and it is a saving over nothing.

## Alternatives rejected

**Closing the fourth claim as "measured" without the offset control.** The
keyset test existed and counted four calls for four pages, so the claim could
have been marked measured on it alone. That would have recorded the number
without testing the sentence, and the sentence is what three entries repeat.

**Rewording the three entries that say "cheaper on the wire".** They were
accurate about the kernel measurement they inherited and sloppy about which
wire. A ledger entry is dated and historical; the tracker is what says what is
true now, and the verdict says it. Editing the bullets would also have orphaned
their verdicts, which this repository has done twice by accident already.

**Leaving the 33 unread on yesterday's argument.** The argument is that
same-day caveats are guaranteed passes. Yesterday's own evidence is that
same-day caveats are the *highest*-risk stratum: 2 of 33 here against 4 of 99
there, which is 6.1% against 4.0%. Small counts, overlapping intervals, and the
direction is still the opposite of the argument.

**Extending the random draw to forty, as `a-random-twelve-found-nothing`
proposed.** Overtaken: the population was read entire, so the interval is
4/99 = 4.0% [1.1%, 10.0%] rather than the ~7% a forty-draw would have given.

## Evidence

- The 33 were read against the tree, most by a `git grep` over every place the
  named thing could live: no scale on the wire (the proto says so in a comment
  arguing for it), no response-head fix in `metrics.rs`, both copies of the
  port allocator still in `examples/*/run.sh`, no guard naming `ChannelOptions`
  under `scripts/`, 133 `moment` verdicts still unread by anything.
- **A survivor, and what it exposed.** Mutating
  `offset(page * PAGE_SIZE)` to `offset(0)` — every page asking for the same
  window — **survived** the first version of the offset test, which asserted a
  call count and a row count. Four identical pages satisfy both. The cursor
  test shipped yesterday had the same hole: `after(cursor)` → `after(None)`
  returns twenty rows in four calls. Both now assert the *keys*, in order, and
  both mutations are caught. Recorded in
  `ledger/mutations/20260929T003333-clients-python-tests-test-round-trips-py.json`
  — two cases, one caught and one survived, recorded `outcome: problems` — and
  `ledger/mutations/20260929T003439-clients-python-tests-test-round-trips-py.json`
  — three cases, all caught, after the fix.
- The counts observed, on this container against a debug testserver: 4 `Query`
  calls for 4 keyset pages, 4 `Query` calls for 4 offset pages, 20 keys walked
  in order by each. No spread, because a count has none.
- `clients/python` 339 of 339 pass — the suite, not the file, because the
  fixture is shared and the first version of this file broke five tests in
  `test_streaming.py` by leaving rows behind.
- `python3 scripts/caveats.py`: 118 open, down from 126.
  `scripts/check_closed_caveats.py`: 355 of 386 closures witnessed, 31 exempt.

## What this does not do

**It does not measure what a page costs in bytes or seconds.** The finding is
that offset and keyset cost the same number of *requests*. Whether they cost
the same number of bytes — an offset page's request carries an integer where a
cursor's carries a key — is unmeasured, and on a real network that is the
question. `examples/deployed` is where it would be answered and it needs MinIO.

**It does not check the store-read side of the claim.** The 495-against-5
figure is inherited from the README and was not re-run here. This entry
separates the two halves of a conflated sentence; it re-measures neither.

**The open count went down by eight and will go back up.** Six of the eight
closed were caveats asking for a read, and this entry's own "what this does not
do" adds more. The count cannot reach zero by construction — the pre-commit
hook requires this section and every item in it becomes a caveat — so the
reachable invariant remains 0 untriaged.

**Two of the 33 that hold are about this entry's own subject and stay open.**
Latency and bytes on the wire, and a conformance case comparing the three
clients' counts. Neither is started.
