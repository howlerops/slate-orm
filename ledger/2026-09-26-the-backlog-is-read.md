# Every open caveat in this repository has now been read against the tree. 283 in one day, on top of the 100 already done: 13 closed, 6 narrowed, the rest confirmed. `--unread 30` answers zero for the first time.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `docs/caveat-status.json`, `scripts/check_site_css.py`, `scripts/test_check_site_css.py`, `clients/python/tests/test_grouped_join.py`
- **Kind:** process, and the three closures that needed code

## What changed

The re-triage pass finished. `scripts/caveats.py --unread 30` lists **zero**;
every `open` and `narrowed` caveat carries a `checked: 2026-09-26` stamp
meaning somebody went to the code and looked.

Three of the last closures needed work rather than a reading:

1. **`check_site_css.py` reads both stylesheets.** `SHEETS` holds
   `site/style.css` and `examples/explorer/web/src/styles.css`, each compared
   against **its own** sources — pooling them would call a demo class used
   because a site page mentions it. 30 classes in the demo's sheet, none dead.
2. **Python orders a chain's groups by its computed value.**
   `test_ordering_a_chains_groups_by_its_computed_value`, the third client for
   the case Go and TypeScript got this morning.
3. **The other mutation dialects are audited.** `go` was the only one with a
   result cache; the evidence is below.

## Why

Because "read the whole list" was the only method that worked, and the only
way to know it was done was to do it.

**The shape of the day.** 383 caveats read. **13 closed, 6 narrowed**, 364
confirmed still true — about **1 in 20 had moved**, against the 1-in-8 prior
from the first unbiased sample of eighteen. The prior was drawn from a random
sample and the day's reading went in date order through a list whose early
entries are the oldest, so the two are not the same population and neither
number is revised. What can be said is that the *first* eighteen were not
representative of the whole.

**Nine of the thirteen closures shared one shape**: two entries written hours
or days apart, one recording a gap and the other filling it, neither citing the
other. `2026-09-15-one-table-twice.md` closed two on its own and mentions
neither. The sharpest was closable the hour it was written — the fix says so in
the guard's own source comment, six days before a tracker existed to hear it.

**Four closures were not stale at all**; they were caveats the repository had
asked for in its own prose and nobody had answered. `.gitignore` said "if this
happens a third time, the answer is a check rather than another comment". A
guard's docstring named a stylesheet it did not read. Reading the list is what
surfaced the requests.

## Alternatives rejected

**Stop at "the rate is one in twelve, so about twenty remain".** The estimate
was available after six batches and would have saved most of the day. It would
also have been wrong in both directions: it predicted ~20 closures against 13,
and it could not have named which — and three of the thirteen were requests
this repository had written down for itself, invisible to any estimate.

**Sample instead of enumerating.** A sample answers "how much of the list is
dead". It cannot answer "is the list trustworthy now", which is the question a
reader of `docs/caveat-status.json` actually has, and it leaves every unread
caveat exactly as suspect as before.

**Batch the stamping into one commit per ten.** What the first ten batches did,
at a cost: each entry adds two to four caveats of its own, so ten batches added
about thirty. The last eight rounds were stamped under one entry instead —
160 caveats read for one entry's worth of new ones. That is the arithmetic
`ledger/2026-09-25-closing-caveats-opens-caveats.md` described, answered by
writing less often rather than by writing less honestly.

## Evidence

**The counts.** `scripts/caveats.py`: **860 caveats — 277 open, 6 narrowed,
193 closed, 324 deliberate, 0 untriaged.** At the start of the day: 822 — 285
open, 180 closed, 308 deliberate. The open count fell by 8 while 13 closed and
6 narrowed, because the entries recording that work added caveats of their own.

`--unread 30`: **0**, from 267 this morning.

**The audit of the other dialects**, which closes a caveat from this morning:

