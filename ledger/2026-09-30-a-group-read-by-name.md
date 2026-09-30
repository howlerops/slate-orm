# A group read by name, a plan measured, and three guards on my own mistakes

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `crates/slate-orm/`, `examples/helpdesk/`, `scripts/check_workspace.py`, `scripts/check_caveat_citations.py`, `scripts/test_read_deliberate.py`, `docs/labelled-verdicts.json`
- **Kind:** feature

## What changed

The five caveats today's three entries left open, closed. Four of them
needed code.

1. **`Records::grouped_records`** and [`Grouped`] — a grouped read whose
   aggregates are looked up by what they compute rather than by where they
   landed. `examples/helpdesk`'s roll-up uses it.
2. **`Helpdesk::analyze` and `Helpdesk::explain_most_urgent`**, and a test
   that measures the plan at three sizes. It falsified the docstring it was
   written to confirm.
3. **`.dockerignore` admitted *whole***: `check_workspace.py` now also
   refuses a tracked file inside a member that a `**/…/` line takes back out.
4. **An absence rule** in `check_caveat_citations.py`: a verdict whose
   reason claims something is missing from the tree must name where somebody
   looked. It fired on eleven existing verdicts, including the one that
   motivated it.
5. **`docs/labelled-verdicts.json` and `scripts/test_read_deliberate.py`** —
   the labelled set the later-entry signal is scored against, and a harness
   that scores it. The number moved while I was writing it.

## Why

`ledger/2026-09-30-the-helpdesk-on-slatedb.md` and
`ledger/2026-09-30-the-caveat-that-came-true-in-an-hour.md` each ended with
open caveats, and open is a verdict, not a plan. These five were small
enough to close in one sitting and two of them turned out to be findings.

**The positional grouped read.** `group_records` returns `Vec<Value>` in
request order — the right shape for the kernel, where a group *is* a row —
and the helpdesk's first roll-up had to map `values[0]` to a count and
`values[1]` to a sum with nothing but proximity holding them together.
`Grouped::get(Aggregate::Sum(…))` takes the aggregate, so a mismatch is
`None` rather than a wrong number.

**The plan nobody had checked.** `most_urgent`'s docstring said the planner
"can serve it without a sort", because `tickets_by_priority` stores exactly
that order. Measuring it found the claim true only above about 12,500 rows,
and found something more interesting on the way: a fresh store has **no
statistics at all** and plans every query against `TableStats::assumed` — no
rows. `analyze_records` and `set_statistics` are both public and documented;
what was missing was anybody saying that an application has to call them.

**The two guards** are on mistakes I made today, four hours apart, and the
second is the general form of the first: a claim about the tree, written
into a reason, that nobody had checked.

## Alternatives rejected

**A generic `group_records_as::<R, K, A>` decoding into a tuple or a
struct,** with the types checked at compile time. A better API and a much
larger one — a trait per arity, or a second derive over a struct whose
fields are aggregates. Rejected because the positional decode does not go
away, it moves into generated code where a reordered list is *still* wrong
and now invisible. `Aggregate` is `Copy + Eq`, so a lookup by value costs a
linear scan of a handful of entries and removes the coupling outright. When
there is a reason to want the tuple, `Grouped` is the layer to build it on.

**Resolving a duplicated aggregate instead of refusing it.** `[Count,
Count]` makes `get(Count)` ambiguous, and first-wins, last-wins and
error-at-lookup all hide the mistake further from where it was made. It is
rejected at the request, which is the only place the caller can see both
entries. `group_records` still accepts it, correctly: the kernel can compute
it, and it is the typed layer that cannot say which one a lookup meant.

**Making `Helpdesk::analyze` happen automatically** — on `open`, or on the
first read. Rejected because re-analysing to plan a query means scanning the
table to decide how to scan the table, and because `&mut self` is the
honest signature: statistics live on the `RecordStore`, and an application
schedules this the way it schedules a migration.

