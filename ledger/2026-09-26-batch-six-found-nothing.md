# Ten more open caveats read; all ten are still true. A null batch, written up because a rate estimated from the batches that found something is not a rate.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `docs/caveat-status.json`
- **Kind:** process — re-triage, no code, no closures

## What changed

Ten caveats from 2026-09-16 and 2026-09-17 checked against the tree. **All ten
are still true**, stamped with today's date. Nothing closed, nothing narrowed,
no code touched.

## Why

Because a batch that finds nothing has to be written down as carefully as one
that finds something, and it is the batch most likely not to be.

Five batches today closed one each and the sixth closed none. If only the
productive batches leave a trace, the record says "the reading pass closes
about one caveat per batch", which is the number the previous five entries
would support on their own and is not what the evidence says. Six batches, five
closures: **1 in 12**, against the 1-in-8 from the earlier unbiased sample of
eighteen.

That is the estimate this entry exists to protect, and
`ledger/2026-09-26-batch-four-and-the-shape-that-keeps-recurring.md` already
paid for it once by declining to read the rich seam first. Declining the seam
and then not recording the lean batch would have thrown the payment away.

## Alternatives rejected

**Stamp them and skip the entry.** The pre-commit hook requires an entry for
any change outside `ledger/`, and `docs/caveat-status.json` is such a change,
so *an* entry was going to exist. The real choice was between a one-line entry
and this one. A one-line entry records that ten were stamped and loses the
thing worth keeping, which is that ten were stamped and none moved.

**Fold it into the next batch's entry.** Cheaper, and it turns a null result
into a footnote on a positive one, which is the shape of reporting that makes a
method look better than it is.

**Re-estimate the rate from six batches.** 5 in 60 is a tighter interval than 2
in 18, and combining them properly means deciding whether the batches are
comparable samples — they are not: the first eighteen were drawn at random and
these sixty are the head of a list ordered by date. Stating both and combining
neither is the honest reading. No number is revised here.

## Evidence

| caveat | checked against |
|---|---|
| the browser render was a one-off, not a check | `site/check/` holds `docs.py`, `quickstarts.py`, `workbench.py`; `docs.py` has **zero** occurrences of `playwright`, so no CI job renders a docs page |
| no predicate, ordering or limit per level | `message Relation` (`crates/slate-server/proto/slate/v1/records.proto:1912`) is `table`, `foreign_key`, `direction` and nothing else |
| the depth is bounded; the fan-out is not | `max_returned_rows` is read at `main.rs:678` and its own refusal message scopes it to `RETURNING` |
| nothing measures a path against two calls | no path or relation timing in `docs/performance.md` |
| `ErrorInfo.metadata` still is not surfaced | no occurrence of `ErrorInfo` in `crates/slate-serverd/src/` |
| one captured blob, one shape of error | no Python test builds a status with two details or none |
| the cap is a row count and the thing that breaks is bytes | unchanged |
| the memory cost of a predicate write is untouched | `matching_rows` (`crates/slate-kernel/src/record.rs:2084`) still collects into a `Vec<Row>`, and `at_most` is `None` for a write that does not return |
| nothing measures how much the early stop saves | no "early stop" anywhere in `docs/performance.md` |
| nothing prevents the next formatting failure | `scripts/check.sh:79` runs `cargo fmt --all -- --check` — see below |

**One near-closure, recorded as still open.** The formatting caveat says "no
other guard was added: this rests on `cargo fmt --all -- --check` being run
last, by a person who remembers to." `scripts/check.sh` did not exist when that
was written — its first commit is **2026-09-19**, two days later — and it runs
exactly that command as one of 57 checks, reporting at the end rather than
stopping. So what a person must remember shrank from "run `fmt` specifically,
and last" to "run `check.sh`".

It is still a person remembering, so the sentence "nothing prevents the next
one" is still literally true and the caveat stays open. It is also *smaller*
than it was, and `CLAUDE.md` records the reason it cannot be closed here at
all: CI's `rustfmt` is newer than this container's, and "there is no local
command that catches this". Left open rather than rewritten, because an entry
is append-only and a caveat that is narrower than it reads is better served by
this note than by a status that says closed.

**No mutation run.** Nothing executable changed; the only edit is ten `checked`
stamps in `docs/caveat-status.json`, which `scripts/test_caveats.py` covers.

**Counts.** `scripts/caveats.py`: **842 caveats: 289 open, 185 closed, 314
deliberate, 0 untriaged** before this entry's own two open caveats were
added; **218 open caveats are unstamped** with them counted.

## What this does not do

**It does not make the null result cheap.** Ten caveats read, ten greps and
file reads, no change to the repository. That is the honest cost of the method
and this entry is the only place it is visible: the other five entries today
each have a closure to point at.

**Two of the ten were checked by absence.** "No occurrence of `ErrorInfo`" and
"no path timing in `docs/performance.md`" are searches that found nothing,
which is weaker than finding the thing and reading it. A surfaced
`ErrorInfo.metadata` that some other file spells differently would pass this
check.

**The formatting caveat is now wrong in a way no status can express.** It reads
as "nothing was done" and the truth is "something was done that helps and does
not close it". The tracker has `open`, `closed`, `deliberate` and `moment`, and
none of them is "narrower than written". This note is the workaround; a fifth
verdict is not obviously worth it for one caveat, and this is the second today
(the 369-to-491 attribution was the first) so it may become worth it.
