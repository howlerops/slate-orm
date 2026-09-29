# The census excluded the one population its own conclusion was about, and a third of it was already false

- **Date:** 2026-09-29
- **Author:** Claude Code, task K1
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** docs

## What changed

The thirty-three `open` caveats written on 2026-09-28 — the day's own entries,
which the census excluded by construction — were read against the tree. **Twelve
moved: ten to `closed`, two to `narrowed` with a residual each. Twenty-one
hold**, and each of those twenty-one now carries a `checked` date of today
rather than the triage date it was written with. Two new witnesses and eight
exemption rows go into `scripts/check_closed_caveats.py`. Open falls 126 → 114.

## Why

`ledger/2026-09-28-the-census-finished-and-three-more-were-already-answered.md`
read the ninety-nine open caveats whose entries predate 2026-09-28, found four
false, and drew a conclusion about the cause:

> The tracker's failure mode is not decay. It is that a session writes down
> what it did not do, then does it, and closes the loop in the code and not in
> the record.

Every one of its four stale caveats was answered by a commit made **within
eighty minutes**, by the same session. That conclusion has an immediate
consequence the census did not act on: the population where same-session
overtaking operates hardest is the one the census filtered out. It read
entries from previous days precisely because their same-day window had closed.
The thirty-three caveats written *on* 2026-09-28 sat inside an eighteen-hour
window of a session that kept working, and nobody had re-read them.

They are much worse than the rest, and not by a little:

| population | stale | rate | 95% Clopper-Pearson |
|---|---|---|---|
| entries before 2026-09-28 (the census) | 4 of 99 | 4.0% | [1.1%, 10.0%] |
| entries written on 2026-09-28 (this) | 12 of 33 | **36.4%** | [20.4%, 54.9%] |

The intervals do not overlap; Fisher's exact test, two-sided, gives
p = 9.1 × 10⁻⁶. This is the first quantitative confirmation of the census's
cause, and it is stronger than the census's own evidence for it — four
anecdotes with short gaps, against a ninefold rate difference between the
population inside the window and the population outside it.

Three of the twelve are worth naming, because they are not "a gap got filled":

**The scale hole was grouped as unstarted work eight days after it was fixed.**
`2026-09-28-the-invisible-backlog-is-triaged.md` names "a client's declared
decimal scale that nothing checks against the server's … which renders money a
hundred times wrong in silence" as *the largest* of the 118 it triaged, and
`…-part-three.md` groups it with the response-head latency as "two findings,
named, not fixed". `crates/slate-server/src/fingerprint.rs` has hashed a
decimal's scale since commit `35d9182` on 2026-09-18. Later the same day,
`2026-09-28-the-scale-hole-was-closed-ten-days-ago.md` found that and closed
the five caveats recording it — and did not touch the two entries that had
grouped the same hole as the headline item of a backlog. The correction reached
the caveats it was about and not the summaries that quoted them.

**Four round-trip claims were grouped as one unmeasured piece of work, and all
four were measured within the day.** `…-part-two.md` names batching from a
client, a many-to-many's two reads, a nested load's two reads, and keyset paging
on the wire. `crates/slate-orm/tests/read_counts.rs` measured the two relation
counts at 21:34; the three clients' channel counters measured the other two
between 22:44 and 23:12. The grouping was written at 20:59.

So the pattern is not only that a session overtakes its own caveats. It is that
a session's *summary* entries — a triage, a grouping, a backlog roll-up — go
stale fastest, because they restate claims sourced from elsewhere and the
correction lands on the source.

## Alternatives rejected

**Leaving the same-day caveats to the next scheduled re-read.** This is what
the census implicitly chose, and it is the wrong choice for exactly the reason
the census established: the same-day window is when the answering happens. A
re-read a month from now would find the same twelve, having let them read as
work for a month. The cost of reading them now was two hours; the rate says
they are nine times more likely to be wrong than a caveat picked at random from
the rest of the backlog.

**A guard that flags an open caveat written on the same day as a later commit.**
Considered and not built, and this time the reason is arithmetic rather than a
measurement: on a day like 2026-09-28 the rule flags *every* caveat written
that day, because the session commits continuously. Precision would be 12/33 —
better than the 4/65 the census measured for the citation flag, and still a
flag that raises the whole day. The useful version of it is not a guard at all
but an ordering: read the day's own caveats at the end of the day, which is
what this entry did by hand and what `ledger/README.md` could say. A convention
is not a check and this entry does not pretend otherwise.

**Rewriting the two stale summary entries.** `ledger/README.md` forbids it and
should. The two grouped caveats are recorded `narrowed` with the scale half
named as closed and the residual named as what is left, which is the mechanism
this repository built for exactly this and is why the verdicts live outside the
entries.

**Closing the two grouped caveats outright.** Both name two things and one of
the two is genuinely still open — the response-head latency in one case, the
other 117 of the 118 in the other. Closing either would erase a true residual,
which is the failure `narrowed` was added to prevent.

**Witnessing the six "the rest of the open caveats are unread" closures with a
file.** They are closed by every open verdict now carrying a `checked` date,
which is a state of `docs/caveat-status.json` — the file
`check_closed_caveats.py` already parses. A witness pointing at it would be
circular, which is what the `=stamped` exemption exists to say, and each of the
six carries its own sentence in `EXEMPT_BECAUSE` rather than sharing one.