**Tuning the later-entry signal until it scores 2 of 2.** Widening the
printed cut from three, or making the rarity cutoff absolute rather than a
sixth of the ledger, would very likely put both labelled answers in view.
Against a labelled set of **two rows chosen after the fact**, that is
fitting the parameter to the test set and the resulting number would mean
nothing. `SHOWN = 3` and the sixth are unchanged, the score is written down,
and the number to move is the size of `docs/labelled-verdicts.json`.

**Scoring the signal by hit rate.** The obvious measurement, and useless
here: both rows currently miss, so a hit rate is 0 and *stays* 0 through
changes that make the signal much better or much worse. Measured — widening
the cut to ten, and removing the skip that stops an audit entry crowding out
the real one, both leave 0/2 untouched, and a rank moves under both. So the
harness records the closing entry's **rank among candidates**. With two
labelled rows, resolution is the only thing there is.

**Requiring a cited path for every absence claim.** The first draft of the
rule did, and it pushed every honest reason into the exemption roster: "no
file in the tree uses an anchor" is a claim about the tree, not about a
file, and there is nothing to cite. A recorded `git grep` anchors it just as
well, because what both forms share is the thing that matters — the reason
says **where somebody looked**. A rule everyone has to exempt is a
formality.

**A full `.dockerignore` parser,** and a `.dockerignore`-aware context
simulator. Both rejected in
`ledger/2026-09-30-the-caveat-that-came-true-in-an-hour.md` and the
reasoning is unchanged; this adds the one shape that file actually uses,
checked against `git ls-files` rather than the filesystem, because `target/`
is on every working checkout and in no commit.

## Evidence

**23 tests in `examples/helpdesk`**, up from 20 this afternoon and 8 this
morning. The new ones:

- `reordering_the_aggregate_list_does_not_change_what_the_roll_up_reads` —
  the hazard from both sides. Under `group_records`, `values[0]` is
  `I64(1)` with one aggregate order and `I64(275)` with the other; under
  `grouped_records`, `get(Count)` is 1 and `get(Sum(hours_logged))` is 275
  either way. An aggregate nobody asked for is `None`, not a neighbour.
- `an_ambiguous_grouped_request_is_refused_before_it_is_read` — `[Count,
  Count]` and a doubled grouping column both refused, and plain
  `group_records` still serving both.
- `the_priority_index_wins_somewhere_between_12k_and_13k_rows` — below.
- `an_agent_may_not_ask_for_a_plan` — `Action::ALL` is `[Read, Insert,
  Update, Delete]`, four and not five. An agent holding `Action::ALL` on
  tickets still cannot `EXPLAIN`, because a plan is costed against
  whole-table statistics and leaks the shape of rows the row policy hides.
  Deliberate and documented in the kernel; worth a test because "ALL" reads
  as *all*, and the one action it excludes is the one an application reaches
  for when a read is slow. The helpdesk now grants it to `supervisor`.

**The plan, measured at four sizes** (`MemoryStore`, `LIMIT 5`):

| tickets | plan | cost |
|---|---|---|
| 30, before `analyze` | `Sort -> Table Scan`, `rows=0` | 1.00 |
| 30 | `Sort -> Table Scan` | 1.01 |
| 3,000 | `Sort -> Table Scan` | 2.07 |
| 12,000 | `Sort -> Table Scan` | 5.75 |
| 13,000 | `Index Scan using tickets_by_priority`, no sort | 6.00 |

The index plan's cost is flat because a `LIMIT 5` walk stops after five
entries however large the table is; the scan-and-sort grows with it. Below
the crossover the planner sorts and is **right** to. The docstring now
carries this table instead of the claim it replaced. The test asserts 13,000
rather than 12,500 so it is not sitting on the boundary, and takes 1.1 s.

