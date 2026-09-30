# The four entries that were true when they were written

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** docs

## What changed

One caveat closed, on a null result. The question, opened an hour ago in
`ledger/2026-09-28-reading-the-open-caveats-instead-of-grepping-them.md`, was
whether any *other* ledger entry states the withdrawn claim that the schema
fingerprint does not hash a decimal's scale — the claim I published three times
today before finding `crates/slate-server/src/fingerprint.rs` hashing it.

Four do:

| entry | added | line |
| --- | --- | --- |
| `2026-09-18-decimals-and-conditional-updates-in-three-clients.md` | `ce3f2df`, 16:53:33 | 43, 146 |
| `2026-09-18-arithmetic-over-money-and-the-expressions-that-are-refused.md` | `d7e4cf1`, 19:36:19 | 162 |
| `2026-09-18-nineteen-ninety-nine-is-a-decimal-not-a-float.md` | `9dc091c`, 20:14:39 | 131 |
| `2026-09-18-three-clients-and-the-integer-they-would-all-have-reached-for.md` | `71e2c78`, 20:26:32 | 110 |

`35d9182`, "fix: hash a decimal's scale into the schema fingerprint", is
**21:23:04 the same day**. Every one of the four predates it by between an hour
and four and a half. Each was true when it was written, and none is struck.

## Why

The rule this repository runs on is that an entry is dated and append-only: a
statement of what was true in September is a record, and correcting it destroys
the thing it is for. `claim_pages` in `scripts/check_site_claims.py` says so in
as many words, and `check_retired_claims.py` excludes `ledger/` for the same
reason.

That rule decides these four, and the timestamps are what make it apply rather
than an appeal to the rule. The one entry struck this evening —
`2026-09-28-the-limits-page-omitted-the-sharpest-limit.md` — is not covered by
it, because it restated the claim *ten days after* the fix, on a docs page whose
job is to say where the system stops today. A dated record of a thing that was
true is not the same object as a fresh assertion of a thing that is false, and
the difference is a timestamp anyone can check.

Worth saying what this does *not* excuse. The four entries are correct as
records and were still the mechanism of the error: a triage pass read all five
caveats they raised, saw four independent present-tense statements of the same
design rationale, and marked every one open without opening `fingerprint.rs`.
Four agreeing entries are one source, not four. The tracker's answer to that is
the verdict — all five are now `closed` against the fingerprint as witness — and
the verdict is the thing a reader consults, which is why it lives in
`docs/caveat-status.json` and not in the entries.

## Alternatives rejected

**Strike all four anyway, for the reader who arrives at one directly.** It is a
real reader and the argument is not silly. Rejected because the cost is paid by
every future entry: if an entry can be edited when later work contradicts it,
then no entry can be read as evidence of what was believed at the time, and the
ledger stops being a record of reasoning and becomes a wiki that happens to have
dates on it. The mechanism for the direct reader already exists and is the
tracker — `scripts/caveats.py` keys a verdict by entry filename, so the five
caveats these entries raise each carry a `closed` verdict naming the fingerprint.

**Add a forward pointer to each — "answered later the same day by …".** A
smaller edit than a strike and it keeps the text intact. Rejected on the same
principle one step weaker, and on a practical count: the pointer would have to
be added to every entry any later work touches, by hand, with nothing checking
it, which is the `COPIES`-with-one-pair failure this repository has already
written up twice.

**Widen `check_retired_claims.py` to `ledger/`.** It would have found these four
today and it would be wrong tomorrow. The guard's whole design is that a retired
phrase must not stand anywhere a reader takes as current, and `ledger/` is the
one tree where a retired phrase is the point. Its `SUFFIXES` comment says this
already.

## Evidence

- `git log --diff-filter=A --format='%h %ci'` on each of the four files, against
  `git show -s --format='%ci' 35d9182`. The four timestamps are in the table
  above; the fix is 2026-09-18 21:23:04 +0000. The latest entry precedes it by
  56 minutes.
- `git grep -n fingerprint` over the five entries that raised the scale caveats:
  four hits, listed above, and none in
  `2026-09-18-the-three-sdks-compared-on-a-decimal.md`.
- `sh scripts/check.sh`: 71 passed, all of them.
- `python3 scripts/caveats.py`: 1564 caveats, 114 open, 61 narrowed, 372
  closed, 884 deliberate, 0 untriaged.

## What this does not do

**It checks one claim, not the class.** The question asked was whether these
four entries carry *this* withdrawn claim. Whether any other entry states
something a later commit reversed — the same shape, a different subject — is
unexamined, and the count of how often it happens is what would say whether the
forward-pointer idea rejected above is worth its cost. Four instances in one
subject on one day is a sample of one day.

**It leaves the four entries readable as current by anyone who does not check
the tracker.** That is the accepted cost of the append-only rule and is stated
rather than solved. A reader who lands on
`2026-09-18-nineteen-ninety-nine-is-a-decimal-not-a-float.md` from a search
engine sees a true sentence about 2026-09-18 with nothing marking that it stopped
being true at 21:23.
