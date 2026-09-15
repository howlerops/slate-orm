# The storage view's 21.7 MB was double-counting, and the reason was not the one recorded

- **Date:** 2026-09-15
- **Author:** Claude Opus 5, at Jacob's direction
- **Touches:** `crates/slate-slatedb/examples/bucket_layout.rs`, `site/data/bucket.json`, `site/README.md`, `site/check/workbench.py`
- **Kind:** fix

## What changed

`bucket_layout` now writes one real row after the bulk load, runs SlateDB's
garbage collector, and lists what is left. The page's storage view reads
**11.0 MB across 12 objects** instead of 21.7 MB across 11, and a browser
assertion covers the arithmetic rather than only the shape of the listing.

## Why

The page showed a bucket holding 21.7 MB for 11.0 MB of rows, because the WAL
segment that carried the load was sitting beside the SST that now held the same
data. A reader taking the headline at face value would conclude a record layer
over SlateDB costs twice what its data weighs.

## Alternatives rejected

**Wait for the WAL to be reclaimed and then list.** This is what the ledger
recorded as the fix and it does not work — see below. It was tried first.

**Run the collector after closing and list.** Also tried, also does not work,
for the same reason. It does tidy five manifests down to one, which is why the
listing has fewer manifest objects than before, but the 10.7 MB stays.

**Subtract the WAL from the total and show the difference.** The smallest
change, and dishonest: the bytes really are in the bucket at that moment. What
is misleading is not the arithmetic but the moment chosen — a database that has
finished loading and will never be written to again.

**Present both totals side by side and explain.** Seriously considered, and it
is what the terminal output does. Rejected for the *page* because the storage
view is a folder tree with one headline number, and two numbers with a
paragraph of SlateDB internals attached is not what a reader came to that tab
for. The paragraph lives in `site/README.md`, where someone chasing the number
will find it.

**Advance the boundary with a synthetic key rather than a real row.**
`db.put(b"probe", …)` is two lines and needs no record store. Rejected: it puts
a key in the database that belongs to no table, and this listing is supposed to
be what a *record layer* leaves in a bucket. Rewriting the last trip row costs
four more lines and leaves the database holding exactly the rows it claims to.

## Evidence

The recorded diagnosis was that the example "does not wait for the WAL to be
reclaimed after compaction". That is wrong, and the measurement says so.

SlateDB's WAL collector keeps every segment from the manifest's
`replay_after_wal_id` **inclusive** onwards — the comment in
`garbage_collector/wal_gc.rs` calls it "the current compaction boundary", kept
to protect concurrent writers. Reading the manifest after the load:

```
replay_after_wal_id=2  next_wal_sst_id=3
records/wal/00000000000000000002.sst   10.7 MB
```

The boundary *is* the segment holding the trips, so it is retained by design.
Running the collector with `min_age: 0` collected four manifests and left the
WAL exactly where it was: **21.7 MB, unchanged**. Waiting longer would do
nothing, because age was never what protected it.

Reopening the database does not move the boundary either — measured, because it
seemed likely: it writes a fence at the next id and leaves the boundary alone
(`replay_after_wal_id=2, next_wal_sst_id=4`), and the listing came back at
21.7 MB with one more zero-byte object in it.

Only a write *past* the boundary releases the segment. With one row rewritten
after the load, `replay_after_wal_id` advances to 4 and the collector takes it:

| | objects | total |
|---|---:|---:|
| after the load, as the page shipped it | 11 | 21.7 MB |
| + collector, no write | 8 | 21.7 MB |
| + reopen and close, no write | 11 | 21.7 MB |
| + one row rewritten, then collector | 12 | **11.0 MB** |

What the extra write costs is in the listing rather than hidden: a 376-byte
compacted SST and a 268-byte live WAL segment.

**A browser assertion, mutation-tested.** The check that the listing has "an
SST, a WAL and a manifest" passed against both the wrong listing and the right
one — it was about which kinds of object appear, and the wrong one had all
three. The new check is arithmetic: no object listing may sum to more than 1.5×
its largest member. Fed the old `bucket.json`, it fails with
`largest object 11534336, total 22754549`; fed the new one it passes. 48 checks
in that file now.

## What this does not do

**It does not make the listing self-checking.** Regenerating `bucket.json` is
still a manual step, and nothing notices if the schema changes and the file
does not. The new assertion catches a *double-counted* listing, not a stale
one.

**`min_age: Duration::ZERO` is safe here and nowhere else.** Both handles are
closed and nothing else is reading, so there is no reader whose segment could
be pulled out from under it. The comment in the example says so; if that line
is ever copied into something that serves traffic it will cause the data loss
the option exists to prevent.

**The zero-byte WAL fences are still in the listing** — two of them, at ids 1
and 3. They are what SlateDB writes to claim a position, `wal_fence_options`
defaults to dry-run for good reasons documented at length in its own config,
and collecting them was not worth the risk for two zero-byte objects.

**11.0 MB is still 100 bytes a row for rows that serialise to ~110**, which
looks better than it is: it is one compacted SST of a freshly loaded database
with no overwrites, no tombstones and nothing to compact away. It is not a
steady state under churn, and nothing here measures that.
