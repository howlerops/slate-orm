# An adversarial review of #278–#280 found eleven defects, including a guard of mine that read a module global where it was handed a parameter — with a fixture that patched the global too, so the test agreed with the bug.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #281 (F6a)
- **Touches:** `scripts/{check_cost_prose.py,test_check_cost_prose.py,check_build_stamp.py,test_check_build_stamp.py}`, `crates/slate-kernel/src/build.rs`, `crates/slate-slatedb/{build.rs,src/stamp.rs}`, `crates/slate-slatedb/examples/cost_at_scale.rs`, `ledger/2026-09-22-a-number-that-cannot-say-what-built-it.md`
- **Kind:** eleven defects in my own three commits, all fixed

## What changed

Eleven findings, most severe first, each with what it actually broke:

1. **`PER_READ` read a measured total as the constant.** `join.rs` and
   `chain.rs` both say *"four hundred point reads cost 1,221 requests"*, and
   the pattern read that as *a point read costs 1*. It passed **only because
   1,221 begins with a 1** — change the measured total to 2,442 and the guard
   accuses two correct files of saying a point read costs 2. The figure now
   needs a word boundary and no grouping comma after it.
2. **`named()` read a module global where `check()` took a parameter.** Over
   any tree but the real one, every path fell through to its absolute form,
   matched no exemption, and reported every excused program as unstamped. The
   fixture hid it by patching the global *as well as* passing the argument.
3. **Three doc comments cited `scripts/check_measurement_provenance.py`,
   which has never existed** — I renamed it to `check_build_stamp.py` while
   writing it and left the references. One of them also promised a check on
   recorded tables that #280's own entry lists as not done.
4. **The load-bearing names were sliced between two source anchors**, so
   renaming either made that half of the guard read an empty string and pass.
5. **The program sweep missed three layouts cargo builds** — `src/bin/*.rs`,
   `examples/<dir>/main.rs`, `benches/<dir>/main.rs` — which defeats an
   inverted roster, whose whole point is catching a program nobody thought
   about.
6. **The `Cargo.lock` walk ran to `/`**, so a vendored build could stamp an
   unrelated workspace's `slatedb` version as this build's.
7. **A violation inside a joined comment run was reported at the run's first
   line** — `file:1` for a module doc, which reads as a guard that cannot
   locate anything. It reports the range now.
8. **The debug warning said "wall-clock numbers below"** in `slate-serverd`,
   which prints none. That is the `dhat-heap` mistake #280 fixed, committed
   in the same change that fixed it.
9. **`cost_at_scale`'s two header columns were two characters out of line**,
   because the padding stayed literal when the values became interpolated.
10. **`sources()` still said "under the kernel"** two tasks after #278 widened
    it to every crate — in the file whose job is catching sentences that
    outlive a change.
11. **#280's entry said "seventeen programs"; it is nineteen**, by the
    enumeration in its own sentence.

## Why

The three commits had 44 green checks, clean clippy, and 24 caught mutations
behind them, and still carried eleven defects. That is worth stating plainly:
a suite and a mutation run establish that the code does what its tests say,
not that the tests say the right thing. Seven of the eleven are in the guards
themselves — code whose only job is to catch this class.

Finding 2 is the one to keep. It is #212's lesson again, one level up: not a
test that rebuilt the logic, but a **fixture that reproduced the bug**, so the
guard and its test were wrong in the same direction and agreed with each
other.

## Alternatives rejected

**Fix the severe ones and record the rest.** Findings 9, 10 and 11 are
cosmetic in isolation. They are also a misaligned benchmark header, a
docstring describing a tree that changed, and a count that contradicts its own
sentence — the three exact categories the last four tasks were about. Fixing
eight of eleven would have been the same judgement that produced them.

**Take the review's word for it.** Three findings were reproduced first — the
`1,221` misread at a Python prompt, the program count by `grep -c`, and the
missing script by `ls`. The `1,221` one mattered: the review said it "passes
only because 1,221 starts with a 1", which is precisely true and would have
been easy to dismiss as theoretical.

**Report the exact source line for finding 7.** Mapping a match offset back
through joining, `~~`-stripping and whitespace collapse is real work for a
report that is already actionable. A range is honest about the granularity
this has; a precise line would be a fourth thing to keep in step.

**Drop the lockfile walk's bound after it regressed.** The first bound stopped
at the first ancestor without a `Cargo.toml` — and `crates/` has none, so
every build stamped `unknown`. The reflex is to revert. What it needed was the
right bound: a lock only counts beside a manifest, and the climb is capped at
twelve.

## Evidence

**The misread, at a prompt, before and after:**

```
'point reads cost 1,221 requests'      -> ['1']   →  []
'point reads cost ~3x the requests'    -> []      →  []
'a point read costs 1 request'         -> ['1']   →  ['1']
```

The middle line is the second half of the fix and was found by making the
first: a lookahead alone let `~3x` through as a cost of 3, because a **ratio**
is a third kind of sentence. Both guards are needed; the tests now hold all
three shapes apart. Claims read fell from 23 to **19** — four were totals and
ratios that were never claims and had been "checked" by accident.

**Nine mutations, nine caught**, each by a named test: the grouping comma, the
word boundary, the line range, the global-versus-parameter, both new program
layouts, and the moved anchor. The three cases added for finding 1 fail
against the old pattern in both directions.

**A fix of mine broke a feature, and the existing test caught it.**
Finding 6's first bound made every build stamp `slatedb unknown`;
`the_resolved_slatedb_version_is_read_from_the_lock` failed, because it
asserts a resolved `x.y.z` rather than merely "not empty". That is the second
time this week a test written slightly stronger than needed paid for itself.

**Verified by running, not reading**, before any of this: `bucket_layout
--json` still emits valid JSON with its `provenance` block intact (the
`if !json` guard holds); both criterion benches run under `--test` and print
`build: release`, confirming `cargo bench` sets `PROFILE=release`;
`run_examples.sh` passes **8/8** for `slate-slatedb` and **5/5** for
`slate-headbench`; and a live `slate-serverd` still prints `LISTENING
127.0.0.1:7451` as the first line of stdout, with the stamp on stderr — the
contract three client harnesses parse.

`sh scripts/check.sh`: **44 passed, all of them.**
`python3 scripts/test_check_cost_prose.py`: 30 passed, 0 failed.
`python3 scripts/test_check_build_stamp.py`: 16 passed, 0 failed.
`cargo clippy -p slate-kernel -p slate-slatedb --all-targets`: clean.

## What this does not do

**It does not re-examine the other two commits' reasoning, only their code.**
The review read the diff. Whether #278's *conclusion* about the block cache is
right rests on the measurement in that entry, which nothing here re-ran.

**Finding 7 gives a range, not a line.** A violation in an eighty-line module
doc is now reported as `1-80`, which is better than `1` and worse than the
line.

**The lockfile bound is twelve levels, chosen and not measured.** Four would
do for this checkout. Nothing tests the vendored case the bound exists for,
because nothing here builds vendored.

**`clients/python/testserver` is still outside the swept tree**, as noted in
#280 and unchanged: it is a binary that measures nothing today, and the guard
walks `crates/` only.

**Nothing checks that a recorded table kept its build line.** Finding 3
removed a doc comment that claimed otherwise. The gap is now described
accurately in two places instead of denied in one.