**The absence rule found eleven verdicts**, eight of them unanchored. Each
was anchored with the search I ran before writing it — including one that
turned out to still be true for a reason the caveat did not give: *"the
fingerprint deliberately does not hash the scale"* is right,
`crates/slate-kernel/src/migrate.rs` now stores `scale` in `StoredColumn`
and `fingerprint` still hashes only `type_code`, `is_nullable` and
`is_dropped`.

**The labelled-verdict score moved from 1 of 2 to 0 of 2 in four hours**,
with no change to the signal. This is the measurement worth keeping:

- Row 1's closing entry is still a candidate, ranked **16th of 19**.
  `later_entries` prints `hits[:3]`; a fixed cut against a candidate list
  that grows with the ledger drops the answer as soon as three later entries
  share two rare words, and ties break alphabetically, favouring newer
  filenames.
- Row 2's closing entry is **not a candidate at all**. "Rare" means "in
  fewer than a sixth of all entries", and at 523 entries that is 87. Words
  that were rare in a smaller ledger are ordinary in this one.

The hand measurement in
`ledger/2026-09-30-four-caveats-that-had-a-check-in-them.md` was correct
when taken. It is the kind of number that cannot stay correct without a
harness, which is the caveat's own point made sharper than the caveat made
it.

**Mutations.** Three runs from this entry —
`ledger/mutations/20260930T204642-docs-caveat-status-json.json`,
`ledger/mutations/20260930T204943-scripts-read-deliberate-py.json` and
`ledger/mutations/20260930T204959-scripts-read-deliberate-py.json` —
plus `ledger/mutations/20260930T175903-dockerignore.json` from the entry
before it:

| mutation | outcome |
|---|---|
| an absence claim loses the search that anchors it | caught by `check_caveat_citations` |
| the Dockerfile stops copying a workspace member | caught by `check_workspace` (earlier entry) |
| `ranked()` stops skipping the audit entry that quotes the claim | caught by `test_read_deliberate` |
| the rarity cutoff moves from a sixth to a third | caught by `test_read_deliberate` |
| the printed cut widens from three to ten | **survived, recorded** |

The survivor is the measurement, not a gap: both labelled rows rank at 16
and nowhere, so they are outside a cut of ten exactly as they are outside a
cut of three. A two-row labelled set cannot resolve the cut. The reason is
in the run record via `expect_survivor`, so the case fails if it ever starts
being caught without somebody updating it.

**`sh scripts/check.sh`: 93 passed, all of them** — 92 plus the new
`later-entry-signal`.

## What this does not do

**`Grouped` is a lookup, not a type.** `get` returns `Option<&Value>` and
the caller still matches on the `Value` enum to get an `i64`. The positional
coupling is gone; the type mapping is not, and `read_workload` is still
fifteen lines of `match`. A derive over an aggregate struct would close
that, and is the larger change rejected above.

**Nothing uses `Grouped` outside the helpdesk.** The demo, the three clients
and the wire all still take the positional `Group`. Adding a second grouped
shape to the protocol is a protocol change and this is a Rust-surface fix.

**The crossover is one query on one backend.** 12,000-to-13,000 is
`most_urgent` with `LIMIT 5` over `MemoryStore`, whose scan is a `BTreeMap`
walk. On SlateDB over an object store the constants are entirely different —
that is what `POINT_READ_COST` and `SCAN_ROW_COST` are calibrated for — and
this test says nothing about where the crossover falls there.

**`Helpdesk::analyze` is not scheduled.** It exists and the runner does not
call it, so `examples/over_slatedb.rs` still plans everything against
`assumed`. Adding it there would change what the runner measures, and the
runner measures durability rather than plans.

**The absence rule reads a reason, not a claim.** It asks whether a verdict
says where somebody looked. Whether they looked correctly, or whether the
sentence around the citation is true, is the reading — the same limitation
every citation guard in `scripts/` inherits and states.

**One of two is still not a validation rate.** Nor is zero of two. The
caveat that prompted this is closed because the *measurement* is now
standing and the next false verdict is a row somebody adds; the sample is
unchanged and any rate computed from it would be noise.
