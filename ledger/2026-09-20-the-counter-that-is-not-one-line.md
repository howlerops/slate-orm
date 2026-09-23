# Withdrawing "small": a restore counter is a two-crate change

- **Date:** 2026-09-20
- **Author:** Claude, correcting a claim in a note committed an hour earlier
- **Touches:** `docs/undo-window.md`, `ledger/2026-09-20-where-an-undo-window-lives.md`
- **Kind:** docs

## What changed

Two sentences. `docs/undo-window.md` said a restore counter would be "small";
it is not, and now says what it actually costs and why. The entry that shipped
that note carries a pointer here.

## Why

The note weighed three shapes for an undo window and said of the third, an
audited restore, that the cheap half was within reach:

> `write_many` already reads the previous row, so "the stamp went from set to
> null" is known where the write happens. Counting it —
> `slate_rows_written_total{statement="restore"}` — is small.

The first half is true. The second does not follow, and I wrote it without
checking. **`WriteObserver` is in `slate-server`, not the kernel.** It is fed
by the session's `Tally`, which names statements from the daemon's own request
arms — it knows `update` because that is the RPC it served, and it has no way
to learn that a particular update cleared a soft-delete stamp. The kernel knows
the transition and has no observer; the server has the observer and cannot see
the transition.

Surfacing it therefore means the kernel *returning* the count. `write_many` is
`Result<()>`, and so are `insert_many`, `upsert_many` and `update_many` — a
public API change in `slate-orm`, plus a change in `slate-server` to carry it
into the `Tally`. That is a different decision from adding a label to a counter,
and the note was inviting a reader to make it on the wrong information.

Worth its own entry rather than a quiet edit, because the note is three commits
old and **this is the second time today a claim of mine turned out to be an
assumption I had not checked** — the first being "a thousand-row upsert
allocated a thousand errors", which measured as noise. Both were plausible,
both were written in a "what this does not do" section, and that section is
exactly where an unchecked claim is least likely to be challenged.

## Alternatives rejected

**Build the counter, so the note needs no correction.** It would close the item
and it would be me making the API decision the note exists to hand to somebody
else — the third shape is the one a regulated deployment asks for, and its
design should not be settled by whoever happened to want a tidy ledger.

**Edit the sentence silently.** The note is committed and pushed, so the wrong
version is already the record; correcting it without saying so leaves the ledger
claiming a note that was right all along. The repository's own standard is to
withdraw a claim and say that you did.

**Delete the bullet.** It is still the right thing to describe: a restore
counter *is* the cheapest step toward shape 3, and a reader should know it
exists. What was wrong was the price, not the option.

## Evidence

`grep -rn "WriteObserver" crates/` puts the trait, its registration and its only
consumer in `crates/slate-server/{lib,service,session}.rs`. Nothing in
`crates/slate-kernel` references it. `write_many`, `insert_many`, `upsert_many`
and `update_many` all return `Result<()>`, so there is no existing channel for a
count to travel back through.

No test changed and none needed to: this is a claim about what a change would
cost, and the correction is that the claim was unmeasured.

## What this does not do

**It still does not build the counter**, and the note still does not recommend
one — it now states the price correctly so the decision can be made on it.

**It does not audit the rest of the note for the same fault.** The other claims
there — that a time-bounded policy works today, that the config language has no
clock, that a restore is indistinguishable in the log — were each checked
against the code when written, and two of them have a test. "Small" was the one
I asserted from shape rather than from reading, and I have not re-derived the
others from scratch.

**It does not change how `WriteObserver` is structured.** That the kernel knows
the transition and the server owns the observer is a real seam and might be
worth closing; nothing here proposes it.
