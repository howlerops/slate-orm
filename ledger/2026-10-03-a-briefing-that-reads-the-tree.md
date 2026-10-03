# A handoff that is computed, because every written one has gone stale

- **Date:** 2026-10-03
- **Author:** Claude Code (session: what's next, after the release)
- **Touches:** `scripts/handoff.py`, `scripts/test_handoff.py`,
  `scripts/check.sh`, `.github/workflows/ci.yml`, `CLAUDE.md`
- **Kind:** process

## What changed

`python3 scripts/handoff.py` prints what a new session needs: the git
position, the caveat frame, what is still `narrowed` and what closes it, the
newest ledger entries, and the caveats those entries stopped at. `--json` for
a machine.

There was nothing like it. A session arriving cold read `CLAUDE.md`, guessed,
and asked.

## Why a script and not a document

Because this repository has a measured failure rate for written summaries,
and it is not low. `CLAUDE.md` said *seventeen* CI jobs while there were
twenty-three. `docs/releasing.md` said nothing had been published two weeks
after a release had shipped binaries — and it said that in the paragraph
written to describe publishing. `scripts/check_retired_claims.py` exists
because a correction reached one copy of a claim and not another. A
`HANDOFF.md` would join that list within a day, and unlike the others it
would be read *first*, by somebody with no context to catch it with.

So everything in it is computed at the moment it is asked, and the numbers
come from the modules that already own them — `caveats.report()` for the
frame, `check_live_frame.STILL_NARROWED` for the roster. Importing rather
than re-deriving is the point: two counters of one thing is how `CLAUDE.md`
and `ci.yml` disagreed in the first place.

**The part that would otherwise be a hand-maintained TODO is derived too.**
`## What this does not do` is a mandatory section, every bullet in it carries
a verdict in `docs/caveat-status.json`, and `check_live_frame.py` refuses to
let one sit undecided. So the backlog is already written, in a form a person
is forced to keep true. The briefing reads the recent end of it. No new list.

## The 138 KB first draft

The first version took a **seven-day window**, reasoning that this
repository's output is bursty and a count of entries would mean a different
span each time. Run against the real tree it printed **138 KB**: 197 entries
and 754 caveats, because thirty entries a day for a week is not a window, it
is the whole book.

The reasoning was right about burstiness and wrong about which direction it
cuts. A count is bounded by construction; a span is bounded by how busy
somebody was. `RECENT_ENTRIES = 12`, and the briefing prints the dates it
spans so the reader can see the rate for themselves — which is the
information the window was trying to convey, delivered without the risk.

Claims are cut to their first sentence as well (median 49 characters, longest
509). A caveat's first sentence is the claim and the rest is its argument, so
a sentence-boundary cut keeps the half that tells you whether to open the
entry.

## Alternatives rejected

**A `HANDOFF.md` somebody edits.** Above: this repository's own history is
the argument, three times over.

**Having it call GitHub for CI's conclusion.** The single most useful fact it
cannot derive, and the reason not to is the one `scripts/check.sh` is built
around — a briefing that needs a token and a network is one that fails in
exactly the container that has neither. It prints the `gh api` line to ask
with, which is also what `CLAUDE.md` already tells a reader to run.

**Ranking the live edges, or calling some of them "next".** Tempting and
dishonest: the ordering would be mine, not anybody's. It lists where work
stopped and what was decided about each stop; choosing is a person's job. A
script that implied a priority nobody set would be read as one somebody did.

**Reading the frame by re-parsing `docs/caveat-status.json`.** Four lines and
it would have been a second counter of a thing that already has one, which is
the drift `scripts/test_check_sh.py` exists to catch one level out.
`caveats.report()` is imported instead, so the two cannot disagree.

**A ledger `## What this does not do` section as the only source, without the
verdicts.** The claims alone read as a to-do list, which is precisely what
they are not — most are boundaries somebody argued for. Printing the verdict
beside each is what keeps `deliberate` from looking like `open`.

## Evidence

**Seventeen cases**, `scripts/test_handoff.py`, over a real `git init` with
real commits and a tag, a two-entry ledger and a tracker — not the live tree,
where every case would pass by printing something. Two never-fires cases: an
empty ledger must say so, and must not render as a tidy briefing.

**Five mutations, all caught**
(`ledger/mutations/20261003T170424-scripts-handoff-py.json`):

| mutation | caught by |
|---|---|
| the entry count stops bounding anything | `the count bounds what is shown`, and one more |
| a claim is printed whole, however long | `a claim is cut at its first sentence`, and one more |
| entries come back oldest first | four cases, including the span |
| a caveat loses the verdict recorded against it | `each entry's caveat is found, with its verdict` |
| `rev-list HEAD..tag` instead of `tag..HEAD` | `commits since the tag are counted` |

The last is the one worth having: reversed, it answers `0` on a tree with
unreleased work, which is the wrong answer in the reassuring direction.

**The test file had the defect it was written to catch.** It printed
`total = 16` beside seventeen cases — a count asserted rather than derived,
in a suite whose subject is a briefing that must not assert. Counted now.

**Measured, because the first draft's problem was size:** 138 KB before,
**10 KB** after, on the same tree.

**Not measured.** How long it takes. It reads the ledger directory and one
JSON file and returns immediately; nothing here is near a budget.

## What this does not do

**It cannot tell whether CI is green.** By design, argued above. The practical
consequence is that the one thing most likely to be *actionable* on arrival —
a red run — is the one thing the briefing will not mention.

**It has no idea what anybody intends.** It reports where work stopped, not
what should happen next, and a reader who wants that still has to read
`docs/orm-comparison.md`'s gap table and the entries themselves. That is a
choice rather than a gap, and it means the briefing is weakest exactly when
the previous session left something half-finished without writing it down —
which the ledger discipline is supposed to prevent and does not enforce
mid-change.

**Twelve entries is a guess.** It is bounded, which was the requirement, and
nothing says twelve is the right bound; `--entries` exists because it is not
a number anybody measured.

**It reads the filename's date, not the commit's.** An entry dated for work
done last week sorts by that date, which is right for reading the ledger and
wrong if somebody wants "what changed since I was last here". There is no
"since when" argument, and adding one means reading git rather than the
directory.
