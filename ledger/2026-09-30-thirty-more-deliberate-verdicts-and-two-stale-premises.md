# Thirty more `deliberate` verdicts read, and both false ones were premises about facts outside this tree

- **Date:** 2026-09-30
- **Author:** Claude Opus 5
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`, two ledger entries
- **Kind:** process

## What changed

Thirty of the 1039 `deliberate` verdicts were drawn at random and read against
the tree. Two were false. Both are now corrected: `main` **is** the default
branch, so `2026-09-14-main-and-the-documentation-sweep.md`'s caveat is
`closed`; `ErrorInfo.metadata` **is** surfaced, by all three clients, so
`2026-09-17-a-path-of-relationships-on-the-wire.md`'s is `narrowed` with the
residual named. Both entries carry a struck-through copy of the bullet
crediting this one, and the twenty-eight that held carry a fresh `reviewed`
date. `check_closed_caveats.py` gains a `setting` exemption kind, for a closure
whose evidence is a repository setting on the hosting service.

## Why

Four earlier entries built this sample to 216 reads and 4 false — 1.85%, with
the running summary "tens across the bucket, not hundreds". The bucket is the
largest in the tracker and the least examined per row, and
`2026-09-29-the-deliberate-sample-carried-to-216.md` asked the next pass for
two specific things: more reads, and a tally of whether "Nothing…" claims fail
more often. This is thirty more reads.

## Alternatives rejected

**Continuing the existing draw** rather than starting a new one. It is the
right thing and it is not possible: none of the four entries records its seed
or which rows it read, and the reverse sweep stamped `reviewed` on 434 rows in
bulk, so the dates cannot separate a campaign read from a sweep stamp. That is
a real defect in the campaign's method and is recorded below rather than
papered over. This draw's seed is written down.

**Drawing "Nothing…" claims on purpose** to test the standing hypothesis
faster. Refused for the reason the previous entry gave: it would confirm the
hypothesis by construction. The unstratified draw is the only one whose rate
means anything.

**Leaving the default-branch caveat `deliberate`** on the grounds that the
reasoning ("a repository setting no commit can change") is still sound. The
reasoning is sound and the *claim* is false, which is exactly the distinction
this audit exists to draw: a verdict certifies the claim, not the argument.

## Evidence

The draw: `random.seed(329)`, `random.sample(deliberate, 30)` over the 1039
`deliberate` rows in `docs/caveat-status.json` at commit `3387f4c`.

**Two false, of thirty.** Clopper-Pearson, computed here (the bisection
reproduces the 216-row entry's published 4.7% upper bound):

| sample | false | rate | 95% interval | implied, over 1037 |
| --- | --- | --- | --- | --- |
| the earlier 216 | 4 | 1.85% | 0.5% – 4.7% | 5 – 48 |
| **this 30** | **2** | **6.7%** | **0.8% – 22.1%** | **8 – 229** |
| pooled, 246 | 6 | 2.4% | 0.9% – 5.2% | 9 – 54 |

The Pages claim, closed after this table was written, is **not** counted in it:
it was not in the draw, it was read because it sat beside one that was, and
adding a found-by-looking row to a random sample would bias the rate upward.
It is a thirty-first claim examined and a third false one, and it belongs to
the shape below rather than to the arithmetic above.

**The pooled row is an upper bound on precision, not a real pooling**: the two
draws are independent, so an unknown number of these thirty were among the 216
and any overlap is double-counted. "Tens, not hundreds" survives either way.

The two false ones, and how each was checked:

| claim | checked | what is true now |
| --- | --- | --- |
| "`main` is not yet the **default** branch" | `GET /repos/howlerops/slate-orm` | `"default_branch": "main"` |
| "`ErrorInfo.metadata` still is not surfaced" | all three clients' detail decoders | `violationsOf` reads `info.GetMetadata()`; `_details.py` reads `dict(info.metadata)`; `details.ts` decodes the map |

**Both were true when written and were overtaken.** That is 6 for 6 on the
shape the 216-row entry named. Neither, though, is the "Nothing…" negative
existential that entry flagged: one is a claim about a setting on GitHub, the
other a claim about a protocol capability that a later feature added for a
different reason. Their common shape is narrower and worse — **a premise about
something outside this tree**, which no guard here reads and no reviewer of a
diff would think to re-check.

Twenty of the twenty-eight that held were checked mechanically, not re-argued.
The ones worth naming:

| claim | checked against | holds |
| --- | --- | --- |
| no `#[derive(Record)]` support for arrays | no `Array` or element-type attribute in `crates/slate-derive/src/lib.rs` | yes |
| `clients/python/testserver` is outside the swept tree | `check_build_stamp.py`'s `CRATES = ROOT / "crates"` | yes |
| the demo UI does not use the decoders | `catalog.ts` is names only, "no import, nothing from the SDK" | yes |
| nothing in the browser writes a schema-state key | `slate-wasm/src/lib.rs`: the `0x03` arm exists because nothing reaches it | yes |
| no client knows about the stream cap | `RESOURCE_EXHAUSTED` maps to a refusal in all three | yes |
| `Catalog::insert`'s lookup is invisible to rule 10 | `catalog.rs:59`, spelled as the caveat says | yes |
| no client parses a decimal string back | no such function in any of the three | yes |
| there is no way to ask whether a row is restorable | no `restorable`/`can_restore` anywhere in `crates/` | yes |
| the claims guard reads numbers and names, not sentences | `check_site_claims.py`: "Neither reads the *prose*" | yes |
| fenced blocks are unread | `check_site_claims.py:107` strips ```` ``` ```` before matching | yes |
| the whole-word rule accepts a name in any string | `check_site_css.py:141`, and the case defending it | yes |
| reconcile runs only on the leadership winner | `slate-serverd/src/main.rs:238` | yes |
| idle handles are unbounded | `streams.rs`: "the permit goes back immediately" | yes |
| nothing checks the rosters' *reasons* | no guard reads a reason string | yes |
| the two-workflow complaint carries both values | `test_check_sh.py:325` | yes |
| the duplicated body is keyed on its digest | `check_table_provenance.py:197` | yes |

Eight were reasoning with no mechanical subject — "it is the same reader", "the
messages are read by people, not parsed" — and were read for whether the
reasoning still applies. It does.

`sh scripts/check.sh`: 87 passed, all of them.
`python3 scripts/caveats.py`: 1913 caveats, 141 open, 116 narrowed, 473 closed,
1037 deliberate, 0 untriaged.
`python3 scripts/check_closed_caveats.py`: 473 closed, 436 witnessed, 37 exempt.

## What this does not do

**It cannot say how much of this draw is new.** The four earlier entries record
neither their seed nor their read set, so the 246-row pooled line above is an
optimistic bound rather than a count. Fixing it properly means recording the
read set, not just the date; this entry records its own seed, which is the
cheapest half.

**A `reviewed` stamp does not distinguish a read like this one from the 2026-09-26
reverse sweep's bulk stamp.** Both write the same field. The twenty-eight
stamped today were read against code, which is a stronger claim than the field
can carry, and nothing in the tracker records which kind a date is.

~~**The sibling claim in the same entry could not be checked.**~~ **Closed
within the hour, by the reader**, and recorded in
`ledger/2026-09-14-main-and-the-documentation-sweep.md` beside its sibling. The
claim was stale: `https://howlerops.github.io/slate-orm/` answers 200 and
serves the site. So **three** false of thirty-one claims examined, not two of
thirty, and the shape named above is 3 for 3 in one draw. The lesson is smaller
and sharper than the paragraph below it: I reached for the API the caveat's own
author had been refused, and stopped when that was refused too. Fetching the
published URL costs nothing and needs no token. A premise about the outside
world is checkable from more than one side, and giving up on the first is how
it stays stale. The original, kept for the record:

**The sibling claim in the same entry could not be checked.** "Pages is not
enabled, and cannot be enabled from here" is the same shape as the
default-branch one and is likely to have moved with it, but
`GET /repos/howlerops/slate-orm/pages` is refused by this container's proxy
(403, "not permitted through this proxy"). Left `deliberate` and unverified
rather than guessed at.

**Two per thirty is four events short of a rate.** The interval runs from 8 to
229 verdicts over the bucket; the point estimate moving from 1.85% to 6.7% is
well inside what thirty draws can do by chance, and this entry claims no trend.

**The "Nothing…" hypothesis is neither confirmed nor refuted here.** Neither
false claim has that shape, which is one draw's worth of weak evidence against
it, and the previous entry's 0.025 rests on four events. Five of these thirty
open with a negative existential and all five held.
