# Full-text search: a new index *cardinality*, and what it costs to reach

`orm-comparison.md` has listed full-text search as missing with an unusually
specific piece of evidence: not "there is no tokenizer" but **"every index here
writes one entry per row"**. `entry_for` returned a single `IndexEntry` from
`index.key_values(row)`, and the write path, the unique-slot check, the delete
path and the planner all assumed that cardinality. An inverted index is one
entry per *term* per row, so this is not a new index expression — it is a new
index shape.

This note records the six decisions that shape took, and one measurement that
changed the answer to the last of them.

> **Built.** In the kernel: `IndexBuilder::text()`, `IndexDef::key_sets`,
> `Expr::Contains`, `slate_schema::tokenize`, and a planner branch that ranges
> an inverted index on one term.
> `crates/slate-kernel/tests/fulltext.rs` is the demonstration and its oracle.
> On the wire and outwards: `Expr.contains` in `records.proto`,
> `ColumnRef.contains` / `slate.Contains` / `contains` in the Python, Go and
> TypeScript clients, and `text = true` on an index in `slate-serverd`'s TOML
> schema. Each client has a live test against a real node
> (`clients/*/…/fulltext*`), and each of those hints onto the index rather than
> searching unhinted, for the reason §6 measures.
> In the SQL front end: `WHERE title CONTAINS 'the heaven'`, infix, with a
> text index on the workbench's `books`.
> **No search box in the demo's web UI, and that is not a gap.** The UI calls
> 11 of the adapters' 24 endpoints; windows, chains, nearest-neighbour,
> relationships, paging and purge are all conformance-only too. Its job is the
> SDK switcher and the identity switcher — same query three ways, and what the
> *database* grants — while the conformance runner is what covers the surface.
> A search box without a window panel beside it would make the UI less
> coherent, not more.

## 1. One place decides how many entries a row writes

`IndexDef::key_sets(row) -> Vec<Vec<Value>>`: one list for an ordinary index,
one per term for a text one. Everything downstream iterates it.