## Evidence

- **The population is exactly the thirty-three** `open` verdicts whose `entry`
  starts `2026-09-28`, enumerated from `docs/caveat-status.json` before any
  edit. Twelve moved; twenty-one were left open. The arithmetic checks out
  independently: the date-bump pass over `verdict == "open" and
  entry.startswith("2026-09-28")` touched exactly 21 rows.
- **Twelve moved, each against the tree rather than against an entry:**
  - four round-trip claims → `a_many_to_many_is_two_reads_whatever_the_parent_count`
    and `nested_loading_is_two_reads_whatever_the_parent_count` in
    `crates/slate-orm/tests/read_counts.rs`; `UnaryStreamClientInterceptor` in
    `clients/python/tests/test_round_trips.py`;
    `WithChainStreamInterceptor` in `clients/go/slate/related_test.go`;
    `interceptors` in `clients/typescript/test/roundTrip.test.ts` (2 caveats);
  - the scale hole → `scale` in `crates/slate-server/src/fingerprint.rs`, line
    177, *"A decimal's scale, and only a decimal's"* (2 caveats, both
    `narrowed`);
  - the deliberate population → the fifteen-verdict sample in
    `ledger/2026-09-28-the-first-sample-of-the-deliberate-verdicts.md`
    (2 caveats);
  - "the rest of the open caveats are unread" → the two census passes
    (6 caveats).
- **Twenty-one held**, and the five most likely to have been overtaken were
  checked by grep rather than by reading their verdict:
  `examples/batchbench/run.sh` still carries its own copy of the port
  allocator (lines 24–49); no conformance case anywhere under
  `examples/explorer/` mentions a request count; no file under `scripts/`
  mentions `ChannelOptions`, so no guard sees a fourth client transport;
  `crates/slate-serverd/tests/ceilings.rs` tests that a node with
  `max_concurrent_requests = 1` *still serves* and that `0` is refused at
  startup, and neither is the limit binding; nothing in `crates/` provides a
  way to block a handler on command, which is what observing either ceiling
  needs. 148 `moment` verdicts exist and nothing has re-read one.
- **Rates, exact binomial, computed here rather than looked up:** 12/33 =
  36.4% [20.4%, 54.9%]; the census's 4/99 = 4.0% [1.1%, 10.0%]. Fisher's exact
  test on the 2×2 table [[12, 21], [4, 95]], two-sided, p = 9.08 × 10⁻⁶.
- **Mutation, against the real tree**, via `scripts/mutate.py`, record
  `ledger/mutations/20260929T100534-scripts-check-closed-caveats-py.json`.
  Three cases, three caught, each by a *named* check:
  - breaking the `many-to-many-two-reads` needle → `the real roster: every
    witness is still in the tree`;
  - breaking the `py-round-trips` needle → the same check;
  - deleting one of the six `=stamped` `WITNESSED` rows → `the real roster:
    every closed verdict names a witness or an exemption` **and** `the real
    roster: no reason outlives the exemption it explains`, the second because
    the `EXEMPT_BECAUSE` sentence outlives the row it explains.
  Each mutation was a change: the needles become strings that occur nowhere
  (`git grep -F` returns nothing for either), and the deleted row is three
  lines of dictionary entry.
- `python3 scripts/caveats.py`: 1603 caveats, **114 open** (was 126), 71
  narrowed, 388 closed, 897 deliberate, 0 untriaged.
- `python3 scripts/check_closed_caveats.py`: 355 of 388 closed caveats have a
  witness in the tree, the other 33 exempt with a reason each.

## What this does not do

**It leaves the twenty-one that held.** They are true, which means the gaps
they name are still there — two ceilings nobody has seen bind, latency and
bytes measured nowhere, no conformance case comparing the three clients' request
counts, no guard for a fourth client transport, and the whole `deliberate`
population sampled once at fifteen.

**It does not read the same-day caveats of any other day.** 2026-09-28 was
picked because the census had just excluded it, not because it is special. If
the 36% rate is a property of same-day caveats rather than of that one busy
day, then every other day's own entries were read at a time when their session
was still working and the same bias applies — a check nobody has run, on a
population of roughly two hundred entries.

**It does not touch the `narrowed` verdicts.** Twenty-seven of them carry a
`checked` date of 2026-09-26 or 2026-09-27, older than any open verdict, and
a residual goes stale exactly the way a caveat does. The census was of `open`
and so is this.

**The "summary entries go stale fastest" claim rests on two instances.** Both
are in this entry and both are the same subject — the scale hole and the
round-trip grouping, one triage and two roll-ups. That is a hypothesis fitted
to two data points in one day, and the way to test it is to count what fraction
of stale caveats across the whole ledger sit in entries that restate a claim
sourced elsewhere. Nothing here counts that.

**The rate comparison is between two populations that were read by the same
reader on consecutive days.** A second reader finding 36% would be evidence the
first did not manufacture; there is no second reader. What limits the
alternative explanation is that the bias would have to run the *other* way to
explain the result — the census was read first and more slowly, so if care
varies, it favoured finding more staleness there, not less.