| dialect | result cache? | checked |
|---|---|---|
| `go` | **yes** | `ok <pkg> (cached)` replays old verdicts; fixed this morning |
| `pytest` | no | `cacheprovider`, `LFPlugin` and `NFPlugin` are registered and skip nothing unless `--lf` or `--nf` is passed; `clients/python/pyproject.toml` sets no `addopts` |
| `node` | no | `node --test` has no result cache, and neither `clients/typescript/tsconfig.json` nor the demo's sets `incremental` or `tsBuildInfoFile`, so `tsc` re-emits and no `.tsbuildinfo` exists on disk |
| `rust` | no | `cargo test` rebuilds on a source change and caches no verdicts |
| `python` | no | runs a script directly |

So the hazard was one dialect's, not a general one — which is the opposite of
what the caveat assumed, and worth stating because assuming the general case
was the reason it stayed open.

**Mutations: seven run, seven caught.**

| mutation | caught by |
|---|---|
| the demo's stylesheet dropped from `SHEETS` | 4 cases |
| the sheets share one pooled corpus | `a demo class named only by a site page is still dead` |
| a sheet with no sources is not reported | `a stylesheet with no sources at all is reported, not passed` |
| the demo's `.tsx` sources dropped | 4 cases |
| the two above, re-run after `ruff format` | the same |
| `if !grouping.sort.is_empty()` → `if false`, read by pytest | `test_ordering_a_chains_groups_by_its_computed_value` |

The last is worth reading closely: the pre-existing
`test_ordering_and_limiting_is_over_groups` ran in the same command and did
**not** fail, so the new test is the only thing in that file defending the
group sort.

**The runs themselves**, as `mutate.py` recorded them:

- `ledger/mutations/20260926T011223-scripts-check-site-css-py.json` — the four against the widened guard
- `ledger/mutations/20260926T011719-scripts-check-site-css-py.json` — two of them again after `ruff format` moved the anchors
- `ledger/mutations/20260926T011404-crates-slate-kernel-src-aggregate-rs.json` — the kernel's group sort, read by the Python suite

**Suites.** `clients/python`: `tests/test_grouped_join.py` **14 passed** (13
before). `scripts/test_check_site_css.py`: **12 passed** (9 before).

## What this does not do

**Zero unread is not zero open.** 277 caveats are open and every one has been
looked at today; they are work, not doubt. The number that fell is the doubt.

**A stamp is a claim about attention, not a proof.** `caveats.py`'s own
docstring says so. Several of today's checks were searches that found nothing —
"no occurrence of `ErrorInfo`", "no window on `JoinQuery`" — and a differently
spelled implementation would pass them. I counted those in the two batch
entries that used them and not elsewhere, so the total is unknown.

**The stamps all carry one date.** On 2026-10-26 the whole list falls off
`--unread 30` at once. ~~and the next pass has no ordering to work from~~ —
**wrong, within the hour.** `unread()` iterates `caveats(root)`, which yields
in entry order, so the list comes out oldest-entry-first whether the stamps
tie or not; that is the ordering today's pass worked through and it does not
depend on the dates differing. Spacing the stamps was not possible — they were
read today — and nothing in the tracker staggers a re-read, which is the part
of this that is true.

**Nothing schedules the next pass.** ~~`--unread` will start listing again in a
month and only a person running it will notice.~~ **Answered**, in the weak
form: `scripts/check.sh` now prints the unread count in its closing summary
when it is not zero, which puts it in front of somebody at the moment they are
already about to commit. It is not a failing check, for the reason
`unread()`'s own docstring gives — turning it red would mean stamping the list
to get a green build, and a calendar cannot make CI red on a day nobody
touched the code. What remains is that nobody is *obliged* to act on the
line.

**Three of the day's closures were code, and code has its own caveats.** The
widened CSS guard, the build-output check, the Python test and the two guard
fixes each added their own, and those are stamped as of today only because they
were written today — the weakest kind of stamp there is.