The alternative was a second write path for text indexes. Rejected for the
reason the partial-index gap (`ledger/`, "Close the partial-index write-path
gap") is on record for: a second path is a second place to forget, and the
first thing forgotten is always the half of an update that *removes*
something. With `key_sets` there is one loop, and the cardinality is a
property of the index rather than of the code path.

The write path changed from comparing two keys to comparing two *sets* of
keys, and that is the whole of it:

```rust
for old in &old_entries {
    if !unchanged(old, &new_entries) { delete(old.key) }
}
for new in new_entries {
    if unchanged(&new, &old_entries) { continue }
    put(new.key, new.value)
}
```

Set membership rather than "did the key change", because editing one word of a
paragraph rewrites two of its hundred entries. Rewriting the other ninety-eight
would make every writer of a long text conflict with every other writer of it,
which is a correctness-preserving change that destroys concurrency.

## 2. The key is still the column's type, so nothing that reads an entry changed

A term is a `Str` and the indexed column is a `Str`. So `index_key_types`,
`decode_index_entry` and the index cursor are untouched: an entry is
`[header][tenant?][term][primary key]`, exactly the shape an ordinary
single-column string index has. Only the *number* of them differs.

This is why `text` is a flag on `IndexDef` rather than a third key shape beside
`columns` and `expression`. A third shape would have meant a third arm in every
`match` that reads a key, for no difference in what a key holds.

## 3. One tokenizer, and it is deliberately stupid

`slate_schema::tokenize`: lowercase, split on anything that is not
`char::is_alphanumeric`, sort, deduplicate. It is called in exactly two places
— the write path, through `key_sets`, and `Expr::contains`, which tokenizes the
caller's search text rather than taking terms.

**That second call is the safety property.** A caller who split their own
search would find fewer rows than the table contains, silently; the only way to
notice is to compare against a scan, which is what the oracle in
`fulltext.rs` does.

What it deliberately does not do, and why:

- **No stemming.** A stemmer is a language model. `universities → universe` is
  Porter's own documented over-stem, and getting it wrong does not fail, it
  changes which rows match. It is also per-language and nothing in this layer
  knows what language a column holds.
- **No stop words.** Dropping `to`, `be`, `or`, `not` makes `to be or not to
  be` unsearchable. The rows saved are a rounding error against one entry per
  distinct term per row, and the cost of a common term is paid by whoever
  searches for it — the right person to pay it.
- **No Unicode normalisation beyond lowercasing.** `café` and `cafe` are
  different terms. Folding them means NFD plus a decision about which combining
  marks are decoration, and in Swedish `å` is not an `a` with a hat on.
- **No word segmentation.** `is_alphanumeric` means a Cyrillic or Greek term is
  a term rather than a gap between separators, and it also means Japanese and
  Chinese tokenize into one enormous term. That is a real limit, not an
  oversight: segmenting them needs a dictionary.

## 4. `contains` is conjunctive, and an empty search matches nothing

`Expr::Contains { column, terms }` is true when the column's text holds *every*
term. That is what a search box means by two words. A disjunction is
`Expr::any` of two of them; **phrase search is not offered at all**, because an
inverted index without positions cannot answer "adjacent, in order" and adding
positions is a different structure.

A search whose text has no terms — `contains(title, '???')` — matches nothing.
The alternative, matching everything, hands back the whole table for a query
the caller thought was narrow. It is the same choice `Expr::Matches` makes for
a pattern that does not compile.

Null is Unknown, not false: a null title neither contains a term nor fails to,
so it is in neither the positive answer nor the negated one. SQL's own rule and
the one `LIKE` already follows here.

## 5. Four declarations are refused

A text index must be one string column, and not unique, and not an expression.
All four refusals carry one error with a reason, because they are one mistake:

- **Unique.** A term appears in many rows by construction, so a unique inverted
  index is a constraint no realistic text satisfies. Accepting it would turn
  the second row containing "the" into a write failure nobody could read.
- **More than one column.** A cross product of two columns' terms is a
  different and much larger structure.
- **A non-string column.** There is nothing to tokenize.
- **An expression.** A term is not a computed value.

`only_where` composes and is free: a partial text index holds terms for the
rows its predicate admits, through the same `admits` gate every other index
uses.

## 6. The planner: costed like every other index, and that is the finding

An inverted index is ranged on **one** term of a `contains` — a prefix of the
keyspace — with the remaining terms left to the residual, which re-checks all
of them on every row the scan admits. Which term is taken is the first, which
after `tokenize` is the lexicographically smallest. Choosing the *rarest* would
be better and needs a per-term document count, which is not collected; the
alternative on offer is a heuristic dressed as a decision (the longest word,
say, which is wrong for a search containing one long common word), so the
choice is deterministic rather than clever.

Two things are refused rather than guessed:

- **No `covering` scan, ever.** An entry holds a term, so a row cannot be
  rebuilt from one — not even the column the index is on, whose value is the
  whole text rather than the word the entry stands for.
- **No candidacy on a tenant-scoped table without an equality on the tenant.**
  The tenant leads every index key, so without it the term is not at a fixed
  offset and there is no contiguous span to read. The row policy supplies that
  equality on every tenant-scoped read, so the shape being declined is exactly
  the one a caller reaching past their tenant would need.

### The measurement, and the conclusion it forced

`TERM_SELECTIVITY` — how much of a table one term is expected to keep — started
at **0.05**, on the argument that a term is narrower than an unanchored `LIKE`.
With it, the planner chose a table scan at every table size tried:

| rows | scan cost | index cost (hinted) | chosen |
| --- | --- | --- | --- |
| 1,000 | 1.125 | 4.000 | scan |
| 10,000 | 2.250 | 31.001 | scan |
| 100,000 | 13.500 | 301.012 | scan |
| 1,000,000 | 126.000 | 3001.125 | scan |

Lowering it to a thousandth did not change the verdict at any of those sizes,
which is when the interesting thing turned up. **It is not about text.** The
same table, with an ordinary index and an equality, at the same estimated
selectivity:

| rows | distinct values | chosen |
| --- | --- | --- |
| 100,000 | 100 | scan |
| 100,000 | 10,000 | scan |
| 100,000 | 1,000,000 | **index** |
| 1,000,000 | 10,000 | scan |
| 1,000,000 | 1,000,000 | **index** |

A non-covering index is chosen on this cost model when it is expected to return
about **one row in 24,000** — which is `SCAN_ROW_COST / POINT_READ_COST`,
0.000125 against 3.0, both measured on object storage rather than assumed.
Following an entry to its row is three requests; a scanned row is an
eight-thousandth of one.

So the honest state of the feature is:

- A `contains` with no text index is a table scan with the predicate as a
  residual, and is correct.
- A `contains` *with* one is costed the ordinary way, and on a corpus where
  the average term is not vanishingly rare the planner prefers the scan. A
  caller who knows the term is rare says `using_index`, which is a first-class
  hint the kernel and the wire already carry — and which, since the clients
  went in, all three of them can send: Python's `Query.using_index`, Go's
  `Query.Hint` with `slate.UsingIndex`, TypeScript's `query.hint` with
  `usingIndex`. The Go and TypeScript halves were added by this work, because
  without them a full-text test in either language is a table scan wearing the
  word `contains` and would pass with the index deleted.
- `the_planner_costs_a_text_index_like_any_other` asserts exactly that
  equivalence, so a future change that costs the text path specially breaks a
  test rather than passing quietly.

Two things would change the verdict, and both are measurements rather than
opinions:

1. **A per-term posting count**, collected by `analyze`, would let the planner
   know that `earthsea` is rare and `the` is not. The structure can hold
   millions of distinct keys, so this is a sampled statistic rather than an
   exact one, and sampling a Zipf distribution for the *rare* tail is the hard
   part.
### The keyword is worth having even where the index is not chosen

An earlier draft of this note argued the opposite — that a `CONTAINS` the
planner answers with a table scan would be "a scan with a nicer spelling", and
that the SQL front end should therefore wait. That was wrong, and the test
that withdrew it is `a_contains_is_not_a_like`:

- `LIKE '%Cosmic%'` finds *Cosmicomics*; `CONTAINS 'Cosmic'` does not, because
  a term is a whole word.
- `CONTAINS 'heaven the'` finds *The Lathe of Heaven*; `LIKE '%Heaven%the%'`
  does not, because a pattern is ordered and terms are not.

Neither predicate can be written as the other, so the keyword is a new thing to
say rather than a faster way to say an old one. What the access path costs is a
separate question from what the predicate means, and conflating them is what
the withdrawn argument did.

2. ~~**Whether `POINT_READ_COST` is right for this walk.** It was calibrated
   on 400 rows reached through an ordinary index, whose entries are in
   *column* order, so the row keys are scattered. Under one term of an
   inverted index the primary keys are ascending, and ascending reads may
   coalesce into far fewer block fetches.~~ **Measured, and the premise was
   wrong.** An index entry is `0x02 <index id> <tenant?> <indexed tuple>
   <primary key tuple>`, so under *one* indexed value an ordinary index's
   primary keys already ascend — an equality and a term produce the same shape
   of walk, and there was no difference for a second constant to capture.

   `crates/slate-slatedb/examples/ascending_walk.rs` holds the two apart. Four
   arms return the same 400 rows over 200,000, each from a freshly reopened
   store so no arm reads what the one before it cached:

   | arm | GETs per row |
   | --- | --- |
   | spread (every 500th row), ordinary index | 1.015, 1.015, 1.015 |
   | spread, inverted index | 1.015, 1.015, 1.015 |
   | dense (one contiguous run), ordinary index | 0.048, 0.048, 0.043 |
   | dense, inverted index | 0.048, 0.043, 0.048 |

   Three runs. **The index's kind changes nothing** — the spread arms are
   identical to the request, and the dense arms differ by less than they
   differ between runs of themselves. What moves the number is **density**,
   how many table rows separate consecutive matches, by a factor of 21. That
   is a property of the predicate, not of the index, and the model does not
   know it for *either* kind: `POINT_READ_COST` is a constant where the
   measurement is a range from 1.015 to 0.043.

   So the text path needs no special constant, and the case for a
   density-aware read cost — which would change ordinary index plans far more
   often than text ones — is now a measurement rather than an intuition.

## What this does not do

No ranking. BM25 needs term frequencies and document lengths, which is a second
structure over the same entries; the rows come back in primary-key order, which
is at least an order rather than an accident.

No phrase search, no prefix search (`contains(t, 'wiz*')`), no fuzzy matching.
The first needs positions, the second needs the term keyspace walked as a range
rather than a point — which the encoding *would* support and the predicate does
not express — and the third needs an edit-distance index.

No `contains` on a join or a chain. It is an ordinary predicate over one
table's ordinals and nothing stops it being evaluated over a joined row as a
residual; what it cannot do there is reach an index, for the reason every
join-side index access is a separate question.
