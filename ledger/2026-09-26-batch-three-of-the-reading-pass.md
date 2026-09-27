# Ten more open caveats read against the tree. One was already closed — by a guard written the same afternoon it was recorded, in a neighbouring entry nobody joined up.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `docs/caveat-status.json`
- **Kind:** process — re-triage, no code

## What changed

Ten `open` caveats from 2026-09-15 were checked against the code rather than
re-read from their entries. Nine are still true and carry today's date. One is
closed: **"Nothing checks the `slate-wasm` bundle's freshness the same way."**
`site/check/workbench.py` has `stale_sources()`, which fails the run when any
`.rs` or `Cargo.toml` under `slate-wasm`, `slate-kernel`, `slate-schema` or
`slate-tuple` is newer than `site/slate_wasm_bg.wasm`. The eight caveats
yesterday's two entries added are triaged too, so `untriaged` is back to zero.

## Why

Because the reading pass is the only method shown to shrink the open list
(`ledger/2026-09-26-two-ways-to-find-a-stale-caveat-that-do-not-work.md`
records the two cheaper heuristics that do not work, with the measurements),
and it only shrinks it one batch at a time.

The one closure is worth more than its arithmetic. The caveat was written in
`2026-09-15-a-month-is-not-a-number-of-seconds.md`; the guard that answers it
landed in `2026-09-15-one-table-twice.md`, the **same day**, and its docstring
describes the exact failure the caveat predicted — a check run against a bundle
built ninety minutes earlier, reading as "aliases do not work" instead of "you
are running last hour's code". Two entries, hours apart, one predicting a gap
and the other filling it, neither aware of the other.

That is the third instance of the same shape. Both stale caveats found in the
first sample (`…-two-ways-to-find-a-stale-caveat…`) were "waits for X" where X
had shipped, and this is a fourth. The shape is not "the caveat aged out"; it
is **two entries written close together that do not cite each other**. What
makes it invisible is that the closing entry has no reason to mention a caveat
in a file it never opened.

## Alternatives rejected

**Grep for the shape and close the rest in bulk.** Exactly the heuristic
already measured and found useless: a regex for blocker language hit 4 of 267
and all four were false. That the four known-stale caveats share a *semantic*
shape does not make the shape findable by text — "waits for X" is almost never
written in those words.

**Have the tracker cross-reference: flag an open caveat whose entry is cited by
a later one.** Also already measured — 26 candidates, 4 read, 0 stale. The
citation graph is the wrong graph. Here the closing entry does not cite the
entry holding the caveat, and could not sensibly be made to.

**Search the guard names instead of the caveat text.** Tempting after this
find: if a caveat says "nothing checks X", grep for a checker of X. It is what
I did by hand here and it worked, and it does not generalise — it needs a guess
at what the guard would be called, which is a reading of the caveat, which is
the expensive step. It is the reading pass with extra steps, not a shortcut.

## Evidence

| caveat | checked against | verdict |
|---|---|---|
| `date_trunc` stops at the year | `CalendarUnit` at `crates/slate-kernel/src/scalar.rs:257` has exactly `Month` and `Year` | still true |
| the truncation is UTC | both variants' doc comments say "in UTC"; no zone argument on `DateTrunc` | still true |
| nothing checks the wasm bundle's freshness | `stale_sources()`, `site/check/workbench.py:521`, called from `main()` before the browser starts | **closed** |
| nothing in the browser writes a schema-state key | `crates/slate-wasm/src/lib.rs:1692` says so in a comment: space `0x03` is labelled and never written | still true |
| a backfill is not concurrent-safe against writers | `crates/slate-kernel/tests/migrations.rs` has no concurrent case; `migrate.rs:1436` checks uniqueness inside the backfill only | still true |
| the reverse zone direction is missing | one `in_zone` (`scalar.rs:751`), UTC → local; no local → UTC anywhere | still true |
| no zone on a literal in the SQL `WHERE` | `crates/slate-sql/src/sql.rs` takes a zone on time functions only | still true |
| no `DISTINCT ON` | no occurrence in any crate | still true |
| the demo's HTTP contract does not expose joined-space naming | the twenty `/api/…` routes build their joins server-side | still true |
| nothing measures a large backfill | no timing assertion in `migrations.rs` | still true |

**Rate.** One in ten, against a prior of one in eight from an earlier unbiased
sample of eighteen. Two batches today: one closure in batch two, one here. That
is consistent with the prior and is far too small a sample to refine it — four
closures over 28 reads is the whole of the evidence, and the interval around
1-in-8 at that count is wide enough to contain 1-in-4 and 1-in-20 alike. No
number is being revised.

**Not a measurement.** No code changed, so nothing here can have a mutation
run. `scripts/caveats.py` reports **830 caveats: 288 open, 182 closed, 309
deliberate, 0 untriaged**, against 822/285/180/308/0 at the start of the day.

## What this does not do

**The demo HTTP check was a route listing, not a contract audit.** I read the
twenty `/api/…` route names and confirmed the joins are built server-side. I
did not read each handler's request shape to prove none of them accepts a
joined-space ordinal. If one does, that caveat is wrong and I did not find out.

**The closure was found by hand and nothing will find the next one.** Three
alternatives to reading are rejected above and all three have now been measured
or reasoned to a dead end. What actually worked here was recognising a phrase —
"the same way" — as pointing at a guard that might exist, and going to look.
That is not a procedure.

**Two batches is 20 of 247.** At one in ten, the remaining unstamped list holds
perhaps 25 more closures and costs about 227 reads to find them.
