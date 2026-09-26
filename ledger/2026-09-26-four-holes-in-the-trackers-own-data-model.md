# Four caveats the tracker had written about itself, closed together: a `residual` field, two dates instead of one with two meanings, a reason per exemption, and the name half of a citation

- **Date:** 2026-09-26
- **Author:** Claude, working through the open backlog in themed batches
- **Kind:** feature
- **Touches:** `scripts/{caveats.py,test_caveats.py,check_caveat_citations.py,test_check_caveat_citations.py,check_closed_caveats.py,test_check_closed_caveats.py}`, `docs/caveat-status.json`

## What changed

Four open caveats, all about `docs/caveat-status.json`'s own shape, closed in
one pass because they touch one file and one suite.

**`narrowed` has a `residual` field.** `report` refuses a `narrowed` verdict
without one, `--residual` lists what the eighteen narrowed caveats still owe,
and the nine whose residual was buried in `by` prose have it split out.

**`checked` and `reviewed` are two fields.** `checked` means *read against the
tree* and is what `--unread` lists on; `reviewed` means *the verdict was
re-read* and is what a settled row carries. 455 rows migrated. A `closed`,
`deliberate` or `moment` row wearing `checked` is now a reported problem.

**Every exempt closure has its own reason.** `EXEMPT_BECAUSE` holds one
sentence per caveat — sixteen of them — beside the three kinds `EXEMPT` names,
and `check_closed_caveats.py` refuses an exempt row without one, or a reason
whose exemption has gone.

**`check_caveat_citations.py` follows names as well as paths.** A `by` citing
`` `a_join_groups_by_more_than_one_key` `` is checked against every `fn`,
`def`, `func` and `const` the workspace defines.

## Why

Each was a caveat this tracker wrote about itself, and three of the four were
written in the last twelve hours by the passes that built it:

> **`narrowed` has no `residual` field.** The residual lives in the `by` prose,
> so nothing can count how much work the narrowed caveats represent, and
> nothing stops a `by` that names what closed and forgets what is left.

> **`checked` now means two different things.** … A second field would say it;
> one field with two meanings is what is there.

> **16 closures are exempt and are checked by nothing.** … A closure filed
> under `=read` because writing a witness was hard looks exactly like one filed
> there because no witness exists.

> **Nothing checks a `by` that names a test, a function or a commit.**

They are one batch because they are one file's schema, and because a pass that
fixed them one at a time would migrate `docs/caveat-status.json` four times.

The `checked`/`reviewed` split is the one worth dwelling on. The reverse sweep
stamped `checked` on 316 `deliberate` rows to record that their *verdict* had
been re-read, which is a claim about prose; `--unread` reads `checked` as a
claim about *code*. Nothing was wrong today, because `unread()` filters
`deliberate` out before it looks — but that is a guard holding by accident, and
the first time somebody re-verdicts a row to `open` it would inherit a stamp
that means the wrong thing and drop off a worklist it belongs on.

## Alternatives rejected

**Keep the residual in `by` prose, with a convention.** The convention already
existed — "closed: … Left: …" — and nine of eighteen rows followed it well
enough to split mechanically while the other nine said the same thing nine
different ways. A convention a reader can break silently is what a field is
for; the migration that separated them is the evidence that prose does not
hold a shape.

**One date field with a `kind` beside it.** Fewer keys, and it puts the
distinction one level further from the thing that reads it: `unread()` would
have to know the kind rather than the field name. Two names that say what they
mean cost one migration, once.

**Derive the exemption reasons from the entry each closure cites.** Every one
of the sixteen has an entry that explains the closure, so in principle the
reason is already written. In practice it is written as a whole document, and
the sentence that says *why this leaves nothing in the tree* is not marked in
it. Extracting it is the "read the entry and judge it" problem that
`check_closed_caveats.py`'s docstring already declines.

**Raise `NAMED`'s threshold above five words, or lower it.** Four words matches
`read_text`, `max_groups`, `write_row_with` — ordinary identifiers, and a
citation guard that reports a field name is one people learn to ignore. Six
would miss `a_join_groups_by_more_than_one_key`'s neighbours. Five is what
`check_cited_tests.py` already uses on the same corpus, so the two agree rather
than each having a number.

## Evidence

`python3 scripts/caveats.py`: no problem, no orphan, 904 caveats.
`--residual`: 18, each with one. `--unread 30`: 0.
`check_caveat_citations.py`: 154 paths and every cited name resolves.
`check_closed_caveats.py`: 187 witnessed, 16 exempt each with its own reason.
`sh scripts/check.sh`: **65 passed, all of them.** `ruff` and `ty` clean.

**The migration, as numbers.** 455 rows had `checked` renamed to `reviewed`;
nine `narrowed` rows had a residual split out of `by` mechanically on the
"Left:" convention and nine were written by hand because they said it nine
other ways.

**Suites:** `test_caveats.py` 29 → 35 cases, `test_check_caveat_citations.py`
21 → 30, `test_check_closed_caveats.py` 21 → 25.

**Mutation run**, `ledger/mutations/20260926T173558-scripts-caveats-py.json` —
**six cases, six caught**, after two survivors fixed:

| mutation | the test that failed |
|---|---|
| the `residual` requirement → `if False:` | *narrowed with a `by` and no `residual` is refused* |
| the settled-verdict `checked` rule → `if False:` | *a settled verdict carrying `checked` rather than `reviewed` is refused* |
| that rule applied to **every** verdict | *an open caveat carrying `checked` is accepted* |
| the residual read from `by` instead of its field | *narrowed with a `by` and no `residual` is refused* |
| `--residual` lists nothing | *--residual lists what each narrowed caveat owes* |
| `--residual` lists every caveat | the same test |

The two survivors were both missing tests, and both in the same direction:
nothing asserted the **accepting** side. Nothing said an `open` row *may* carry
`checked` — so a rule reading "no row may carry `checked`" passed every case
and would have broken the only field that drives a worklist. Nothing exercised
`--residual` at all.

**A third defect, found by the suite rather than by mutation.** `name_case`'s
first draft passed a `dict` of file bodies to `tree()`, which takes bare paths
and writes `x` into each — so every name case failed for want of a function
body, and the fixture that was supposed to define `a_test_that_is_really_there`
defined nothing. Its own writer now, with the reason in a docstring.

## What this does not do

**`residual` is prose, like `by`.** It can be counted — eighteen of them, which
is the point — and it cannot be measured. "the other half is still open" and
"three of the four named components" both count as one.

**`reviewed` is not enforced on a settled verdict, only permitted.** A
`deliberate` row with neither date is fine, because most are: the field records
a re-reading that happened, and requiring one would mean stamping 421 rows to
get a green build, which is the rubber-stamp pressure `unread()`'s docstring
already argues against.

**The name check reads five-word identifiers in backticks and nothing else.** A
`by` naming a commit, a struct, a CLI flag or a two-word function is invisible.
Commits in particular were in the caveat this closes and are not covered: a
short hash has no shape a regex can distinguish from a hex number.

**Sixteen reasons are sixteen sentences I wrote about my own closures.** They
are more specific than the three kinds they replace and they are not
independent: the same reader who filed a closure under `=read` wrote the reason
saying why `=read` was right.
