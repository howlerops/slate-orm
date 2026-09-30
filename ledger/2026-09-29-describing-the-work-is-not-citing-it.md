# A `by` that describes the work does not get a reader back to it

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `scripts/caveats.py`, `scripts/test_caveats.py`,
  `docs/caveat-status.json`
- **Kind:** guard

## What changed

`struck()` returns which ledger entries each strike credits, not just the keys,
and `report()` refuses a `closed` or `narrowed` verdict whose `by` names none
of them. Two of the three pairs in this repository were doing exactly that and
are fixed. The fixture suite went from 51 cases to 57.

The check is narrow on purpose: a strike that names no entry credits nobody and
leaves the `by` alone, which is 30 of the 51 strikes here.

## Why

The entry that added the strike rule left this open in as many words:

> **It does not check that the `by` names the entry the strike names.** The
> rule demands a settled verdict and a `by`; it does not read the strike's own
> text to see which entry it credits. A `closed` row citing the wrong entry
> passes.
> — `ledger/2026-09-29-a-caveat-closed-by-a-line-through-it.md`

It passes a wrong citation, and it also passes *no* citation, which turned out
to be the live failure. The rule fired on both pairs the moment it ran:

- `2026-09-19-decoders-over-the-servers-own-rows.md`'s decoder caveat said
  `by: "a live-row case for the remaining generated decoders"`. True, and it
  points nowhere: `2026-09-20-the-other-three-decoders.md` is the entry that
  did it, and the strike two lines above the caveat says so.
- `2026-09-19-forgetting-a-retired-row.md`'s `Nothing calls it.` said
  `by: "something calls purge_deleted, and a schedule for it"` — two entries'
  worth of work, neither named.

That is a *description of the work*, not a citation of it. The `by` field's
whole job is to be the way back from a verdict to the entry that settled it;
prose in that slot means somebody triaging this caveat again in a month has to
go and find the work by memory, which is the state the tracker exists to
replace. One in three pairs was already right, and it is the one whose `by`
names a file.

The strike is where the answer already is, and it is machine-readable: 21 of
the 51 strikes here name a dated entry. Comparing the two costs a regex.

## Alternatives rejected

**Read the strike as the verdict and write the `by` from it.** Tempting and
wrong for the reason the previous entry already gave about inferring `closed`
from a line through some Markdown: the strike is prose written by whoever
struck it, and a field generated from prose is a field nobody has checked. The
rule refuses a mismatch and makes a person reconcile it, which is one sentence
of work and leaves the claim theirs.

**Require every strike to name an entry.** It would make the rule total instead
of covering 21 of 51. Rejected because it is a rule about *how to write an
entry*, enforceable only at the `pre-commit` hook, and because 30 strikes are
prose for good reasons — "Withdrawn, the same afternoon" credits nobody because
nobody closed it, it was wrong. Demanding a citation there would produce
ceremonial ones.

**Require the `by` to name *every* entry the strike credits.** The second pair
credits two, because closing it took two steps. Rejected: naming either gets a
reader to the work, and the other is one hop away through that entry's own
prose. Demanding both turns a pointer into a bibliography, and the failure
being prevented is "no pointer at all". There is a fixture case pinning this —
a `by` naming one of two credited entries passes — because the choice is
invisible otherwise and `all` would have looked equally reasonable to the next
reader.

**Match any `.md` rather than a dated one.** Simpler pattern, and it credits
`docs/correctness.md` whenever a strike says where a finding was written up —
which is not somebody who closed a caveat. The date prefix is what separates a
ledger entry from every other Markdown file in the tree. This one is not
theoretical: it took three attempts to write a fixture that distinguished the
two patterns, because the first two used `b.md` and `README.md` and neither is
what the loose pattern would wrongly match.

## Evidence

- `python3 scripts/caveats.py` refused two verdicts on its first run, both
  predicted by the caveat this closes, both now naming their entries. Counts
  unchanged either side: **1796 caveats, 121 open, 96 narrowed, 454 closed,
  986 deliberate, 0 untriaged** — this changes what a `by` says, not what a
  verdict is.
- `python3 scripts/test_caveats.py`: **57 passed, 0 failed**, up from 51.
- **Six mutations, in five runs.** Three survived on first attempt and each was
  a missing test rather than redundant code, which is the whole reason the runs
  are listed separately:
  - `ledger/mutations/20260929T164152-scripts-caveats-py.json` — the `by` check
    removed, and the empty-credit guard removed so a prose strike demands a
    citation. Both caught. `an entry crediting itself counts as a pointer
    elsewhere` **survived**: nothing in the fixtures had a self-crediting
    strike.
  - `ledger/mutations/20260929T164224-scripts-caveats-py.json` — the credited
    set discarded: caught. The self-credit mutation **survived again**, because
    the fixture I added named `a.md` and the pattern wants a dated filename;
    and `all` for `any` **survived**, because no fixture credited two entries.
  - `ledger/mutations/20260929T164303-scripts-caveats-py.json` — both of those
    caught, once the self-credit fixture was given a dated name and a
    two-credit fixture existed. `the credit pattern stops requiring a date`
    **survived**: the fixture used `README.md`, and `[a-z0-9-]+` does not match
    an uppercase name, so the loose pattern behaved identically.
  - `ledger/mutations/20260929T164328-scripts-caveats-py.json` — the same
    mutation, **surviving one more time**, which is what said the fixture and
    not the rule was wrong.
  - `ledger/mutations/20260929T164345-scripts-caveats-py.json` — caught, with
    the fixture naming `docs/correctness.md`, which is both lowercase and what
    a real strike actually cites.
- `sh scripts/check.sh`: **85 passed, all of them**, exit 0.

## What this does not do

**It does not check that the credited entry is the *right* one.** A `by`
naming an entry the strike also names passes, whether or not that entry has
anything to do with the caveat. The remaining half of the original caveat —
*"a `closed` row citing the wrong entry passes"* — is narrowed to "a row citing
an entry the strike does not credit is refused", and a strike crediting the
wrong entry in the first place is not checkable from here.

**Thirty strikes credit nobody and are unchecked.** Measured, not estimated: 21
of 51 name an entry. The rest are prose, and a `by` beside one can say anything.
Whether those 30 *should* name an entry is a question about the ledger's
conventions that this does not answer and this rule deliberately does not force.

**It reaches three caveats.** The rule can only fire on a struck caveat whose
standing twin is in the tracker, and there are three of those — the same three
the rule above it was built for. Its value is not today's two fixes but the
next pair, and there is no evidence about how often that arrives; the previous
entry said one in three is not a rate worth reasoning from, and two in three is
not either.

**A strike that paraphrases still pairs with nothing.** Unchanged from the
previous entry: matching is on the 60-character key, so a strike wording the
claim differently is invisible, and looks exactly like an entry with no strike.
Both of this file's rules inherit that.
