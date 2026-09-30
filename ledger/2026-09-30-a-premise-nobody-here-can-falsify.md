# A caveat whose subject is outside this repository goes stale invisibly, so it now has to say how to re-check it

- **Date:** 2026-09-30
- **Author:** Claude Opus 5
- **Touches:** `scripts/check_outside_premises.py`, its test, `check.sh`, `ci.yml`, `docs/caveat-status.json`, three ledger entries
- **Kind:** process

## What changed

A new guard, `scripts/check_outside_premises.py`. It finds every caveat whose
subject lives outside this repository — a URL, GitHub's own settings, a package
registry, an upstream project, a toolchain that ships on somebody else's
calendar — and requires each one still live to carry a `recheck` in
`docs/caveat-status.json`: a URL and what it should answer, or a sentence
saying what a person must do. `--run`, which CI runs and `check.sh` does not,
fetches the URL recipes and compares.

Ten caveats are in the class. Two are exempted as markers misfiring on this
tree's own directories, with the argument for each. Five are live: three were
checked and hold, and **two more were stale** and are corrected here.

It also withdraws a claim from
`ledger/2026-09-30-thirty-more-deliberate-verdicts-and-two-stale-premises.md`,
written earlier today. See **The correction** below.

## Why

`ledger/2026-09-14-main-and-the-documentation-sweep.md` was committed as
`16a1db6` at **13:49:06** and says Pages "cannot be enabled from here" — tried,
refused with `Resource not accessible by integration`. The Pages workflow's run
on that same commit failed seven seconds later. The next run, **on the same
SHA, with nothing changed in the tree**, succeeded at **14:06:41**. Somebody
with admin had flipped the switch.

The caveat was false seventeen minutes after it was written. It stood for
sixteen days, through five entries that were explicitly auditing caveats,
including one this morning that read its sibling and closed *that* and left
this one — see the correction below. Its neighbour, "`main` is not yet the
default branch", went the same way.

Neither was a bad decision. Both were true when written. Nothing anybody does
in this repository falsifies them, so nobody here is positioned to notice: a
diff review does not touch them, no test covers them, and `git grep` cannot
answer them. That is the whole character of the class, and it is why a rule
about it has to be about *re-checking* rather than about being right.

## Alternatives rejected

**Have the guard fetch everything itself, with no `recheck` field.** Simpler
and wrong twice. It puts the network on the path of a check whose value is
being runnable in a container with none, and it turns a bad afternoon at npm
into a red `scripts` job — which is how a check gets switched off, and this one
is about things nobody looks at. The split costs a field and keeps `check.sh`
free of the internet.

**A tighter marker set, with no exemptions.** `npm` is the whole problem: "an
npm package" and "on first `npm install`" are about directories here, while
"the package is not published anywhere" is about a registry and says `npm` only
in a different clause. A marker precise enough to drop the first two drops the
third, which is a real member. The marker stays loose; the two misses are
argued in `INSIDE_AFTER_ALL`, one line each, and a rot rule reports any that
stops matching.

**Nothing at all, on the grounds that ten caveats is not many.** Ten is few and
the failure rate in the class is **two of the five live ones**, both found in
one afternoon, one of them stale for sixteen days under active audit. Rate
beats count.

## Evidence

`python3 scripts/test_check_outside_premises.py`: 17 passed, 0 failed —
eleven synthetic trees, the three never-fires halves held to firing, and the
real tree last.

`python3 scripts/check_outside_premises.py --run`: 2 recipes fetched, 0
unreachable, 0 wrong.

The class, and what each live member answered:

| caveat | checked how | verdict |
| --- | --- | --- |
| `2026-09-14-ci-that-had-never-run.md`: Pages needs a hand-turn, "a decision for whoever merges" | Pages runs 3 and 4 on `16a1db6`: failed 13:49:13, succeeded 14:06:41 | **closed** |
| `2026-09-14-main-and-the-documentation-sweep.md`: `enablement: true` "does not work here" | the newest run's `configure-pages@v5` step succeeds, the fallback step is skipped | **narrowed** |
| `2026-09-13-typescript-client.md`: the package is not published | `registry.npmjs.org/@slate-orm/client` → 404 | holds |
| `2026-09-16-a-line-break-hid-fourteen-minutes-of-checks.md`: CI's clippy is newer | CI's job log says rustc 1.98.1 (2026-09-01); here, clippy 0.1.94 / rustc 1.94 (2026-03-25) | holds, four minor versions apart |
| `2026-09-29-the-proto-roster-named-one-copy-of-three.md`: nothing checks the vendored `google/rpc` against upstream | fetched both from `googleapis/googleapis@master`; the vendored files are trimmed of comments and options, and `ErrorInfo`'s three fields match exactly | holds as a gap; the copies agree today |

**A test found a real defect in the guard while it was being written.** With
every match exempted, `problems()` reported "no caveat anywhere names a URL" —
true of the set it was looking at and false of the tree. There are three ways
this rule can end up applying to nothing, not two: the markers stop matching,
everything gets closed, or the exemption list swallows the last member. Each
has a different fix, so each gets its own message. The case that caught it was
written to test the exemption, not the halves.

## The correction

`ledger/2026-09-30-thirty-more-deliberate-verdicts-and-two-stale-premises.md`,
written this morning, says of its two false verdicts: *"Their common shape is
narrower and worse — a premise about something outside this tree."* **That is
withdrawn.** Only one of the two is. The other, "`ErrorInfo.metadata` still is
not surfaced", is a claim about three files in `clients/`, falsified by a later
feature adding the capability — a negative existential about this tree, which
is precisely the shape
`ledger/2026-09-29-the-deliberate-sample-carried-to-216.md` had already
proposed and which that entry then reported as unsupported.

The same entry says *"Five of these thirty open with a negative existential and
all five held."* **Also withdrawn**, on both halves. Classifying all thirty
rather than the two that were false gives **twelve** with the shape, by the
criterion "asserts that no instance of X exists" — broader than the earlier
entry's "opens with a negative existential", so the two counts are not
comparable without saying so. And one of the twelve is the false one.

Corrected: across the campaign's seven false claims, **four are negative
existentials and two are premises about the outside world**. Both classes are
real; neither displaces the other. This entry is the second class. The first
still wants the tally the 216-row entry asked for, which is now four of seven
rather than three of four.

The mechanism of the error is worth more than the error. I had two data points,
saw a shape they shared, and wrote it up without classifying the other
twenty-eight — the thing this repository's standards say to do, in an entry
about claims that were asserted rather than checked. Measuring the class, which
took one script, is what caught it two hours later.

## What this does not do

**A premise nobody's wording trips cannot be seen.** `OUTSIDE` is six surface
markers, not an understanding of what a sentence is about. A claim about the
outside world phrased without one of them is invisible here, the same cost
`scripts/check_retired_claims.py` states about its registry, and for the same
reason: the alternative is a classifier for an undecidable question.

**`--run` proves a recipe still answers right, not that the caveat is true.**
`registry.npmjs.org/@slate-orm/client` answering 404 is consistent with the
package being published under a different name. The recipe is a tripwire on the
claim's most falsifiable part, not a proof of it.

**Two of the ten are exempted by hand**, and an exemption is an argument that
can be wrong. The rot rule catches one whose caveat moved; nothing catches one
that was wrong when written.

**Nothing re-checks the `manual` recipes.** Two of the five live members have
one, because there is no URL that answers them — "read a recent CI job log"
cannot be a fetch. They are a note to the next reader, and the next reader has
to exist.

**The two exempted caveats were not themselves re-read.** They were classified
as inside-the-tree and left at whatever verdict they had; being out of this
guard's scope is not the same as being true.
