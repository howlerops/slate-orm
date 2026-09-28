# Triaging the 468, part one: 2026-09-13 and 2026-09-14

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`
- **Kind:** process

## What changed

104 verdicts, for every caveat in the 41 entries dated 2026-09-13 and
2026-09-14 that `ledger/2026-09-28-the-tracker-could-not-see-a-third-of-the-caveats.md`
made visible. On the 13th: 54 caveats — 28 closed, 5 narrowed, 2 moment, 13
deliberate, 6 open. On the 14th: 50 — 12 closed, 4 narrowed, 4 moment, 19
deliberate, 11 open.

## Why

Those two days are where the recall hole is worst — 55 invisible caveats
against 1 visible on the 13th, 93 against 34 on the 14th — and they are also
the oldest, so they are the ones most likely to have been quietly closed by
work nobody connected back to them. That turned out to be true: **40 of the 104
were closed by later work**, including six client features, the site's
deployment, grouping a chain on the wire, EXPLAIN for a grouped read, and the
demo's configurable ports. None of that was visible from the tracker, because
the tracker could not see the caveats.

The 17 now standing `open` are the honest residue. They are genuinely small and
genuinely unbuilt: an uncapped `IN` list, a per-connection rather than
node-wide concurrency bound, ordinals rather than column names in a grouped
EXPLAIN, a bar chart asserted on count rather than geometry, unpublished
packages named in a quickstart's install line.

## Alternatives rejected

**Mark the old ones `moment` wholesale.** Tempting — they are two weeks old and
a lot has happened — and wrong for all but five of them. `moment` means a claim
that was never a standing one: "the deploy job has run zero times", "the panel
opens on `books`". A claim that something is unbuilt is a standing claim, and
calling it a passing observation would delete work rather than record it.

**Mark them `closed` where a later task plausibly covered them.** Faster and
exactly the failure this repository keeps recording: a verdict's `by` must name
what closed it, and a plausible name is how seven fabricated citations got
written. The closures were checked against the tree rather
than against memory before the `by` was written — and four `by` fields I did
write from memory named ledger entries that do not exist, which
`scripts/check_caveat_citations.py` refused.

**Do it in one pass over all 468.** Rejected on the same grounds a batch of ten
was chosen for the original triage: a verdict written without reading the tree
is a verdict that looks triaged, and 468 of those is worse than 468 untriaged.

## Evidence

`scripts/caveats.py` went from `1536 caveats: 1 open, 36 narrowed, 249 closed,
703 deliberate, 468 untriaged` to `18 open, 45 narrowed, 289 closed, 735
deliberate, 364 untriaged`.

`scripts/check_caveat_citations.py` refused this batch three times before it
passed, each time on a fabricated ledger filename — nine in the first round and
two in the second. Every one was a plausible name recalled rather than looked
up: `2026-09-16-having-on-a-join-or-a-chain.md` for
`2026-09-15-having-on-a-join.md`,
`2026-09-14-move-the-python-testserver-into-the-root-workspace.md` for
`2026-09-14-the-testserver-joins-the-workspace.md`. That is the eighth through
the eighteenth invented citation this repository has recorded, all caught, and
the reason the guard exists.

One correction went further than a filename. A verdict claimed
`crates/slate-kernel/src/plan.rs` "costs a grouped join as grouped". The path
resolves, so the guard passed it — and reading the file, nothing there does
that. What actually happened is better: the item was **withdrawn** with a
demonstration, in
`the_per_row_term_is_symmetric_so_grouping_cannot_flip_the_algorithm`
(`crates/slate-kernel/tests/grouped_chain_oracle.rs`), which shows the
per-joined-row term is added to the hash cost and the loop cost equally, so
discounting it for a grouped join changes no plan at all. The verdict says that
now.

## What this does not do

**364 remain, and they are the 2026-09-15 to 2026-09-27 entries.** Nothing
about this batch makes the rest cheaper; they are the same work at the same
rate.

**A verdict citing a path that resolves can still be wrong about it**, which
the grouped-cost correction above demonstrates rather than argues.
`check_caveat_citations.py` asks whether a cited file opens, never whether it
says what the citing sentence claims — the same limitation
`ledger/2026-09-26-the-citation-nobody-could-follow.md` records as the larger
half. One of the forty closures here was caught that way; nothing
guarantees the other thirty-nine were read as carefully.

**The `open` verdicts name no owner and no plan.** They are a list of seventeen
true statements about what is unbuilt, which is what the tracker is for, and
not a decision about which of them is worth building.
