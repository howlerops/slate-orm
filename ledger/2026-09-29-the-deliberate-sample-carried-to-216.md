# The `deliberate` sample carried from 168 to 216, and both new false claims say "Nothing"

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** docs

## What changed

Forty-eight more `deliberate` verdicts read against the tree, carrying the same
seeded sample from 168 to **216 of 1008**. Two were false and are re-recorded as
`closed`, each with a witness:

- `2026-09-20-the-accessor-three-adapters-now-call.md` :: *"Nothing outside the
  demo calls it."* → **`closed`**, witness `retired-outside-the-demo`.
  `examples/retention/seed.py`'s `undo()` reads `row.retired` from the generated
  accessor, and calls `row.restored()` beside it.
- `2026-09-18-the-python-that-runs-ci-had-no-checker.md` :: *"Nothing here is
  `*run*` by the new job."* → **`closed`**, witness `scripts-job-runs-things`.
  The `scripts` job now runs about sixty executable steps beside `ty check` and
  `ruff check .`.

The bucket goes 1010 → 1008 and `closed` 461 → 463. No `open` row moved.

## Why

The task is the user's, in their words: the `deliberate` bucket *"outnumber the
open ones seven to one … reading them is the only method left that works, and it
hasn't been done."* Two cheap mechanical routes into it were measured and
rejected earlier today. This is the expensive route, continued.

It also closes a hole this session made. The previous entry stopped at 168 and
the note-to-self for resuming said to start at **192** — so verdicts 169–192
would have been skipped in silence, and the denominator would have said 216 for
a sample that read 192. Both blocks were read here. The lesson is small and
exact: a resume offset is a number that has to be derived from what was written
up, not remembered, and `2026-09-29-the-deliberate-sample-carried-to-168.md` is
the only thing that knows where the last batch ended.

## Alternatives rejected

**Record the two as `open` rather than `closed`.** That is what the first pass
did with its false verdict, and it was right there: the thing the caveat named
was still missing. Here it is present — a call site and sixty CI steps — so the
gap is closed and a witness can watch it. Filing a closed gap as `open` puts a
non-task on a list whose whole value is that everything on it is a task.

