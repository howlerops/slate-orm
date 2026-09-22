# Yesterday I widened the cost-prose guard to every crate and said so; the file it was widened *for* still held two stale claims, in two forms it cannot see — and fixing that found a ninth, in a file eight rounds of sweeping had never touched.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #279 (F5y)
- **Touches:** `scripts/{check_cost_prose.py,test_check_cost_prose.py}`, `crates/slate-kernel/src/latency.rs`, `crates/slate-slatedb/examples/cost_at_scale.rs`, `ledger/2026-09-21-the-constant-was-right-about-a-build-that-no-longer-exists.md`
- **Kind:** defect, found in my own fix from the previous session

## What changed

`check_cost_prose.py` reads **three** derived figures and one restatement,
over comment *runs* rather than lines. It read one and a half, over lines.

`cost_at_scale.rs` loses its last two hand-written figures. `latency.rs` — a
file nothing in #277 or #278 looked at — says a point read costs one request,
which it has since #269.

The #278 entry gets a correction block. Its mutation was real and its
conclusion about that file was not.

## Why

#278 widened this guard from `crates/slate-kernel` to every crate, because
`slate-slatedb`'s `cost_at_scale` example *printed* `POINT_READ_COST = 3.0` in
its header. I fixed that header, demonstrated the mutation was caught, and
wrote that the example "prints the two constants from `slate_kernel::stats`
rather than restating them beside the table".

Opening the file today, three lines quote the constants. I had fixed one.

```
 12 //! - `SCAN_ROW_COST = 0.000125`, which is the claim that a scan returns about
 14 //! - `POINT_READ_COST = 3.0`, which is the claim that a point read costs about
 15 //!   **three requests**.
619     "\nSCAN_ROW_COST says 8,000 rows per request; POINT_READ_COST says 3 requests
```

The guard passed over all three. Not because of the tree it reads — it reads
that file now — but because **none of the three is in a shape it recognizes as
a claim**:

- line 14/15 wraps: `costs about` ends one line and `**three requests**`
  begins the next. Reading line by line sees a verb with no figure and a
  figure with no verb. This is not a contrived split; it is what a 100-column
  margin produces, and it is the *only* form the prose in this repository
  takes when a sentence is long.
- line 619 says `POINT_READ_COST says 3 requests`, with no "a point read
  costs" anywhere. A phrasing the pattern had never been shown.
- and a bare `POINT_READ_COST = 3.0` — the literal restatement, the exact
  string the widening was aimed at — matches nothing at all. What caught the
  old header was the English clause that happened to sit beside it.

That last one is the finding. **I widened which tree the guard reads and
reported it as having closed the defect, when what let the defect through was
which shapes the guard recognizes.** A guard aimed at the right tree and the
wrong shape reads as thorough, passes, and is not.

## Alternatives rejected

**Fix the two lines and leave the guard.** Twenty minutes, and it is what the
last two sessions did — #269 fixed `stats.rs`, #277 swept eight files by hand,
#278 fixed one line. Three rounds of fixing instances. The instances keep
coming back because nothing mechanically recognizes the form they take.

