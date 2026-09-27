# The verdict sweep ran to the end of the backlog: 64 more caveats were `open` that were not work, `open` falls 244 → 180, and the entry that started the sweep got its own arithmetic wrong

- **Date:** 2026-09-26
- **Author:** Claude, working from the standing instruction to address every caveat
- **Touches:** `docs/caveat-status.json`, `ledger/`
- **Kind:** process

## What changed

`ledger/2026-09-26-a-verdict-is-not-a-reading.md` established that the reading
pass earlier the same day had asked the wrong question. It asked "is this
caveat still true?", got "yes" 364 times out of 383, and left those caveats
`open` — but `open` means *still work, the thing to do*, and a caveat can be
perfectly true and not be work. That entry re-read the first six batches for
**verdict** rather than for truth and moved 37. This is the other seven
batches, which run the sweep to the end of the list: every one of the 869
caveats now carries a verdict reached under the same criterion.

64 moved: **61** `open` → `deliberate`, **2** `open` → `closed`, **1** `open` →
`narrowed`. No code changed and no caveat's text changed. `open` falls from
244 to 180.

It also corrects the entry that started it. That entry says `open` "falls from
279 to 242". 279 and 37 are right; **242 is not — it was 244**, and the two
missing are the two fresh caveats the entry itself contributed to the
`## What this does not do` section it was being counted from.
`2026-09-25-closing-caveats-opens-caveats.md` is the entry that recorded
exactly this, and I did it again in the act of citing the tracker instead of
running it.

## Why

The criterion, unchanged from the entry that set it:

> `deliberate` requires the reasoning to be written down, in the caveat or its
> entry: **the alternative named, and why it was not taken.** Everything else
> stays `open`.

Held strictly, that leaves a lot open. "Not done", "a real thing to want", "the
obvious follow-up", "I did not look", "nobody has done it" are all work, and
none of them moved. What did move divides into three recognisable kinds:

**The alternative is the thing the entry already rejected.** `a-counted-claim-
must-name-its-run.md` cannot tell whether a cited mutation record is the *right*
one — and pairing an entry to its record is the inference problem the entry
rejects three paragraphs above the caveat. Checking that *a* run exists and
that a clean sweep is not claimed over a survivor is what is left once the
inference is refused, and it is what the guard does.

**The alternative needs evidence that does not exist and cannot be made.**
`the-mutation-runner-could-not-read-a-doctest.md` cannot audit mutation runs
from before 2026-09-22 because `ledger/mutations/` begins that day.
`the-cliff-does-not-reproduce…` cannot explain the original five-minute
compaction because the machine cannot be inspected and nothing recorded what
else was running on it. `batch-four-and-the-shape-that-keeps-recurring.md`
cannot give a denominator for "six of six have the same shape", because the
denominator is the set of stale caveats nobody found. Each of the three names
what it would take and why it is unavailable. None is a backlog item; leaving
them `open` says the repository owes work that no amount of work can discharge.

**The reason is in the caveat and reads like an apology.**
`the-check-the-gitignore-asked-for.md` says ignore coverage is checked for
JavaScript packages only — and then says why a Rust crate has no equivalent: a
`Cargo.toml` says nothing about where its output lands, and the workspace's one
root `target/` is already ignored, so there is nothing to compare a crate
against. That is a complete argument wearing the grammar of a gap. The same
shape in `the-stylesheet-had-nothing-dead-in-it.md`: the id check is vacuous on
this tree, is exercised by `scripts/test_check_site_css.py` against written
trees, and is kept so the first id anyone adds is checked.

The two closures are the reading pass catching up with its own worklist.
`the-open-caveats-nobody-re-reads.md` said "270 open caveats are still unread"
and `two-ways-to-find-a-stale-caveat-that-do-not-work.md` said 267; both are
now zero, by `ledger/2026-09-26-the-backlog-is-read.md`.

## Alternatives rejected

**Leave the backlog at 244 and call the reading pass the finish line.** This is
what the previous entry's own caveat argued against, and the count is the whole
reason: a number read as "work outstanding" that is 26% wrong in the direction
of pessimism makes the ledger's caveat discipline look like debt accumulation
rather than what it is. Cost of doing nothing: the next person to ask "what is
left" gets 244 items, reads thirty, finds most are settled decisions, and stops
trusting the list. That is the failure mode the tracker exists to prevent.

**Rewrite the caveats instead of re-verdicting them.** A caveat that is a
settled decision could be reworded to say so. `ledger/README.md` forbids it,
and rightly: an entry states what was believed on its date. It is also
self-defeating here — the key is the bullet's first 60 characters, so rewriting
orphans the verdict and the tracker reports the orphan. The verdict lives
beside the claim for exactly this reason.

**Use `narrowed` far more freely.** Ten of the 64 read as half-answered on a
first pass. Only one got it — `the-clients-could-always-send-a-disjunction.md`,
below — because `narrowed` demands `by` name *both* halves, and naming the
residual honestly takes as long as reading the caveat properly did. Guessing at
a residual to shorten a list produces a `by` that is wrong in a field nothing
checks, which is worse than an `open` that is merely pessimistic. The others
stayed `open` and are recorded as the finer-grained pass the previous entry
already asked for.

