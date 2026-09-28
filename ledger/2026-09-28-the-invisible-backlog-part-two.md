# Triaging the 468, part two: the rest of the 14th through the 17th

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** process

## What changed

81 more verdicts, over the 2026-09-14 entries the first batch's listing cut
off and every entry dated the 15th, 16th and 17th. 22 closed, 4 narrowed,
12 moment, 29 deliberate, 14 open. Ten new witnesses and one new exemption kind,
`deleted`, for a closure whose evidence is a file that is gone.

The backlog is 468 → 283.

## Why

Same reason as part one, and the same result: a third of what looked like
standing limits had been answered. The demo frontend grew a batch button, a
predicate-write panel and a relationship view; `through` reached the wire, so
"no client has it" stopped being true for many-to-many *and* for nested
loading; the conformance runner covers relations; TypeScript has `related`.
None of that was connected back to the caveats that named it.

The 14 open ones cluster: **four are the same unmeasured claim** — batching
helps a client, a many-to-many is two reads, a nested load is two reads, keyset
paging is cheaper on the wire — all four structural rather than measured, and
all four measurable by the same deployed harness that does not exercise them.
That is one piece of work, not four, and it is now visible as one.

## Alternatives rejected

**Cite the ledger entry that closed each item.** Tried, and it produced three
more fabricated filenames — `2026-09-16-n2-a-ceiling-on-returning.md`,
`2026-09-16-n3-keyset-paging-over-a-join-or-a-chain.md`,
`2026-09-20-a-cte-is-a-name-for-a-subquery.md`, none of which exists. Eleven in
the previous batch, three here. The rule from here is to cite a **tree path**,
which is checkable by the same guard *and* by reading, rather than a filename
recalled from a task list. The entries are still findable; the verdict just
does not pretend to know their names.

**Take "all four were built" as the answer for the ORM-comparison caveat.**
The caveat names six absences. Five were built and the sixth, CTEs, was
decided — and the first draft of that verdict said `sql.rs` "refuses WITH by
name". Reading the file: non-recursive `WITH` is *implemented*, and only
`WITH RECURSIVE` is refused, because a fixpoint is a loop whose trip count is
unknown. That is the second verdict in two batches whose cited path resolved
and whose claim about it was wrong.

## Evidence

`scripts/caveats.py`: `25 open, 46 narrowed, 299 closed, 754 deliberate, 324
untriaged` → `33 open, 49 narrowed, 311 closed, 766 deliberate, 283 untriaged`.

`scripts/check_closed_caveats.py`: 291 of 311 closures now carry a witness in
the tree, 20 are exempt with a reason each. Every needle was verified with
`git grep` before its row was written — the helper that adds them refuses a
needle it cannot find, which is what made the CTE error visible.

`sh scripts/check.sh`: 71 passed, all of them.

## What this does not do

**283 remain**, all between 2026-09-18 and 2026-09-27, and they are the
densest part of the ledger rather than the thinnest.

**Two verdicts in two batches cited a path that resolves and said something
false about it**, which is not a coincidence and is not caught by anything. The
guard checks that a cited file opens; reading it is what catches the rest, and
reading it is what I did in both cases only because something else looked
wrong.

**The four unmeasured round-trip claims are grouped here and not measured.**
Naming them as one piece of work is the whole of what this entry does about
them.
