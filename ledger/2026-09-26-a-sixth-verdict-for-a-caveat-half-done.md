# Three caveats in one day described a gap wider than the one that remains, and the tracker had no word for that. `narrowed` is the sixth verdict: part answered, part not, and `by` must say which is which.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `scripts/caveats.py`, `scripts/test_caveats.py`, `docs/caveat-status.json`
- **Kind:** process — a verdict, and batch ten of the re-triage pass

## What changed

`scripts/caveats.py` gains a sixth verdict. `narrowed` means part of the claim
has been answered and part has not; `by` is required and must name **both** —
what closed it and what is left. It counts separately in the summary, and
`--unread` lists it beside `open`, because the residual is still work and still
wants re-reading. Four new cases in `scripts/test_caveats.py`. Three caveats
carry it. Batch ten of the reading pass is stamped alongside: nine still true.

## Why

Because I hit the shape three times in one day and wrote a note about it each
time, and the second note said a fifth verdict "may become worth it".

- **"Nothing prevents the next formatting failure"** (2026-09-17). `scripts/
  check.sh` was first committed two days later and runs `cargo fmt --all --
  --check` as one of its checks. What a person must remember shrank from that
  command, run last, to one script. That a person must remember did not change,
  and `CLAUDE.md` records why it cannot be closed here at all: CI's `rustfmt`
  is newer than this container's and no local command catches the gap.
- **"Nothing is attributed between 369 and 491"** (2026-09-15). The caveat
  named four components. `docs/performance.md:1780` has since measured one of
  them — the commit's conflict history, at two allocations a row and no
  measurable bytes. Three remain.
- **"The demo and the docs site show no array"** (2026-09-20).
  `site/docs/features.html` has an "Arrays, ordered by prefix" section. The
  demo still shows none, deliberately: `posts` is in `NOT_IN_THE_UI` because
  nothing seeds it and the UI has no list cell.

Each was left `open`, three times, for the same reason: closing it erases a
true residual. And each then reads as more missing than is missing, which is
the cost `open` was quietly charging. A backlog that overstates itself is read
less carefully, which is the failure mode this whole tracker exists against.

**The shape is not rare and will not stop.** A caveat here is a sentence, not a
ticket, and a sentence often names two things. When one gets done the claim is
half false and half true, and neither `open` nor `closed` can say so. `moment`
was added for the same reason — 677 caveats could not be sorted into four
verdicts without lying about one in twelve — and this is that argument again,
one verdict later.

## Alternatives rejected

**Rewrite the caveat to describe only the residual.** The obvious fix and the
one `ledger/README.md` forbids: an entry is dated and append-only, and one that
gets rewritten when the world changes is not a record. It is also the exact
reason `docs/caveat-status.json` exists as a separate file.

**Close it and open a fresh caveat for the residual, in a new entry.** This
works, and it is what I did for the mutation-spec caveat earlier today, where
the residual was large enough to be its own piece of work with its own cost.
It is wrong for the small ones: it manufactures an entry per half-closure, and
the new caveat loses the thing worth keeping — that somebody already looked at
this and found most of it done.

**Split the verdict into `mostly-closed` and `mostly-open`.** Two words for a
distinction nobody can draw, and a reader would have to guess which half the
author thought bigger. One word plus a required `by` puts the judgement in
prose, where it can be read.

**Leave it at five and keep writing the note in the entry.** What happened
three times. Each note is correct, each is in a different file, and none of
them is where somebody reading the tracker looks. The status of a claim is not
the claim, and "partly" is a status.

## Evidence

**Mutations: four run, four caught**, each by a named case:

| mutation | caught by |
|---|---|
| `narrowed` dropped from `VERDICTS` | both new verdict cases, which then read `unknown verdict 'narrowed'` |
| `narrowed` not held to naming a `by` | `narrowed without naming what closed and what is left is refused` |
| `narrowed` dropped from `--unread`, so its residual falls off | `a narrowed caveat is re-read like an open one` |
| every verdict re-read, not just the unsettled ones | `closed and deliberate are not re-read`, and two older cases |

The runs themselves, as `mutate.py` recorded them:

- `ledger/mutations/20260926T010015-scripts-caveats-py.json`
- `ledger/mutations/20260926T010215-scripts-caveats-py.json` — the two that touch the changed lines, re-run after `ruff format` rewrote the file

**`scripts/test_caveats.py`: 29 passed, 0 failed** (25 before).

**Counts.** `scripts/caveats.py` now reports **856 caveats: 289 open, 3
narrowed, 188 closed, 320 deliberate, 0 untriaged.** The three narrowed came
out of `open`, which is the first time today that number has fallen for a
reason other than a closure.

**Batch ten, nine still true:**

| caveat | checked against |
|---|---|
| `posts` is not in `crates/slate-server/tests/common/mod.rs` | zero occurrences |
| the ruff guard does not check `ty` for the same hazard | no equivalent check |
| two occurrences, one rule | unchanged |
| `any_scalar` is a hand-written list | `crates/slate-tuple/tests/untrusted.rs:175` still documents what it deliberately omits |
| the codec's ordering properties were not attacked | no ordering property in the fuzz tests |
| the merged `RESTRICT`/`CASCADE` test was not verified to cover both | unchanged |
| no test exercises a duplicated header over a real connection | unchanged |
| the clients were not checked for sending it twice | unchanged |
| one property, not a contract | unchanged |

## What this does not do

**It does not go back over the 188 closed and 320 deliberate verdicts looking
for ones that should be `narrowed`.** Three were found by reading thirty
caveats; the same rate over the settled ones would be a hundred re-reads for a
distinction that matters most on the open list, where it changes what somebody
picks up next.

**`narrowed` has no `residual` field.** The residual lives in the `by` prose,
so nothing can count how much work the narrowed caveats represent, and nothing
stops a `by` that names what closed and forgets what is left. The test refuses
an empty `by` and cannot read one.

**Nothing promotes a `narrowed` to `closed` when its residual closes.** It will
come round on the `--unread` list like any other, and a reader will have to
notice. That is the same hand-reading the whole pass depends on.

**Three is a small sample for a new verdict.** It was three in thirty reads
today; whether the rate holds over the remaining 190 unstamped is unknown, and
if it turns out to be three in total then a sixth verdict was more machinery
than the problem needed.
