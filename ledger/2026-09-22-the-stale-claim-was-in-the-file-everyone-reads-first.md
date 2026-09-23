# #283 recorded that the guards read `crates/` and `docs/` and stopped there. The stale claim they could not see was in `README.md`, stated as a live checklist item, one section from the top.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #288 (F6h)
- **Touches:** `README.md`, `scripts/{check_cost_prose.py,check_table_provenance.py,frozen_tables.json}`, both their test files
- **Kind:** a stale claim found, two guards widened, and a fixture-isolation trap sprung four times

## What changed

**The stale claim.** `README.md` said, unstruck, in the open-work list:

> `POINT_READ_COST` is 13–19% low at 200k — not enough to change a plan here

`docs/performance.md` carries that same sentence **struck through** and marked
`<!-- not a cost-model claim -->`, because #278 found what caused it: the
figure was measured on a build with SlateDB's block cache compiled out. The
README copy outlived the correction. #284 had just measured a point read at
1.02 requests against the constant's 1.0 — about 2% high, not 13–19% low.

Both guards now read `README.md`, `clients/*/README.md` and
`examples/*/README.md`.

| | before | after |
|---|---:|---:|
| cost-model claims checked | 24 | **25** |
| measurement tables checked | 71 | **74** |

<!-- not a measurement -->

## Why

This is the gap #283 wrote down and left open, and it was not theoretical for
even one task. README.md is the first page a reader reaches. It held a
withdrawn measurement as current, and two unstamped measurement tables — the
batched-write table and the join-plan table, both with wall times.

## Alternatives rejected

**Add a fifth pattern for "is N% low".** It is the shape that was actually
wrong, so it looks like the thing to catch. It cannot be: "13–19% low" states
an *error*, not a value, and checking it needs the measurement the error is
against — which the guard does not have and would have to hard-code. A
hard-coded measurement in the file whose job is catching restated numbers that
went stale is the defect, not the fix. **So this widening would not have caught
the defect that motivated it.** That one was found by reading, and the entry
says so rather than implying coverage it does not have.

**Widen to `site/`.** Every page there was read, for the four patterns and for
both constants by name. It restates neither; its one `8,000 rows` is a
`RETURNING` ceiling in a roadmap bullet. Widening would have meant an HTML
tag-stripper protecting nothing. What is recorded instead is that it was
checked, in the constant's comment, so the next reader does not re-derive it.

**Default the new parameter to `None`, so a fixture cannot forget.** Tempting
after it bit four times in one task. Rejected, and the reasoning is in both
test files: a fixture that forgets fails *loudly* — 36 cases at once in one
guard, 13 in the other — while a production entry point that forgets would
silently check less, and neither guard's never-fires case would notice, since
`docs/` alone always has tables. A loud failure in fixtures beats a quiet hole
in the real run.

**Leave the roster keyed by filename.** Once two trees are read, a bare
`performance.md` stops being unique and every message saying `docs/<name>`
starts lying. Keyed by path from the root now, which rewrote all 72 entries.

## Evidence

**The claim.** `docs/performance.md:1603` has the sentence inside `~~…~~` with
a marker; `README.md:1301` had it plain, in a `- [ ]` bullet. The rewrite
states the current measurement in a form the guard can check — "a point read
costs 1 request" — so the sentence that replaced a stale claim is itself now
guarded.

**Mutation, `check_cost_prose.py`:** 4 cases, 3 caught, 1 recorded. The
survivor is the `is_file()` filter, and it is *demonstrably* redundant rather
than untested: `check` already catches `OSError`, `IsADirectoryError` is one,
and a root holding a directory named `README.md` yields `seen=0 wrong=[]` with
the filter removed. Run, not argued.

**Mutation, `check_table_provenance.py`:** 5 cases. First round, 3 survived —
all in `freeze` and `shown`, **because `freeze` had no test at all**. It could
not have one: it wrote to the checked-in roster, so calling it from a fixture
meant overwriting the real file. It takes a destination now. Second round, all
caught.

**Four fixture-isolation failures, one task.** Adding a parameter with a live
default to `check` and to `main`, in both guards, put the repository's own
files into fixtures that had written none. Each failed loudly and immediately:
36 of 39 cases, then 13 of 14. The cases named "a tree with no claim reads
none" and "a tree with no measurement fails, not passes" are what say it out
loud, and both were written for this exact failure two tasks ago.

**A hand-kept total, found in passing.** `test_check_table_provenance.py` ended
with `total = len(CASES) + 6`. Three new cases printed their results and the
summary still said 14. That is the #268 defect one file over — a count that
cannot notice a case that stopped running. It counts what ran now: 17.

`sh scripts/check.sh`: **47 passed, all of them.**

## What this does not do

**It does not check the shape that was wrong.** Stated plainly above: no
pattern here can verify a percentage-error claim, and none was added. If the
next stale sentence is phrased as an error rather than a value, this finds it
exactly as well as it found this one — by somebody reading.

**It does not read `site/`**, by decision, on evidence that there is nothing
there today. If a cost claim lands in the HTML, nothing catches it.

**Two of 74 tables were added to the frozen roster, not stamped.** The README
runs behind them are from September and cannot be reconstructed, same as the
other 71. They are frozen, which records that they predate the rule — it does
not make them reproducible.

**`examples/batchbench/README.md` duplicates a `docs/performance.md` table
verbatim**, and the digest-keyed roster covers both with one entry. That is
deliberate and now tested, but it means the roster's `file` names one of two
places a body appears, not all of them.
