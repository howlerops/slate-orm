# A folder view of the whole database, and the real bucket beside it

- **Date:** 2026-09-14
- **Author:** Claude (agent session), at the request of the repository owner
- **Touches:** `crates/slate-wasm/src/lib.rs`, `crates/slate-wasm/tests/keyspace.rs` (new),
  `crates/slate-slatedb/examples/bucket_layout.rs` (new), `crates/slate-slatedb/Cargo.toml`,
  `site/{index.html,workbench.js,style.css,README.md}`, `site/data/bucket.json` (new),
  `site/check/workbench.py`
- **Kind:** feature

## What changed

A **Storage** view — a top level of the application, switched from the header
beside Query, not a tab inside the results pane — showing two things.

The first is the live keyspace as a folder tree: `rows/` and `index/`, a
folder per table or index under each, and opening one pages through the real
keys twenty-five at a time — in the order the store holds them, with what each
decodes to and how many bytes its value takes. It reads the store's own
committed map through a cloned `MemoryStore` handle, so it moves when the
reader inserts a row.

The second is a real bucket listing — the SSTs, WAL, manifests and compaction
records SlateDB writes — captured by a new example that seeds the site's own
100,000-trip sample into a real SlateDB store over a real object store and
walks the result.

## Why

The owner asked to see how the dataset looks in the S3/MinIO layout. Behind
that is the thing this project is hardest to explain in prose: **there is no
index.** There is one ordered key space, rows live at one prefix, index entries
at another, and "index maintenance is atomic with the write" means two keys go
in together. A reader can be told that. Seeing `rows/trips` at 100,000 keys and
`index/trips.by_pickup_zone` at 100,000 keys *carrying no values at all* is a
different kind of knowing — the empty value column is the covering-index story
stated as a fact about bytes.

The bucket half exists because the keyspace is not what an operator sees. They
open a bucket and find eleven files. Showing only the keyspace would leave the
impression that these keys *are* the objects.

## Alternatives rejected

**Describe the bucket layout in prose.** Cheapest, and it rots. A described
bucket is exactly the sort of documentation that stays confidently wrong
through three schema changes. Capturing a real listing costs one example and
makes the claim checkable.

**Fabricate a plausible listing.** Never seriously. The page's whole argument is
that what it shows is real; a hand-written `.sst` filename would poison that.

**Run SlateDB in the browser.** It cannot: no object store, no filesystem, and
the wasm build would grow by a large multiple to carry a compaction engine
nothing would use. The browser store is a `BTreeMap` and the page says so.

**Use MinIO rather than `LocalFileSystem` for the capture.** SlateDB writes
objects through `object_store` either way, and the paths and sizes are the
same; MinIO adds a container to the reproduction steps for nothing. The example
says this in its own docs so a reader does not wonder.

**Snapshot the keys at seed time and render that.** Cheaper than holding a
store handle, and wrong the moment a reader inserts a row. A keyspace viewer
that does not move when the data moves is worse than no viewer — it teaches
something false about the one property it exists to demonstrate.

**Render every key at once.** 210,319 keys is not a view, it is a hex dump
that locks the tab. Twenty-five at a time with a pager, fetched when a folder
is opened rather than eagerly — each fetch walks the whole store, and opening
six folders eagerly would walk it six times before the reader asked for
anything.

**Leave it as a tab in the results pane.** That is where it was first built and
where the owner did not find it: "I want a top level view of the ENTIRE DB".
A storage browser sharing a pane with query results is a storage browser
nobody opens, and the pane is a third of the window. As a mode it gets the
whole width, which a tree of keys needs.

**Re-implement the key layout in JavaScript.** The hex split and the decode go
through the kernel's own `decode_row_key` and `decode_index_entry`. A second
reading of the format in JS would drift from the first, and the panel would
start describing a layout nothing writes.

## Evidence

The viewer, on the live page (headless Chromium, the built bytes):

```
210,319 keys · 8.5 MB in 6 prefixes
rows/trips                  100,000 keys  814.7 KB keys + 6.5 MB values
index/trips.by_pickup_zone  100,000 keys  1014.1 KB keys + 0 B values
  01 00000003 | 1601   trips row id=1
```

The bucket listing, captured by `examples/bucket_layout.rs` against a real
SlateDB store: **11 objects, 21.7 MB** — one 11.0 MB compacted SST, a 10.7 MB
WAL, five manifests, three compaction records.

Six Rust tests in `crates/slate-wasm/tests/keyspace.rs` and four new browser
assertions, all passing. Mutations, each failing a *named* test:

| mutation | check that failed |
| --- | --- |
| call every key a row key | five of six, including `a_row_and_its_index_entry_are_two_keys_in_one_space` |
| report a fake key size | `the_byte_columns_are_real_sizes` |
| print a 3-byte id instead of 4 | `the_viewer_reads_the_layout_the_kernel_writes` |

The layout assertion is worth its line: it pins `01 <table id : u32 BE> | …`
against the module docs of `slate_kernel::keys`. The viewer is a second reader
of that format, and the day the format changes this test says the panel is
lying rather than letting it quietly show the wrong prefix.

`a_write_shows_up_in_the_keyspace` inserts a row and requires **both** counts
to move, then deletes it and requires both to move back. That is the atomicity
claim as an assertion rather than a sentence.

## What this does not do

**The bucket listing is static and nothing checks it is current.** It is a
snapshot of one load of one sample. Change the schema or the row count and it
silently describes the old thing. Regenerating it is a manual step in
`site/README.md`; a check that it matches would mean running SlateDB in CI for
a picture, which is not worth it yet.

**The WAL in that listing still holds 10.7 MB.** The example closes the
database, which flushes, but does not wait for the WAL to be reclaimed after
compaction — so the 21.7 MB total roughly double-counts the data. The listing
is true and the total is not the steady-state size on disk.

> **Closed on 2026-09-15** by `the-wal-the-collector-would-not-take`, and the
> diagnosis above turned out to be wrong: waiting does nothing, because SlateDB
> retains the boundary segment deliberately. The listing now reads 11.0 MB, and
> a browser assertion covers the arithmetic.

**It is one bucket at one moment.** No compaction over time, no second writer,
no fencing, none of the things `docs/topology.md` is about.

**Paging is forward-only and starts from the top.** No search, no jump to a
key, no way to look up a particular row's entry — and paging deep into 100,000
keys means walking the store once per page, which is fine at twenty-five and
would not be at two thousand.

**Values are counted, never shown.** A row's encoded bytes are not rendered, so
the tuple codec — the thing that makes a prefix of the key a prefix of the
tuple — is visible in the *keys* only.

**Building the view walks every key**, which is 731 ms for 210,000 of them in
the browser. It runs when the tab is opened, so a reader who never opens it
pays nothing, and one who opens it twice pays twice.