**Pick the witness needle by function name** (`def undo`, or the job's `name:`).
Rejected for both: a renamed `undo` that still calls the accessor keeps the
closure true, and the job's name is prose. The needles are the *call* and the
*step*, which are the facts the closures rest on.

**Chase the "Nothing…" enrichment now rather than record it.** Three of the four
false claims found in 216 reads open with a negative existential. That is a
lead, and acting on it would mean re-drawing the sample stratified by claim
shape — which throws away the unbiased rate the last three entries were built
to produce, on four events. Recorded as a hypothesis with its own test below.

**Re-read the 168 already done, now that a pattern is visible.** They were read
against the same tree by the same method; re-reading would mostly re-confirm,
and the reads that would change are exactly the ones a *later* pass over the
remaining 792 will reach anyway.

## Evidence

216 verdicts read, sampled with a fixed seed from those with a live bullet, in
eighteen batches. **4 false: 1.85%.** Intervals are Clopper-Pearson, the same
method the two earlier entries quote, checked here by reproducing their numbers:

| sample | false | rate | 95% interval | implied, over 1008 |
| --- | --- | --- | --- | --- |
| first 72 | 2 | 2.8% | 0.3% – 9.7% | 3 – 98 |
| first 168 | 2 | 1.2% | 0.1% – 4.2% | 1 – 43 |
| **all 216** | **4** | **1.85%** | **0.5% – 4.7%** | **5 – 47** |

The point estimate went *up* and the interval still narrowed, which is what
adding 48 reads to a small sample does. The 168-row interval's lower end of 1
was always the artefact of a run of zeros; 216 rules out "essentially none"
without moving the upper bound much. **Tens of false verdicts across the
bucket, not hundreds** is the same summary as before, now with a floor under it.

**Both new false claims were true when written and were overtaken by later
work.** That is 4 for 4 on this shape. The accessor one is datable to the
minute: the caveat was committed at 14:57 on 2026-09-20 (`2ae9f1f`) and
`83cf0ca` added the call at 16:38 — **one hour and forty-one minutes** later,
by a session that had no reason to look at a caveat in another entry.

### The lead, stated as a hypothesis

Three of the four false claims open with a negative existential — "Nothing
outside the demo calls it", "Nothing here is run by the new job", "nothing
schedules it". Measured, rather than asserted: **42 of the 216 read (19%)** have
that shape, and 195 of the 1008 live ones do. If false verdicts were spread
evenly, three-or-more negatives among four draws has probability **0.025**.

That is suggestive and it is **four events**. It is not a finding, and it is
recorded here so the next pass can test it rather than assume it: if the rate
among "Nothing…" claims is really several times the rest, the next 100 reads
should show it, and the way to check is to keep drawing from the unstratified
sample and tally the shape — not to start drawing negatives on purpose, which
would confirm the hypothesis by construction.

The mechanism, if it is real, is unsurprising: a negative existential is
falsified by *anybody adding one instance anywhere*, while a positive claim
about how something works is falsified only by changing that thing.

### The forty-eight

Twenty were checked mechanically against the tree rather than read for
reasoning, all holding. The ones worth naming:

| claim | checked against | holds |
| --- | --- | --- |
| the open verdicts name no owner and no plan | the 133 `open` rows carry only `by`/`checked`/`entry`/`key`/`verdict` | yes |
| nothing bounds an expression's *width* | `crates/slate-server/src/convert.rs`, `MAX_EXPRESSION_DEPTH` and no width counter | yes |
| `RUNTIME_ON_PURPOSE`'s reasons are unchecked prose | `scripts/check_wasm_runtime.py` checks the crate exists and is outside the closure, never the string | yes |
| the value-type coverage guard is Python's alone | `test_every_value_type_appears_in_a_pinned_table`, no Go or TypeScript sibling | yes |
| one adapter exists, so the list has one row | `scripts/mutate.py --help` prints `mutate_guard.py` and nothing else | yes |
| no brackets in a join's `WHERE` | `crates/slate-sql/src/sql.rs`: "takes `AND` only, brackets or no brackets" | yes |
| windows on a join are not covered | `convert.rs`: "a join computes no windows" | yes |
| nothing compares the three clients' freshness floors automatically | no `freshness` anywhere in `examples/explorer/conformance/` | yes |
| no client checks a batch's length before sending | no cap in any of the three clients' sources | yes |
| the search panel refetches on every keystroke | `examples/explorer/web/src/panels.tsx`, bare `onInput`, no debounce | yes |
| `purge.py` reaches its declaration dynamically | `importlib.import_module` on the `--schema` argument | yes |
| nothing expires a stamp on a change to the code | `scripts/caveats.py`'s `unread()` takes `days` and nothing else | yes |
| it does not close the toolchain-version gap | `check_toolchain_pins.py` is about Go pins, not clippy's version | yes |
| the `Record` doctests outside those eight are uncovered | no later entry mutates a doctest | yes |

One is worth a note rather than a row. *"Nothing outside the wasm crate and the
site"*, about table aliases, is now imprecise in its **location**: the alias
types moved to `crates/slate-sql` when the front end was extracted, and
`slate-serverd` links that crate for views. Its **substance** holds — a view is
refused anything past a single-table `WHERE`, so nothing a view can express
could carry an alias, and no client has a SQL surface. Left `deliberate`. Calling
it false on a crate boundary the caveat was not about would inflate the rate with
a rename.

### Mutations

Both new witnesses, record
[`ledger/mutations/20260929T201837-examples-retention-seed-py.json`](mutations/20260929T201837-examples-retention-seed-py.json)
and
[`ledger/mutations/20260929T201851-github-workflows-ci-yml.json`](mutations/20260929T201851-github-workflows-ci-yml.json):

| mutation | outcome |
| --- | --- |
| the retention example stops calling the generated accessor | caught, `check_closed_caveats.py` |
| the `scripts` job stops running the generator's own suite | caught, `check_closed_caveats.py` |

Aimed at the witnessed files rather than at the guard, which is the right
target: a witness row is only worth writing if removing the thing it watches
turns the guard red. An earlier run
([`20260929T201823`](mutations/20260929T201823-examples-retention-seed-py.json))
is the same case with a replacement that left the needle's semantics intact; it
is kept because a discarded attempt is part of the record.

`python3 scripts/check_closed_caveats.py`: **463 closed, 428 witnessed, 35
exempt**, all eight rules ok. `scripts/caveats.py`: 1855 caveats, 133 open, 109
narrowed, 463 closed, 1008 deliberate, 0 untriaged.

## What this does not do

**792 remain unread**, and the interval above is what 216 buys. Carrying it to
400 would roughly halve the upper bound again; nothing here shortens that.

**It does not test the "Nothing…" hypothesis**, it states it with a number and a
falsification plan. On four events the honest reading is that it might be the
whole explanation for the false rate or might be noise, and a stratified draw
would answer it faster at the cost of the unbiased rate.

**A verdict read for "the reasoning still applies" is graded by the same kind of
judgement that wrote it.** Twenty-eight of the forty-eight had no mechanical
subject — "an untested path is worse than a refusal", "a range is the honest form
for that finding" — and for those, "holds" means a second reader agreed, not that
anything was executed. That is the ceiling on this method and it does not move
with sample size.

**It says nothing about the 1008 that remain `deliberate` being *good* verdicts**,
only about how many are false. A true claim resting on a thin argument is not
what this counts, and the previous entry's one stale *reason* — a claim that
stayed true while its justification rotted — is a category this pass found no new
instances of, which is weak evidence that it is rarer rather than evidence that
it is absent.
