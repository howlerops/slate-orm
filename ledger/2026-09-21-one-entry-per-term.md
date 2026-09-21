# An inverted index, and the cost model's verdict on it

- **Date:** 2026-09-21
- **Author:** Claude Code, on F6
- **Touches:** `crates/slate-schema` (`text.rs`, `table.rs`, `error.rs`), `crates/slate-kernel` (`record.rs`, `expr.rs`, `plan.rs`, `stats.rs`), `docs/full-text.md`, `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

Full-text search in the kernel. `IndexBuilder::text()` declares an inverted
index — one entry per *term* of one string column rather than one per row —
and `Expr::contains(column, text)` searches it. The planner ranges it on one
term and leaves the rest to the residual.

The structural change is `IndexDef::key_sets(row) -> Vec<Vec<Value>>`: one key
list for an ordinary index, one per term for a text one, and the only place
that difference exists. `entry_for` became `entries_for`, and the write path
compares two *sets* of keys where it compared two keys.

## Why

The gap row for this was unusually specific, and it was right: not "there is no
tokenizer" but **every index here wrote one entry per row**. The write path,
the unique-slot check, `erase_row` and the planner all assumed that cardinality.
So the work was not a new index expression, it was a new index shape, and the
interesting question was how much of the layer had to learn about it.

Answer: less than expected, because the *key type* did not change. A term is a
`Str` and the column is a `Str`, so `index_key_types`, `decode_index_entry` and
the index cursor are untouched. What changed is how many entries a row writes,
and that is one function.

## Alternatives rejected

**A second write path for text indexes.** Rejected for the reason this
repository already has on record — the partial-index write-path gap: a second
path is a second place to forget, and the half that gets forgotten is always
the one that *removes* something. With `key_sets` there is one loop and the
cardinality is a property of the index.

**Comparing "did the key change" rather than set membership.** The old write
path skipped an index whose key was unchanged. The set version costs a linear
scan of two short lists per index; the alternative — deleting every old entry
and writing every new one — is correct and would make every writer of a
paragraph conflict with every other writer of it, because editing one word
would rewrite all hundred entries.

**A third key shape on `IndexDef`, beside `columns` and `expression`.**
Rejected: a third shape means a third arm in every `match` that reads a key,
for no difference in what a key *holds*. A `text: bool` beside the single
column says exactly what is different.

**Stemming, stop words, Unicode folding.** Each refused with its own reason in
`docs/full-text.md` §3. The short version: a stemmer is a language model and
nothing here knows the column's language; stop words make `to be or not to be`
unsearchable to save a rounding error; folding `café` to `cafe` is a decision
about which marks are decoration, and in Swedish `å` is not an `a` with a hat
on. All three are the kind of cleverness that does not fail, it changes which
rows match.

**Letting the caller pass terms to `Expr::Contains`.** It takes the search as
written and tokenizes it, which is the whole safety property: one tokenizer,
shared with the write path. A caller splitting their own search finds fewer
rows than the table holds, silently, and the only way to notice is a comparison
against a scan.

**Costing the text path specially.** Under one term an inverted index's entries
are in primary-key order, so the row reads are ascending rather than scattered —
which is a plausible reason they are cheaper than `POINT_READ_COST` says, and
is unmeasured. Not told to the cost model. See **Evidence**.

## Evidence

**12 tests in `crates/slate-kernel/tests/fulltext.rs`**, and 686 kernel tests
green. The oracle runs the same `contains` twice, once hinted onto the index
and once onto a scan, and requires the same rows over generated text — with a
term that is a prefix of another, a repeat, a title that is only punctuation, a
Cyrillic term and a digit term.

**A finding about the oracle itself, before the mutations ran.** The first
version compared *a scan against a scan*: the planner's own choice for a
`contains` is a table scan (see below), so neither side of the comparison went
near the index. It would have passed against an index that wrote no entries at
all. Every read in the file is now hinted onto the index, and the oracle
asserts the two plans really are the two different things before comparing
rows. `a_search_ordered_by_the_primary_key_does_not_sort` had the same hole for
a different reason — a table scan is in primary-key order too.

**The measurement that changed a decision.** `TERM_SELECTIVITY` started at 0.05.
With it the planner chose a table scan at every size:

| rows | scan cost | index cost, hinted |
| --- | --- | --- |
| 1,000 | 1.125 | 4.000 |
| 10,000 | 2.250 | 31.001 |
| 100,000 | 13.500 | 301.012 |
| 1,000,000 | 126.000 | 3001.125 |

Lowering it to 0.001 — anchored on `ColumnStats::default`'s hundred distinct
values, one order the other way, because a text column's vocabulary is larger
than a categorical column's value set — did not change the verdict. **Which is
when the interesting part turned up: it is not about text.** The same table
with an ordinary index and an equality, at the same estimated selectivity:

| rows | distinct values | chosen |
| --- | --- | --- |
| 100,000 | 100 | scan |
| 100,000 | 10,000 | scan |
| 100,000 | 1,000,000 | **index** |
| 1,000,000 | 10,000 | scan |
| 1,000,000 | 1,000,000 | **index** |

A non-covering index is taken when it returns about **one row in 24,000** —
`SCAN_ROW_COST / POINT_READ_COST`, 0.000125 against 3.0, both measured on
object storage rather than assumed. That is the model, and a text index is
subject to it like everything else.

So `the_planner_costs_a_text_index_like_any_other` asserts the *equivalence*
rather than a choice: a `contains` and an equality of the same estimated
selectivity reach the same verdict at every size. A future change that costs
the text path specially — which a measurement might justify — has to break that
test deliberately.

**Fourteen mutations, thirteen caught by a named test, and one survivor that
was a missing test.**

| where | mutation | caught by |
| --- | --- | --- |
| `key_sets` | holds the whole string, not its terms | 4 tests |
| `key_sets` | holds only the row's first term | 4 tests |
| `tokenize` | terms keep their case | 4 tests |
| `tokenize` | only ASCII is part of a term | the two unit cases, including the Cyrillic one |
| `tokenize` | terms are not deduplicated | its own unit case |
| write path | an update leaves the entries of terms the row lost | 4 tests, two of them pre-existing partial-index ones |
| write path | a delete removes only the first entry | `a_delete_removes_every_one_of_a_rows_entries` |
| write path | a write puts only the first new entry | 4 tests |
| `Contains` | matches a row holding *any* term | the two-term case, and the oracle |
| `Contains` | an empty search matches everything | `a_search_with_no_terms_matches_nothing` |
| `contains` | splits the search on whitespace instead of tokenizing | the punctuation case |

**The survivor, and why it is interesting.** Making a null text write one entry
under the *empty term* broke nothing, and could not have: a search for `''` has
no terms and matches nothing, so no query can tell the two apart. It is still
wrong — every row with a null title would share one key, a hot spot holding
rows no term can reach — and the only way to see it is to look at the index
rather than at an answer. `a_row_with_no_text_writes_no_entry` asserts
`key_sets` directly, and catches that mutation and the neighbouring one that
writes an empty-term entry for a title of pure punctuation.

**Two refusals found while building, both pre-existing.** A text index declared
with an expression came back as `EmptyIndex`, whose message says "has no
columns" — it has columns *and* an expression, which is the other half of the
same XNOR, and the message has been wrong for that case since the variant was
written. The message and its doc now say what it actually catches.

## What this does not do

**Nothing outside the kernel knows about it.** Not the wire, not the three
clients, not `slate-serverd`'s TOML schema, not the SQL front end. That is the
same boundary arrays stopped at and is the obvious next piece.

**No ranking, no phrase search, no prefix or fuzzy matching.** BM25 needs term
frequencies and document lengths; a phrase needs positions; a prefix needs the
term keyspace walked as a range, which the *encoding* supports and the
predicate does not express. Each is named in `docs/full-text.md` rather than
half-built.

**No per-term statistic**, which is the thing that would let the planner know
`earthsea` is rare and `the` is not — and therefore the thing that would make
the index chosen rather than hinted. Sampling the rare tail of a Zipf
distribution is the hard part and is not attempted.

**Japanese and Chinese tokenize into one enormous term**, because the split is
on `char::is_alphanumeric` and those scripts do not separate words. A real
limit, stated rather than discovered.

**The memory store cannot measure any of this.** Every number above is the cost
*model*'s, not a stopwatch; what the model is calibrated against is in
`docs/performance.md`. Whether an inverted index is actually faster than a scan
on real object storage is unmeasured here.
