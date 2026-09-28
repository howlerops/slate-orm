# Triaging the 468, part three: the 18th and the 19th

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** process

## What changed

153 verdicts over the two densest days in the ledger: 29 closed, 4 narrowed,
14 moment, 65 deliberate, 41 open. 17 new witnesses. The backlog is 283 → 130.

## Why

These two days are where the validation work, the observability work and the
soft-delete work all landed, and each of them shipped as a chain of entries
whose caveats named the next link. That is visible now in the numbers, counted from the verdicts rather than
remembered: **`crates/slate-serverd/src/metrics.rs` closed two caveats** in two
entries that had each written "no scrape endpoint" as a standing limit,
**the published check metadata closed five**, and
**`crates/slate-kernel/src/record.rs` closed three** between `restore` and
soft delete.

The 41 open ones are not a backlog of the same kind. **Six of them are one of
two sentences**, repeated across entries that each met it from a different
side:

  * *nothing checks a client's declared scale against the server's* — three
    entries say it, and it is the same hole each time. The fingerprint
    deliberately does not hash a scale, because a scale addresses no column, so
    a client declaring scale 2 against a scale 4 column reaches the right
    column and renders every value a hundred times wrong, for ever, with no
    error at any layer. That is the sharpest thing standing open in this
    repository and it is a protocol decision rather than a defect.
  * *timed to the response head* — three entries, and the same consequence: a
    streamed read's rows are in no latency anywhere, including the exported
    histogram.

Both were visible before only as scattered sentences. Grouped, each is one
piece of work.

## Alternatives rejected

**Close the five scale caveats as `deliberate`, since the protocol chose it.**
The protocol chose not to publish a schema; it did not choose for a client to
render money a hundred times wrong in silence. `deliberate` requires an
alternative and a reason it was not taken, and the alternative here — hashing
the scale into the fingerprint, or carrying it on the wire — has not been
weighed anywhere. `open` is the honest verdict for a hole nobody has decided
about.

**Collapse the six duplicates into one caveat.** Not available:
`ledger/README.md` forbids editing an entry, and each of the three scale
sentences is a real claim in a real dated record. What the tracker can do is
give them the same verdict text, which it now does, so a reader meets one
thing said three times rather than three things.

## Evidence

`scripts/caveats.py`: `57 open, 51 narrowed, 325 closed, 813 deliberate, 191
untriaged` → `76 open, 53 narrowed, 340 closed, 832 deliberate, 130 untriaged`,
counting both days.

`scripts/check_closed_caveats.py` refused this batch twice — once for a witness
name already defined, once for fifteen closures with none — and the helper that
adds them refuses a needle `git grep` cannot find, so every witness here was
checked against the tree rather than remembered.

`sh scripts/check.sh`: 71 passed, all of them.

One thing the batch got wrong and corrected: seventeen verdicts were keyed with
the `**` markers included. The key is the bold *text*, so all seventeen missed,
and the helper reported all seventeen rather than stopping at the first —
which is the change made two batches ago after one mistyped prefix cost a
re-paste of forty rows.

## What this does not do

**130 remain**, 2026-09-20 to 2026-09-27.

**The two grouped findings are named, not fixed.** The scale hole and the
response-head latency are each one piece of work now instead of six sentences,
and neither has been started.

**A `deliberate` verdict quoting an entry's own reasoning is not independent
judgement.** Sixty-five of the 153 are that shape: the entry weighed an
alternative, wrote down what it would cost, and the verdict repeats it. That is
the right verdict when the reasoning holds, and nothing here re-derives whether
it still does — a caveat whose reasoning has quietly stopped applying reads
exactly like one whose reasoning holds.
