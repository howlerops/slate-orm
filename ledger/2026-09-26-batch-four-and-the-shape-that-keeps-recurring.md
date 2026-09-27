# Batch four: ten more open caveats read, two closed. Both were "this waits for aliases" or "this waits for a UI", and both shipped the same week in an entry that never mentions them. Six of six stale caveats found so far have that shape.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `docs/caveat-status.json`
- **Kind:** process — re-triage, no code

## What changed

Ten more `open` caveats checked against the tree. Eight still true and stamped;
two closed:

1. **"The `decade` grouping changes what the demo's chart can draw"** — the
   Groups panel's group-by select now reads author / country / **decade**
   (`examples/explorer/web/src/panels.tsx:331`).
2. **"Nothing outside the wasm crate is touched"** — both halves are gone.
   `site/check/workbench.py` runs an aliased three-table chain over the
   pickup/dropoff borough query — the exact query the caveat named as the one
   the dataset is *for* and could not express — and the two-dropdown join panel
   no longer exists at all: `site/` holds `index.html`, `workbench.html` and
   their scripts, nothing else.

## Why

The reading pass, continued. What makes this batch worth its own entry is the
count on the shape.

Six caveats have now been found stale in this repository by reading:

| caveat | closed by | gap |
|---|---|---|
| two from the first unbiased sample of eighteen | "waits for X", X had shipped | — |
| the wasm bundle's freshness (batch three) | `2026-09-15-one-table-twice.md` | **same day** |
| `ORDER BY` a chain's computed value (batch two) | a test I wrote this morning | — |
| the `decade` grouping's UI | the panel gained the option | within the week |
| "nothing outside the wasm crate" | `2026-09-15-one-table-twice.md`, again | **same week** |

**Five of six are the same failure**, and it is not forgetfulness about an old
claim. It is two entries written *close together* — hours, in two cases — one
recording a gap and the other filling it, with no link either way. The closing
entry has no reason to mention a caveat in a file it never opened, and the
recording entry is append-only and correct as of its date.

`2026-09-15-one-table-twice.md` alone has now closed **two** caveats in other
entries and cited neither. It is the single most productive closer found so
far, which is not a property of that entry: it is what "aliases" unblocked.
A feature that several entries are waiting on closes several caveats at once
and says so nowhere.

## Alternatives rejected

**Make the tracker warn when an entry's caveat mentions a feature name that a
later entry's title contains.** This is the third variation on "find it by
text", and the first two were measured and failed
(`ledger/2026-09-26-two-ways-to-find-a-stale-caveat-that-do-not-work.md`). It
would also have missed both of today's: neither caveat says "alias", they say
"the panel's join form" and "the demo's chart".

**Ask the closing entry to check.** The obvious fix — when you ship a feature,
grep the open caveats for it — and it fails for the reason above: the caveats
are phrased in terms of the *consequence*, not the feature. Somebody shipping
aliases would grep for "alias" and find neither of these two.

**Batch the reading by closer instead of by date.** Now that
`one-table-twice.md` is known to be a prolific closer, its neighbours are a
richer seam than the unread list's head. It probably is — and picking the
richest seam first makes the remaining rate estimate worthless, and the rate is
the only number that says how much of the list is dead. Read in order, keep the
estimate honest. This is recorded as a decision rather than an oversight.

## Evidence

| caveat | checked against | verdict |
|---|---|---|
| no computed value on an outer join's unmatched side | no hit for "unmatched" near "computed" in any client suite; the join fixtures are inner | still true |
| 11.0 MB is still ~100 bytes a row | `site/data/bucket.json` still lists a **11,538,479**-byte object | still true |
| tenant affinity is not exercised deployed | `examples/deployed/head.toml` `[routing]` holds `catch_up` only | still true |
| the demo's two new columns are unexplained in the UI | `rating` and `embedding` are in `CATALOG_TABLES["books"]`; nothing in `panels.tsx` describes either | still true |
| the chain path has no proptest round trip | `crates/slate-wasm/tests/sql.rs` has no `ChainSpec` strategy | still true |
| no measurement of an enum name's footprint | `docs/performance.md` mentions enums twice, neither about a name-vs-ordinal row cost | still true |
| 403 is still 4.7× the data, the remainder is `Bytes` inline | `crates/slate-kernel/src/memory.rs:36-38` is still `Arc<BTreeMap<Bytes, Bytes>>`; the representation was never changed | still true |
| nothing is attributed between 369 and 491 | `docs/performance.md:1780-1791` breaks out 219→369 as capacity slack and measures the conflict history at ~0 bytes; the 369→491 store overhead is still not broken down | still true, narrowed |
| the `decade` grouping has no UI | `panels.tsx:331` | **closed** |
| nothing outside the wasm crate is touched | `site/check/workbench.py:1139`; no dropdown panel in `site/` | **closed** |

**Two of these were checked as design statements, not re-measured**, and that
distinction matters: "403 is still 4.7× the data" and "11.0 MB is still 100
bytes a row" are numbers, and re-running them needs the scale harness at
`--release`, which does not fit on this container. What was checked is the
thing the number depends on — the map's representation, and the committed
listing's byte count. If either number has drifted without its input changing,
this pass would not know.

One narrowing rather than a closure: the 369-to-491 caveat named four
components as unattributed, and one of them — the commit's conflict history —
has since been measured at two allocations a row and no measurable bytes. The
caveat is still true of the other three and is smaller than it was. Left open
rather than rewritten, because an entry is append-only and the tracker holds
the status.

**Counts.** `scripts/caveats.py`: **833 caveats: 287 open, 184 closed, 309
deliberate, 0 untriaged.** Four closures over 38 reads today, against the
1-in-8 prior from the earlier sample of eighteen. Still too few to revise it.

## What this does not do

**No code changed, so nothing here is mutation-tested.** The only executable
claim is the tracker's own counts, which `scripts/test_caveats.py` covers.

**"Six of six have the same shape" is a count over found-stale caveats, not over
stale ones.** Every one was found by reading, and reading is biased toward
caveats whose text points somewhere checkable. A stale caveat with no such
handle would not be in this sample and there is no way to know how many there
are.

**The seam this suggests was deliberately not mined.** `one-table-twice.md`'s
neighbours are the obvious next batch and the reading order was kept. That is a
choice to protect a statistic, and if the statistic turns out not to matter it
will have cost real closures.

**234 unstamped open caveats remain**, which is more than the 247 this
morning minus the 30 read today. The arithmetic does not work because each
entry written to record a closure adds two or three caveats of its own —
this one added four. The open list falls by reading and rises by writing
about reading, and today the two are close to cancelling: 285 open at the
start, 288 now, with four genuine closures in between.
