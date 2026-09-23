# #280 made nineteen programs print their build; nothing checked that a table recorded *from* one kept the line, and a recorded table is where a number is actually read.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #282 (F6b)
- **Touches:** `scripts/check_table_provenance.py` (new), `scripts/test_check_table_provenance.py` (new), `scripts/frozen_tables.json` (new), `scripts/check.sh`, `.github/workflows/ci.yml`, `docs/performance.md`
- **Kind:** a guard for the half of #280 that #280 did not cover

## What changed

A guard over `docs/`, shaped as an inverted roster like `check_build_stamp.py`:
find every markdown table, decide which ones record a measurement, and require
each of those to be **accounted for** in one of three ways.

* **Stamped** — a `build:` line between the nearest heading above and the
  table. This is the answer for anything measured from now on.
* **Excused** — `<!-- not a measurement -->` within three lines above, for the
  cases the classifier over-calls.
* **Frozen** — contents match `scripts/frozen_tables.json`, the **70** tables
  that predate the rule and whose runs are gone.

`docs/performance.md` already carried the sentence *"tables on this page that
predate 2026-09-22 have no build line"*. Nothing held it. It does now, and the
sentence says which file does.

## Why

#280's guard covers the programs that **emit** a stamp. A reader does not read
a program; they read a table. The gap was recorded honestly in #280 and again
in #281 — twice written down, never closed — which is its own argument for
closing it.

Two design choices carry the weight:

**Detection reads cell contents and the header, not header keywords.** The
first classifier matched measure-words in the header row and got 36 of 82,
with false positives and negatives in both directions. Matching a number with
a unit in a *cell* got 63 — and missed the two-row shape `| | GETs | rows/GET |`
over `| read nothing | 53 | 3,774 |`, which is the scan-cost calibration, one
of the most load-bearing tables in the repository. Reading both gets 70, and
the 12 it passes over are all genuinely design tables (`| shape | answer |
why |`, `| piece | where |`).

**The classifier leans over-eager on purpose.** A table of line counts trips
it, and that costs one comment. The opposite error is a timing table entering
the docs with no record of the build behind it — the nine-task defect this
whole line of work exists because of. When two errors are not equally bad, the
cheaper one is the one to make.

## Alternatives rejected

**Require every new table to declare itself.** Simplest rule, no classifier,
no false negatives. It taxes every design table in `topology.md` and
`security-review.md` with a provenance annotation that means nothing for them,
and a tax paid on things that do not need it is how an escape hatch becomes
reflexive.

**Key the roster on position, not contents.** Much less friction — a table
stays frozen while its prose moves around it. It also means a legacy table's
numbers can be quietly edited forever with no provenance ever attached.
Content-pinning makes re-measuring the moment the stamp becomes available
again, which is exactly when it is recordable. The cost is real and is stated
under *What this does not do*.

**Hand-enumerate the 70 grandfathered tables.** That is the
list-maintained-by-hand smell #267 was about. `--freeze` generates the roster
from the tree, so the only handwork is reviewing its diff.

**Cover `ledger/` too.** A ledger entry is dated and read as history; a doc is
read as current. Retro-fitting provenance into a historical record would be
falsifying it. `docs/` is the tree where staleness misleads.

**Skip the excuse hatch and tune the classifier until it is exact.** There is
no exact classifier for "is this prose a measurement", and #281 is a fresh
demonstration that tuning a text pattern produces new false positives as fast
as it removes old ones — the lookahead that stopped `1,221` being read as `1`
immediately started reading `~3x` as `3`. An explicit sentence an author
writes beats a regex trying to infer intent.

## Evidence

**Eight mutations, eight caught**, each by a named case: the header signal, the
section scope of a stamp, fence skipping, the excuse's three-line reach, the
digest covering contents, stale-entry reporting, the never-fires half, and —
usefully — the character-range bug, which I made for real while exploring.
Written `[\s:-|]` the class is a *range* from `:` (58) to `|` (124), which
excludes `-` (45), so it matches no separator at all; my first survey reported
**3 tables in a tree that has 82**, and the count was wrong in the direction
that reads as "there is nothing here to check".

Classifier, measured against all 82 tables in `docs/`:

| detection | flagged | what it missed |
|---|---:|---|
| header keywords | 36 | timing tables headed `\| \| before \| after \|` |
| number+unit in a cell | 63 | the two scan-calibration tables, bytes-per-row |
| both | 70 | nothing; the 12 remaining are design tables |

Verified the guard fires on the case it exists for, rather than assuming it:
a timed table with no build line is one problem; adding the `build:` line
above it clears it; moving that line above an intervening `##` heading brings
the problem back.

`python3 scripts/test_check_table_provenance.py`: 14 passed, 0 failed.
`sh scripts/check.sh`: **46 passed, all of them** (44 before this).
`scripts/test_check_sh.py` accounts for both new `ci.yml` steps: 87 steps, all
accounted for. `ruff` and `ty` clean against the CI virtualenv.

## What this does not do

**It does not retro-fit provenance onto the 70 legacy tables.** It cannot. The
runs are gone. What it does is stop the list growing and make each departure
from it visible.

**Nothing currently in `docs/` is stamped.** All 70 measurement tables are
frozen; the only `build:` line on the page is an illustrative one inside a
fence. The first genuinely stamped table arrives with #284's re-measurement.

**Content-pinning will fire on a typo fix.** Correcting a transposed digit in a
legacy table unfreezes it and demands a build line that cannot be produced. The
honest move then is `--freeze` with the correction in the same diff, and that
hatch is exactly as abusable as `--no-verify`. Nothing in the script can tell
the two apart; the roster diff in review is the only thing that can.

**The classifier is a heuristic and will be wrong again.** It is wrong in the
cheap direction by construction, and the escape hatch is one line, but a table
of measurements written with no units anywhere in it would still slip past.

**It reads `docs/` only.** `README.md`, the site under `site/`, and every
crate-level doc comment can still carry an unstamped table.