**Match on the constant's name anywhere near a number.** Catches all three and
much else: `POINT_READ_COST` appears beside genuine measurements ("1,221
requests", "3.05 apiece") that are not claims about the constant's value.
Every one becomes an exemption, and a guard mostly made of exemptions gets
switched off. The four named forms are narrow enough to be read and argued
with.

**Read `docs/` too, now that the patterns are stronger.** Still no.
`correctness.md` narrates these numbers historically at length, and the
paragraph-joining added here makes that *worse*, not better: a joined
paragraph of history is one long chunk full of superseded figures. Recorded
in the guard's docstring; `site/check/docs.py` is that tree's guard.

**Keep the `pub const` skip.** See below — it never did anything, and a
mutation proved it.

## Evidence

The guard read **8** claims yesterday and passes. It reads **22** now and
passes, after three corrections. The gap is what it could not see.

**The ninth stale claim, which fell out for free.** With the wider patterns
pointed at the unchanged tree:

```
crates/slate-kernel/src/latency.rs:40 says a point read costs three;
POINT_READ_COST is 1.
```

`latency.rs` had never been examined — not by #269, not by #277's eight-file
sweep, not by #278. Its comment argued that a point read "costs about three
object-store requests … but they overlap so the wait is closer to one round
trip", a latency-versus-work distinction that is still right and was resting
on the figure #278 established came from a cacheless build. Corrected, with
the old reasoning struck through rather than deleted.

**Nine mutations, nine caught, each by a named test.** Six on the guard —
emphasis-stepping, run-joining, whitespace collapse, and the tolerance on each
of the two new figure kinds — and three on `cost_at_scale.rs` that reinstate
the historical defect verbatim: the literal `POINT_READ_COST = 3.0` bullet,
the wrapped `**three requests**`, and the hand-written `3 requests` summary.
All three are caught by *every claim in the real crates is current*. That is
the test #278's entry cited; it now earns the citation for all three forms
rather than one.

**Two mutations survived the first run, and both were real.**

1. *A comment run is not joined, only its last line read* — survived, because
   `paragraphs()` had **two** flush paths, one in the loop and one after it,
   and every fixture in the file ends on a comment line. Only the trailing
   copy ever ran for a joined run. There is one flush now, closed by a `None`
   sentinel, plus a case whose run is flushed by the code following it.
2. *The declaration in `stats.rs` is treated as a restatement* — survived,
   because the `if "pub const" in chunk: continue` I had written to protect
   `stats.rs` **never excluded anything**. A declaration reads
   `POINT_READ_COST: f64 = 1.0`; the pattern wants `POINT_READ_COST = 1.0`;
   the type annotation already separates them. I wrote a safety check for a
   case that could not occur and would have left it there, looking load-
   bearing, for the next reader to trust. Deleted, with the mechanism that
   actually protects `stats.rs` written down and the test rewritten to cover
   it.

Both are the failure `CLAUDE.md` says mutation testing is for, and neither was
visible in a green suite: 25 tests passed with a duplicated flush path and a
dead conditional in the file.

**A smaller one worth recording.** The first attempt at the wrapped-claim test
failed with `0 claims`, because `**` sits between "costs about" and "three" —
markdown emphasis, in the very bullet the case was modelled on. Had I written
that test loosely enough to pass, the bullet I "fixed" in `cost_at_scale`
would have gone on being unchecked, and the guard would have counted 21 claims
while silently skipping the one this task exists for.

`sh scripts/check.sh`: **42 passed, all of them.**
`python3 scripts/test_check_cost_prose.py`: **26 passed, 0 failed.**
`cargo clippy -p slate-kernel -p slate-slatedb --all-targets`: clean.

## What this does not do

**It does not run the example.** `cost_at_scale` compiles and its format
strings are checked by `rustc`; what it prints at 200,000 rows against a real
S3 server was not observed today. #278's run is the last one.

**A claim split across a Rust string literal's backslash-continuation is still
unseen.** Comment runs are joined; adjacent code lines are not, because
joining arbitrary code invents sentences nobody wrote. Nothing splits a claim
that way today — line 619 did, but its figure sat on one line — so this is a
known hole, not a demonstrated one. Written into `paragraphs()`.

**The `~~` exemption is coarser than the guard now needs.** A chunk with an
unbalanced `~~` is skipped whole. Joining runs makes an unbalanced span rarer,
not impossible, and a skipped chunk is a claim nobody checks.

**It still reads Rust only, and only two constants.** Unchanged from #278, and
now more conspicuous: a figure derived from `POINT_READ_COST` in a Python
harness, a Go comment or a TOML fixture is not seen.

**The general defect from #278 is still open.** No measurement in this
repository records which feature set produced it. Today's work makes the
*constants* harder to misquote; it does nothing about a table of numbers with
no build beside it, which is what made the 3.0 figure a mystery for nine
tasks.