**Sweep the 421 `deliberate` verdicts in the other direction in the same
commit.** That is the sweep this pass does not do and the previous entry
already booked as a caveat; a decision that has stopped being defensible looks
exactly like one that has not, so it is reading, not filtering, and it is a
separate piece of work. Folding it in here would have meant one commit where
the honest and the hopeful numbers could not be told apart.

## Evidence

Counts read off `docs/caveat-status.json` at each commit with a script that
diffs verdict-by-verdict, not off a summary line:

| point | open | narrowed | closed | deliberate | moment | total |
|---|---|---|---|---|---|---|
| `3fb5f58^` | 290 | 3 | — | — | — | 860 |
| `3fb5f58` (reading pass) | 279 | 6 | — | — | — | 865 |
| `181a966` | 277 | 8 | — | — | — | 865 |
| `6b0e87f` (batches 1–6) | 244 | 9 | 193 | 360 | 63 | 869 |
| this commit (batches 7–13) | 180 | 10 | 195 | 421 | 63 | 869 |
| …after this entry was written | 184 | 10 | 195 | 421 | 64 | 874 |

Transitions, counted per key across the two commits the previous entry
describes: `181a966` moved 2 (`open` → `narrowed`), `6b0e87f` moved 35 (34 →
`deliberate`, 1 → `narrowed`). **37 total, which is what that entry claims.**
Its 242 is 279 − 37, and subtracting moves from an old total is not a new
total, because the entry recording a pass adds caveats of its own — here three,
of which two are `open`. 279 − 37 + 2 = 244, which is what the tracker reported
and what I did not run.

This commit: 64 transitions, all from `open` — 61 to `deliberate`, 2 to
`closed`, 1 to `narrowed`. Per batch, 7 · 7 · 4 · 12 · 10 · 13 · 11.

Yield fell across the sweep and then rose, which is worth reading: batches 7–9
(the 2026-09-14 to 09-17 entries) gave 7, 7, 4, and the last four batches gave
12, 10, 13, 11. Older entries wrote caveats as gaps; the entries from the last
week write more of them with the reasoning attached, because that is what this
repository's standards have been asking for. The sweep is measuring the ledger
getting better at the thing, not my judgement drifting — but I cannot separate
the two from inside the sweep, and the reverse-direction pass is where that
would show.

`python3 scripts/caveats.py` reports no problem and no orphan: every `closed`,
`narrowed` and `deliberate` verdict names something. `--unread 30` reports 0.

`sh scripts/check.sh`: 59 of 59 pass.

**No mutation run.** Nothing executable changed — the diff is 64 verdict
strings and their `by` prose in a JSON file, plus this entry. The tracker's own
behaviour is covered by `scripts/test_caveats.py` (29 cases) and is untouched
here.

## What this does not do

**The reverse sweep is still not done**, and it is now the largest single thing
the tracker's state rests on: 421 `deliberate` verdicts, of which this pass
wrote 95 and the original triage wrote the rest without the criterion that now
governs them. The original triage's `deliberate` bar was "this reads like a
decision"; this sweep's is "the alternative is named and so is the reason". The
two are not the same bar, and I have not re-read the older ones against the
newer one. That is a strictly larger job than the sweep that just finished.

**Nothing checks a `by`.** The 64 rows added here name entries, guards and
"the caveat itself", and `caveats.py` verifies only that the string is
non-empty. `2026-09-24-the-ledger-records-762-caveats-and-tracked-none-of-them.md`
already carries this as an open caveat; this pass makes it 64 rows more
expensive to be wrong about.

**"The caveat itself" is a `by` that points nowhere a tool can follow.** 38 of
the 64 use it, because the reasoning genuinely is in the bullet being judged.
It is honest and it is unresolvable: a reader must re-read the caveat to check
the verdict, which is the work the verdict was supposed to save. A `by` that
quoted the deciding sentence would be better and would have to be kept true by
hand.

**One caveat was moved to `narrowed` and its residual overlaps another
caveat's.** `the-clients-could-always-send-a-disjunction.md` asked for its
day's other caveats to be re-read; the 14 that were `open` were, and the other
41 were not — 38 carry no `checked` stamp at all. That residual is the same
fact as `a-verdict-is-not-a-reading.md`'s "the reverse direction was not
swept", counted from the other end. Nothing in the tracker notices that two
caveats describe one piece of work, and closing it will need both edited.

**184 caveats are open and every one of them is work.** The sweep removed
doubt about the *classification*, not a single item from the backlog. Of the
184, roughly 40 want a machine that can hold a `--release` build, which this
container cannot; the rest are code, tests and docs somebody has to write.

The sweep left 180 and this entry's own five caveats — four `open`, one
`moment` — make it 184. That is the arithmetic this entry corrects the previous
one for, so it is stated from a run of the tracker made *after* the entry
existed rather than from subtraction. Writing down where a change stops costs
about four caveats an entry, every time, and the backlog will not fall below
that rate.
