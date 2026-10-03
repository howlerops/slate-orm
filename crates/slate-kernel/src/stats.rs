//! Table statistics, and the cost model they feed.
//!
//! Without these the planner picks whichever index matches the most equality
//! terms, which on the benchmark corpus chose a plan 30× slower than ignoring
//! the index entirely. Structure alone cannot tell you that: an index is worth
//! using only when it selects few enough rows to be worth a point read each,
//! and "few enough" is a fact about the data.
//!
//! # The cost model
//!
//! Everything is in **object-store requests**; see the note on the constants
//! for how they were measured and what they used to say.
//!
//! | | cost | why |
//! |---|---|---|
//! | open a scan | 1.0 | one request |
//! | one row from a scan | 0.000125 | measured: readahead returns ~8,000 rows per request |
//! | one point read | 1.0 | measured: one request per row sharing no block |
//! | `n` overlapped reads | `n` | concurrency hides latency; it does not do less work |
//! | `k` disjoint ranges of one index | `k` opens | each range is its own iterator, walked in turn |
//!
//! The ratio decides when an index is worth using, and since the recalibration
//! that is a statement about *absolute* numbers rather than percentages: a
//! non-covering index scan beats a table scan of `n` rows only while it fetches
//! fewer than `n / 8000` of them. At a million rows that is a hundred and
//! twenty-five rows, not six per cent of a million. It read `n / 24000` until
//! `POINT_READ_COST` was re-measured; same shape, a third less severe.
//!
//! Splitting an `IN` into a range per value pays the same arithmetic with the
//! opens added: `k` ranges start `k` requests in the red and win by fetching
//! fewer rows than one range over the whole span would. That is why the split
//! is worth having and why a hundred-value `IN` is not.
//!
//! What does not change is that covering an index matters far more here than
//! on local disk. Overlapping a round trip makes it cheaper; not making it at
//! all is still free.
//!
//! # Estimates
//!
//! Selectivities multiply, which assumes the columns are independent. They
//! frequently are not, and this is the standard place for an optimiser to be
//! wrong. It is a deliberate trade: correlated-column statistics are a large
//! subsystem, and the residual predicate means a bad estimate costs time rather
//! than correctness.

use crate::expr::{CmpOp, Expr};
use slate_schema::{IndexId, Ordinal, TableDef, TableId};
use slate_tuple::Value;
use std::collections::BTreeMap;

// The unit, and how it was calibrated.
//
// Costs are in *object-store requests*, because that is what a scan and a point
// read can both be counted in, what the provider bills, and what saturates
// first. The constants below were measured against an S3 server at 200,000
// rows by `slate-slatedb`'s `cost_calibration` example — not against a latency
// fixture, which is how the previous ones came to be wrong.

/// Cost of opening a scan, in object-store requests.
pub const SCAN_OPEN_COST: f64 = 1.0;

/// Cost of one row pulled from an open scan.
///
/// Measured: a scan with readahead on returns roughly **8,000 rows per
/// request** — 200,000 rows in 25 requests, 50,000 in 6. The previous value of
/// `0.01` assumed a hundred rows per request, overcharging every scan by about
/// eighty times.
///
/// It depends on row width, so it is an average rather than a constant of
/// nature: wider rows fit fewer per block. Eight thousand is what this corpus
/// gives with a realistic row and the default 1 MiB readahead.
///
/// **It also depends on the block cache, by 7×, and this number is the best
/// case.** The same scan of the same 200,000 rows costs a different amount
/// depending only on what the store read *before* it — a partially populated
/// cache fragments one scan into many small ranged reads rather than serving
/// it:
///
/// | the store had already | rows per request |
/// | --- | ---: |
/// | the whole table cached | 8,000 — this constant |
/// | read nothing at all | 3,774 |
/// | served 200 random point reads | 980 |
/// | served `analyze` and 400 point reads | 542 |
///
/// So a scan is charged between 1× and 15× less than it costs, depending on a
/// state nothing here represents. Nothing in [`TableStats`] carries it, and
/// inventing a statistic for it is a design question rather than a
/// calibration: what a *server* should assume about its own cache depends on
/// its workload, not on its data. The four figures are one fixture, one row
/// width, one scale.
///
/// `slate-slatedb`'s `cost_at_scale` example produces the last three; see
/// `ledger/2026-09-21-a-scan-costs-what-the-reads-before-it-left-behind.md`
/// for why the two examples that looked like they disagreed did not.
///
/// # What moving it costs, measured
///
/// Every candidate above flips plan decisions this crate has named tests for.
/// Each value was set and `cargo test -p slate-kernel --no-fail-fast` run
/// against 689 passing tests; the failures nest, so a dearer scan is strictly
/// more disruptive:
///
/// | rows per request | tests that fail | the new ones |
/// | ---: | ---: | --- |
/// | 8,000 — this constant | 0 | |
/// | 3,774 | 4 | `a_large_set_goes_back_to_a_scan`, `the_planner_picks_a_loop_only_for_a_small_outer_side`, `plans_match_the_committed_snapshot`, `the_fixture_and_the_cost_model_agree_on_rows_per_block` |
/// | 980 | 5 | `a_wide_in_loses_to_a_table_scan` |
/// | 542 | 6 | `the_planner_picks_a_loop_when_the_accumulated_side_is_small` |
///
/// Two of those are mechanical — the snapshot, and the latency fixture that
/// holds the same rows-per-block figure and has its own test saying to change
/// both together. The other four are access-path choices.
///
/// **The headroom is about 1.3×, bisected.**
/// `a_large_set_goes_back_to_a_scan` holds at `0.000_150` and has flipped by
/// `0.000_180`, against the `0.000_125` here. So the constant sits closer to
/// flipping a named decision than the spread of its own calibration — 7× —
/// and the mildest honest recalibration, the completely cold store, is 2.1×
/// away and past the edge.
///
/// That is the argument for leaving it rather than a reason it is right:
/// there is no single value, because the measurement depends on a cache state
/// the model has no input for. Under-charging a scan is how
/// `cost_at_scale` comes to pick one that is 11× slower at 200,000 rows.
/// Over-charging it turns a four-hundred-key `IN` into four hundred point
/// reads on a table that is entirely cached, which is the other direction and
/// no better. Both are wrong; only one is the status quo.
pub const SCAN_ROW_COST: f64 = 0.000_125;

/// Cost of one point read.
///
/// **One request per row reached through an index.**
///
/// ~~Measured at three requests per read, not one: following an index entry to
/// its row goes through more than a single object fetch. 400 rows reached by
/// index cost 1,217 requests.~~ ~~Re-measured later at 3.40–3.57 on a
/// pseudo-random walk, and left at 3.0 as within the spread.~~ **Neither
/// figure reproduces.** Four measurements, three benchmarks, two fixtures,
/// both cache states, all taken on one machine on one day:
///
/// | measurement | requests per row |
/// | --- | ---: |
/// | `cost_calibration`, forced index, 400 of 200,000, warm | 1.02 |
/// | `cost_calibration`, the same, cold | 1.09 |
/// | `cost_at_scale`, 200 probes on a pseudo-random walk, cold / warm | 1.16 / 0.96 |
/// | `ascending_walk`, ordinary and inverted index, stride 500 | 1.015 |
///
/// Nothing produces 3 **on the build that ships today**, and #278 found what
/// changed. It is none of the three things guessed at here — not a SlateDB
/// release, a block size, or the readahead `#34` turned on. It is
/// `slate-slatedb`'s `cache` feature, which `docs/performance.md`'s finding 8
/// turned on after this constant was calibrated: SlateDB's block and metadata
/// caches were compiled out, so every index entry and every row went to object
/// storage separately.
///
/// The same `cost_calibration`, same machine, same 200,000 rows, one session
/// apart:
///
/// | build | forced index, 400 rows | per row |
/// | --- | ---: | ---: |
/// | `--no-default-features --features aws` (as calibrated) | 1,221 GETs | **3.05** |
/// | default, with `cache` (as shipped) | 414 GETs | **1.04** |
///
/// **1,221 is the recorded figure to the unit** — the struck-through sentence
/// above says 1,217 and the comments in `join.rs` and `chain.rs` said 1,221.
/// So the old number was right about the old build and the new number is right
/// about this one; neither measurement was ever wrong, and the constant was
/// three times its value for exactly as long as the cache was missing.
///
/// The scan side did not move: 29 GETs for the full scan on both arms. A cost model wrong by 3× in the direction of "never use an
/// index" is the same class of error as the one this constant was introduced
/// to fix, pointing the other way.
///
/// **1.0 rather than 1.02, because it is a bound with a meaning**: one
/// object-store request for a row that shares its block with no neighbour. No
/// measurement here exceeds it. Rows that *do* share blocks cost far less —
/// `ascending_walk` reads 0.043 per row over a contiguous run — so the honest
/// shape is a function of how clustered a predicate's matches are, which the
/// planner could know and does not. That is the next measurement, not this one.
///
/// ~~**The scan side is not settled and was not changed.** `cost_calibration`
/// and `cost_at_scale` disagree about what a cold full scan of the same
/// 200,000-row fixture costs — 58 requests against 205 — and until that is
/// understood, moving `SCAN_ROW_COST` would be calibrating against a
/// measurement one of the two says is wrong.~~
///
/// **The disagreement was understood and neither file was wrong**: the two
/// scans ran against different cache states, and `SCAN_ROW_COST` above now
/// carries all four. The scan side is still not settled, for a different and
/// better-stated reason — there are now three measured values spanning 7×, and
/// picking one is a decision about which cache state a planner should assume
/// rather than a calibration.
///
/// That matters here because the two constants are compared against each
/// other, and at 200,000 rows on this fixture the comparison now comes out
/// wrong in a way the file says out loud: `cost_at_scale` prints `chose
/// TableScan but Index is faster — WRONG at this scale`. The index issues 368
/// requests against the scan's 370 — a tie in this unit — and finishes 11×
/// sooner, while the model calls it 15× worse, because the scan is charged at
/// the cached rate and measured at the fragmented one.
pub const POINT_READ_COST: f64 = 1.0;

/// What `n` point reads cost when issued `depth` at a time.
///
/// **They cost the same as issuing them one at a time.** Concurrency hides
/// latency; it does not do less work, and the unit here is work. `depth` is
/// kept in the signature because the caller has it and because the wall-clock
/// story is still worth explaining at the call sites, but it no longer divides
/// anything.
///
/// This used to be `ceil(n / depth)`, which was wrong in a way worth recording,
/// because the reasoning that produced it was sound and the measurement behind
/// it was not. The argument was: reads issued together land together, so `n` of
/// them at depth `d` cost `ceil(n / d)` round trips. That is true of *latency*.
/// It was validated against a latency fixture, where concurrent reads do
/// complete in the time of one — and the fixture only ever modelled time, so it
/// could not have shown the error.
///
/// Against real object storage the two halves of the model were being measured
/// in different units, and the errors compounded in the same direction: scans
/// were charged eighty times too much, index lookups forty times too little.
/// For `WHERE bucket = 7` over 200,000 rows the planner preferred an index scan
/// at cost 30 over a table scan at cost 2001, and the plan it chose did **58
/// times more object-store requests and took nine times longer** — 1,217
/// requests and 3.6 s against 21 requests and 0.4 s.
#[must_use]
pub fn pipelined_read_cost(reads: f64, _depth: usize) -> f64 {
    if reads <= 0.0 {
        return 0.0;
    }
    reads * POINT_READ_COST
}
/// How much of a table a comparison between two of its columns is expected to
/// keep.
///
/// A third, the same as a one-sided range against a literal. Both are guesses;
/// this one cannot be improved by a histogram, because the answer depends on
/// how the two columns vary together and nothing here records that.
pub const COLUMN_RANGE_SELECTIVITY: f64 = 0.33;

/// How much of a table an unanchored `LIKE` is expected to keep.
///
/// A tenth. Nothing recorded here can do better — a histogram describes where
/// values sort, and `'%google%'` asks about substrings, which says nothing
/// about sort position.
pub const LIKE_SELECTIVITY: f64 = 0.1;

/// How much of a table one search term is expected to keep.
///
/// A thousandth, and a guess — the same kind of guess [`LIKE_SELECTIVITY`] is
/// and for a sharper reason: a histogram describes where a *column's values*
/// sort, and a term is not one of them. What answers it is how many rows hold
/// each term, which `analyze` now counts — see [`TermStats`] — so this is the
/// estimate for a column whose terms have **not** been counted: one never
/// analysed, or [`TableStats::assumed`]. An analysed text column does not
/// reach this number.
///
/// The basis for the number is the same one [`ColumnStats::default`] uses, one
/// order the other way: an un-analysed column is assumed to have a hundred
/// distinct values, and a text column's *vocabulary* is far larger than a
/// categorical column's value set — a few thousand distinct words out of a
/// corpus of short titles is ordinary. A thousandth is that assumption.
///
/// **It was 0.05 first, and that made the index unreachable.** Not by a
/// little: at a twentieth the planner chose a table scan at every size from a
/// thousand rows to a million, because a twentieth of a large table is a great
/// many point reads and a scan streams. The measurement is in the ledger
/// entry; what it demonstrates is that a selectivity guess on a non-covering
/// index is not a detail, it is whether the index is ever used at all.
///
/// Getting it wrong costs a plan, never an answer: the residual re-checks
/// every term on every row the scan admits, whichever path produced it.
pub const TERM_SELECTIVITY: f64 = 0.001;

/// Distinct terms [`TermCounter`] tracks per text column.
///
/// The same bound as `DISTINCT_TRACKING_LIMIT`, for the same reason: `analyze`
/// reads the whole table and cannot hold all of it, and a text column's
/// vocabulary is the one statistic here that routinely runs to millions. Below
/// it every count is exact; above it the frequent terms keep counts that are
/// right to within the smallest one held, and the rest share one estimate.
pub const TERM_TRACKING_LIMIT: usize = 10_000;

/// Hashes the vocabulary sketch keeps: a k-minimum-values estimate, whose
/// relative error is about `1 / sqrt(k)` — three percent at this size.
const VOCABULARY_SKETCH: usize = 1_024;

/// How many rows hold each term of one text-indexed column.
///
/// **What the planner needed and `TERM_SELECTIVITY` could not say.** A
/// non-covering index is chosen when it returns about one row in 8,000, so a
/// flat guess of a thousandth sent every search to a table scan — including a
/// search for a word three rows hold, which is the case an inverted index
/// exists for. With this, `earthsea` and `the` are different questions.
///
/// A term's count is the number of *rows* holding it, which is also the
/// number of index entries under it: [`tokenize`](slate_schema::tokenize)
/// deduplicates within a row, so one row writes one entry per distinct term.
#[derive(Debug, Clone, PartialEq)]
pub struct TermStats {
    /// Rows expected to hold each tracked term: exact while the counter never
    /// evicted, and an estimate — see [`TermCounter::finish`] — once it had.
    counts: BTreeMap<String, f64>,
    /// Rows expected to hold a term with no entry above.
    ///
    /// Zero when the count was exhaustive: a term the counter never saw is in
    /// no row. Otherwise the mean of the untracked tail, capped at what the
    /// counter guarantees no untracked term exceeds.
    ///
    /// Not floored at one row, though a term in the tail is in at least one:
    /// both readers already are — the search's selectivity at one row of the
    /// table, the index candidate's rows admitted at one — so a floor here was
    /// a third copy of a rule, and a mutation removing it changed no verdict.
    untracked: f64,
}

impl TermStats {
    /// Counts known to be complete: every term not listed is in no row.
    ///
    /// What a test or a caller with its own counts builds; `analyze` builds
    /// one through [`TermCounter`].
    #[must_use]
    pub fn exact(counts: impl IntoIterator<Item = (String, u64)>) -> Self {
        Self {
            counts: counts
                .into_iter()
                .map(|(term, count)| (term, count as f64))
                .collect(),
            untracked: 0.0,
        }
    }

    /// Rows expected to hold `term`.
    #[must_use]
    pub fn documents(&self, term: &str) -> f64 {
        self.counts.get(term).copied().unwrap_or(self.untracked)
    }
}

/// Counts terms across a table in bounded memory, for [`TermStats`].
///
/// **Space-Saving** (Metwally, Agrawal and El Abbadi, 2005): at most
/// `capacity` counters; a new term arriving when they are full takes the
/// smallest counter's place and inherits its count as its error. Until that
/// first eviction nothing is approximate at all, which for a table whose
/// vocabulary fits is every count exact. After it, two guarantees hold and
/// both are used:
///
/// - a tracked term's true count lies in `count - error ..= count`;
/// - an untracked term's true count is at most the smallest counter.
///
/// **Why not a sample of rows**, which is how the histograms are built: a
/// sample finds the common terms and is blind to exactly the ones that matter
/// here. At a one-percent sample a term in three rows is usually absent, and
/// absent cannot be told from absent-because-rare-enough-to-win. The counter
/// sees every posting.
///
/// **Why the tail needs a second structure.** Space-Saving's counters always
/// sum to the number of postings — an eviction hands the evicted count to the
/// newcomer — so "postings minus what is tracked" is always zero and says
/// nothing. What *was* evicted is exactly the sum of the errors, and it
/// belongs to two kinds of word: the untracked ones, and the occurrences a
/// tracked word had before it took its counter. Both are estimated at one
/// mean, so the mass divides by both — every word except those held since
/// their first occurrence. How many words there were is the k-minimum-values
/// sketch: the `VOCABULARY_SKETCH` smallest hashes seen, whose largest says
/// how densely the hash space was hit.
#[derive(Debug)]
pub struct TermCounter {
    capacity: usize,
    /// The term in each slot, its count, and the error it inherited.
    terms: Vec<(String, u64, u64)>,
    slot: std::collections::HashMap<String, usize>,
    /// `(count, slot)`, so the smallest counter is the first entry. The slot
    /// breaks ties, which makes eviction depend on arrival order and nothing
    /// else: the same table analysed twice gives the same statistics.
    by_count: std::collections::BTreeSet<(u64, usize)>,
    evicted: bool,
    sketch: std::collections::BTreeSet<u64>,
}

impl TermCounter {
    /// A counter holding at most `capacity` terms.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            terms: Vec::new(),
            slot: std::collections::HashMap::new(),
            by_count: std::collections::BTreeSet::new(),
            evicted: false,
            sketch: std::collections::BTreeSet::new(),
        }
    }

    /// One row holds `term`. Call once per distinct term per row.
    pub fn add(&mut self, term: &str) {
        let hash = term_hash(term);
        if self.sketch.len() < VOCABULARY_SKETCH {
            self.sketch.insert(hash);
        } else if let Some(&largest) = self.sketch.last()
            && hash < largest
            && self.sketch.insert(hash)
        {
            self.sketch.pop_last();
        }

        // Every slot `slot` and `by_count` name is one `terms` holds: slots
        // are only ever appended or overwritten in place, never removed.
        if let Some(&at) = self.slot.get(term)
            && let Some((_, count, _)) = self.terms.get_mut(at)
        {
            self.by_count.remove(&(*count, at));
            *count += 1;
            self.by_count.insert((*count, at));
            return;
        }
        if self.terms.len() < self.capacity {
            let at = self.terms.len();
            self.terms.push((term.to_owned(), 1, 0));
            self.slot.insert(term.to_owned(), at);
            self.by_count.insert((1, at));
            return;
        }
        let Some((smallest, at)) = self.by_count.pop_first() else {
            return;
        };
        let Some((old, count, error)) = self.terms.get_mut(at) else {
            return;
        };
        self.evicted = true;
        self.slot.remove(old.as_str());
        old.clear();
        old.push_str(term);
        *count = smallest + 1;
        *error = smallest;
        self.slot.insert(term.to_owned(), at);
        self.by_count.insert((smallest + 1, at));
    }

    /// Distinct terms seen, exactly while the sketch is not full.
    fn vocabulary(&self) -> f64 {
        if self.sketch.len() < VOCABULARY_SKETCH {
            return self.sketch.len() as f64;
        }
        let largest = self.sketch.last().copied().unwrap_or(u64::MAX).max(1);
        (VOCABULARY_SKETCH - 1) as f64 / (largest as f64 / u64::MAX as f64)
    }

    /// The statistics, and the end of counting.
    ///
    /// After an eviction, three choices, each made against an exact count of
    /// the same stream rather than argued for:
    ///
    /// - **A tracked word holds what it was counted since it took its counter,
    ///   plus the tail's mean if it inherited an error.** Its occurrences
    ///   before then are exactly an untracked word's, which is what the mean
    ///   estimates; a word with no error has been held since its first
    ///   occurrence and is exact. Against an exact count of a 5,000-word Zipf
    ///   stream through 1,000 counters, its mean absolute error was 6.2 rows,
    ///   the lower bound alone 7.0 and the counter's upper bound 10.3; that
    ///   comparison is a test, so the rule cannot quietly become the worse
    ///   one.
    /// - **The tail's mean divides the evicted mass by every word not held
    ///   since its first occurrence**, the accounting above. The first draft
    ///   divided by the untracked words only, which counted the mass once and
    ///   handed it out twice: on a 1,500-word vocabulary through 1,000
    ///   counters it put the mean at 34.7 rows against a true 15.8, where this
    ///   says 15.9.
    /// - **An untracked word's estimate is capped at the smallest counter**,
    ///   which Space-Saving guarantees no untracked word exceeds. Not
    ///   decoration: 120 words cycling through 100 counters put the draft's
    ///   uncapped mean at 4,995 rows against a ceiling of 1,000.
    #[must_use]
    pub fn finish(self) -> TermStats {
        if !self.evicted {
            return TermStats::exact(self.terms.into_iter().map(|(t, c, _)| (t, c)));
        }
        let smallest = self.by_count.first().map_or(0, |&(count, _)| count) as f64;
        let evicted: u64 = self.terms.iter().map(|(_, _, error)| error).sum();
        let exact = self
            .terms
            .iter()
            .filter(|(_, _, error)| *error == 0)
            .count();
        let sharing = (self.vocabulary() - exact as f64).max(1.0);
        let untracked = (evicted as f64 / sharing).min(smallest);
        TermStats {
            counts: self
                .terms
                .into_iter()
                .map(|(term, count, error)| {
                    let since = (count - error) as f64;
                    (term, if error > 0 { since + untracked } else { since })
                })
                .collect(),
            untracked,
        }
    }
}

/// A term's position in the vocabulary sketch's hash space.
///
/// FNV-1a, then SplitMix64's finaliser: FNV alone is fast and fixed but
/// clusters short similar strings, which is the whole vocabulary of a text
/// column, and a k-minimum-values estimate assumes hashes spread uniformly.
/// Fixed rather than `RandomState` so two analyses of one table agree.
fn term_hash(term: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in term.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ (hash >> 31)
}

/// Cost of one comparison level when sorting a row: CPU only, no I/O, so
/// several orders of magnitude below a round trip.
pub const SORT_ROW_COST: f64 = 0.000_02;

/// What is known about one column's contents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnStats {
    /// Distinct values. One means every row shares a value; `row_count` means
    /// the column is unique.
    pub distinct: u64,
    /// Fraction of rows where the column is null, in `0.0..=1.0`.
    pub null_fraction: f64,
}

impl Default for ColumnStats {
    fn default() -> Self {
        // Stands in for an un-analysed column: selective enough to be worth
        // indexing, not so selective that the planner bets everything on it.
        Self {
            distinct: 100,
            null_fraction: 0.1,
        }
    }
}

/// Buckets holding roughly equal numbers of rows, describing how one column's
/// values are spread.
///
/// Without one, a range predicate gets a fixed guess and the planner cannot
/// tell `at < 10` from `at < 500` — on the benchmark corpus that meant
/// scanning 2500 rows in 58 ms to return the ten an index finds in 4.5 ms.
///
/// Equi-depth rather than equi-width: buckets of equal *population*, so a
/// column with a long tail spends its resolution where the rows are. The
/// bounds are quantiles of a sample, so bucket `i` covers roughly
/// `1/buckets` of the table whatever the shape of the data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    /// Bucket boundaries, sorted. `buckets + 1` of them: bucket `i` holds
    /// values in `bounds[i] ..= bounds[i + 1]`.
    bounds: Vec<Value>,
}

/// Buckets a histogram is built with.
///
/// Sixty-four puts the resolution at about 1.5% of the table, which is fine
/// against a crossover that sits near 6% and an estimate that is a sample
/// anyway.
pub const HISTOGRAM_BUCKETS: usize = 64;

/// Values sampled per column when building a histogram.
///
/// Bounded because `analyze` reads the whole table and cannot hold all of it.
pub const HISTOGRAM_SAMPLE: usize = 10_000;

impl Histogram {
    /// Build from observed values, which need not be sorted.
    ///
    /// `None` when there is too little to describe: one distinct value is not
    /// a distribution, and a histogram claiming otherwise would be worse than
    /// the fixed guess it replaces.
    #[must_use]
    pub fn from_values(mut values: Vec<Value>) -> Option<Self> {
        values.retain(|v| !v.is_null());
        if values.len() < HISTOGRAM_BUCKETS {
            return None;
        }
        values.sort();
        if values.first() == values.last() {
            return None;
        }
        let mut bounds = Vec::with_capacity(HISTOGRAM_BUCKETS + 1);
        for i in 0..=HISTOGRAM_BUCKETS {
            // Quantile i/buckets, clamped so the last index is in range.
            let at = (i * (values.len() - 1)) / HISTOGRAM_BUCKETS;
            if let Some(value) = values.get(at) {
                bounds.push(value.clone());
            }
        }
        (bounds.len() > 1).then_some(Self { bounds })
    }

    /// The fraction of rows below `value`, in `0.0..=1.0`.
    ///
    /// Resolved to the bucket and no further. Interpolating inside a bucket
    /// would need arithmetic on [`Value`], which is a closed type holding
    /// strings and uuids as well as numbers; the midpoint is honest about what
    /// the histogram actually knows.
    #[must_use]
    pub fn fraction_below(&self, value: &Value) -> f64 {
        let buckets = self.bounds.len().saturating_sub(1);
        if buckets == 0 {
            return 0.5;
        }
        match self.bounds.binary_search(value) {
            // Exactly on a boundary: everything before that bucket.
            Ok(index) => (index as f64 / buckets as f64).clamp(0.0, 1.0),
            Err(0) => 0.0,
            Err(index) if index > buckets => 1.0,
            // Inside bucket `index - 1`; call it half way through.
            Err(index) => ((index as f64 - 0.5) / buckets as f64).clamp(0.0, 1.0),
        }
    }

    /// The bucket boundaries.
    #[must_use]
    pub fn bounds(&self) -> &[Value] {
        &self.bounds
    }
}

/// What one statistic describes.
///
/// A column, or the value an *expression* index keys on. The second is not a
/// column and has no ordinal of its own — `lower(email)` is nowhere in the
/// row — so it is named by the index that computes it, which is the only
/// durable name it has.
///
/// One key type rather than a second pair of maps: the two kinds are described
/// by exactly the same [`ColumnStats`] and [`Histogram`], estimated by exactly
/// the same code, and a parallel set of maps would be a second place for a
/// selectivity rule to be written down and to drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StatTarget {
    /// A column of the table.
    Column(Ordinal),
    /// The value an expression index keys on.
    Expression(IndexId),
}

/// What is known about one table's contents.
#[derive(Debug, Clone, PartialEq)]
pub struct TableStats {
    /// Rows in the table.
    pub row_count: u64,
    columns: BTreeMap<StatTarget, ColumnStats>,
    /// How each column's values are spread, where that has been measured.
    ///
    /// Held apart from [`ColumnStats`] so that stays small and `Copy`: a
    /// histogram is a hundred values and is read on far fewer paths.
    histograms: BTreeMap<StatTarget, Histogram>,
    /// Rows holding each term, for each column a text index is on.
    terms: BTreeMap<Ordinal, TermStats>,
}

impl Default for TableStats {
    fn default() -> Self {
        Self::assumed()
    }
}

impl TableStats {
    /// The estimates used before anything has been analysed.
    ///
    /// A thousand rows, a hundred distinct values per column. Wrong for any
    /// particular table, and better than assuming the structure of a predicate
    /// tells you how many rows it selects.
    #[must_use]
    pub const fn assumed() -> Self {
        Self {
            row_count: 1_000,
            columns: BTreeMap::new(),
            histograms: BTreeMap::new(),
            terms: BTreeMap::new(),
        }
    }

    /// Statistics for a table of a known size, with default column estimates.
    #[must_use]
    pub const fn with_row_count(row_count: u64) -> Self {
        Self {
            row_count,
            columns: BTreeMap::new(),
            histograms: BTreeMap::new(),
            terms: BTreeMap::new(),
        }
    }

    /// Record what is known about a column.
    #[must_use]
    pub fn with_column(mut self, ordinal: Ordinal, stats: ColumnStats) -> Self {
        self.columns.insert(StatTarget::Column(ordinal), stats);
        self
    }

    /// Record how a column's values are spread.
    #[must_use]
    pub fn with_histogram(mut self, ordinal: Ordinal, histogram: Histogram) -> Self {
        self.histograms
            .insert(StatTarget::Column(ordinal), histogram);
        self
    }

    /// Record what is known about the value an expression index keys on.
    ///
    /// Against the index rather than against an ordinal, because the value is
    /// not in the row. A query that computes the same expression does give it
    /// an ordinal — computed values are appended after the table's own columns
    /// — but that ordinal depends on the query's own list of computed values,
    /// so it is no name to store anything under.
    #[must_use]
    pub fn with_expression(mut self, index: IndexId, stats: ColumnStats) -> Self {
        self.columns.insert(StatTarget::Expression(index), stats);
        self
    }

    /// Record how the values an expression index keys on are spread.
    #[must_use]
    pub fn with_expression_histogram(mut self, index: IndexId, histogram: Histogram) -> Self {
        self.histograms
            .insert(StatTarget::Expression(index), histogram);
        self
    }

    /// Record how many rows hold each term of a text-indexed column.
    #[must_use]
    pub fn with_terms(mut self, ordinal: Ordinal, terms: TermStats) -> Self {
        self.terms.insert(ordinal, terms);
        self
    }

    /// The fraction of rows holding `term` in `ordinal`'s text.
    ///
    /// [`TERM_SELECTIVITY`] when the column's terms have not been counted,
    /// which keeps every unanalysed search costed exactly as it was.
    #[must_use]
    pub fn term_selectivity(&self, ordinal: Ordinal, term: &str) -> f64 {
        match self.terms.get(&ordinal) {
            Some(terms) => {
                let rows = self.row_count.max(1) as f64;
                (terms.documents(term) / rows).clamp(0.0, 1.0)
            }
            None => TERM_SELECTIVITY,
        }
    }

    /// How `ordinal`'s values are spread, if that has been measured.
    #[must_use]
    pub fn histogram(&self, ordinal: Ordinal) -> Option<&Histogram> {
        self.histograms.get(&StatTarget::Column(ordinal))
    }

    /// What is known about the value `index` keys on, if it has been analysed.
    ///
    /// `Option` rather than the default a column gets, because the caller has
    /// to be able to tell "measured, and it looks like the default" from "never
    /// measured": the planner copies these onto a computed ordinal for the
    /// duration of a query and there is no point copying a default.
    #[must_use]
    pub fn expression(&self, index: IndexId) -> Option<ColumnStats> {
        self.columns.get(&StatTarget::Expression(index)).copied()
    }

    /// How the values `index` keys on are spread, if that has been measured.
    #[must_use]
    pub fn expression_histogram(&self, index: IndexId) -> Option<&Histogram> {
        self.histograms.get(&StatTarget::Expression(index))
    }

    /// What is known about `ordinal`, or the default.
    #[must_use]
    pub fn column(&self, ordinal: Ordinal) -> ColumnStats {
        self.columns
            .get(&StatTarget::Column(ordinal))
            .copied()
            .unwrap_or_default()
    }

    /// The fraction of rows an equality on `ordinal` is expected to keep.
    #[must_use]
    pub fn equality_selectivity(&self, ordinal: Ordinal) -> f64 {
        let stats = self.column(ordinal);
        let distinct = stats.distinct.max(1) as f64;
        // Nulls never match an equality, so they are not among the candidates.
        ((1.0 - stats.null_fraction) / distinct).clamp(f64::MIN_POSITIVE, 1.0)
    }

    /// The fraction a range comparison is expected to keep, knowing nothing
    /// about where the value falls.
    ///
    /// The fallback for a column with no histogram. It cannot tell `at < 10`
    /// from `at < 500`, which is the whole reason [`Histogram`] exists.
    #[must_use]
    pub fn range_selectivity(&self, ordinal: Ordinal, one_sided: bool) -> f64 {
        let stats = self.column(ordinal);
        let base = if one_sided { 0.33 } else { 0.1 };
        (base * (1.0 - stats.null_fraction)).clamp(f64::MIN_POSITIVE, 1.0)
    }

    /// The fraction of rows whose value starts with `prefix`.
    ///
    /// The range between the prefix and the next string above it, which the
    /// histogram already knows how to answer. Without a histogram it is the
    /// same flat guess an unanchored pattern gets.
    #[must_use]
    pub fn prefix_selectivity(&self, ordinal: Ordinal, prefix: &str) -> f64 {
        let Some(histogram) = self.histogram(ordinal) else {
            return LIKE_SELECTIVITY;
        };
        let low = Value::Str(prefix.to_owned());
        // The successor of the prefix: the same string with its last character
        // bumped, which is the first value that does not start with it.
        let mut upper = prefix.to_owned();
        upper.push(char::MAX);
        let high = Value::Str(upper);
        let span = histogram.fraction_below(&high) - histogram.fraction_below(&low);
        span.clamp(f64::MIN_POSITIVE, 1.0)
    }

    /// The fraction `bounds` keeps, using the column's histogram if there is
    /// one and [`TableStats::range_selectivity`] if there is not.
    ///
    /// Nulls never satisfy a comparison, so whatever the bounds keep is
    /// scaled by the fraction of rows that are not null.
    #[must_use]
    pub fn bounded_selectivity(&self, ordinal: Ordinal, bounds: &[(CmpOp, &Value)]) -> f64 {
        let Some(histogram) = self.histogram(ordinal) else {
            return self.range_selectivity(ordinal, bounds.len() == 1);
        };
        // Start with everything and narrow by each bound. Two bounds on one
        // column are an interval, and an interval is what is left after
        // cutting from both ends.
        let mut low = 0.0f64;
        let mut high = 1.0f64;
        for (op, value) in bounds {
            if value.is_null() {
                return 0.0;
            }
            let at = histogram.fraction_below(value);
            match op {
                CmpOp::Lt | CmpOp::Le => high = high.min(at),
                CmpOp::Gt | CmpOp::Ge => low = low.max(at),
                CmpOp::Eq => return self.equality_selectivity(ordinal),
                CmpOp::Ne => return 1.0 - self.equality_selectivity(ordinal),
            }
        }
        let not_null = 1.0 - self.column(ordinal).null_fraction;
        ((high - low).max(0.0) * not_null).clamp(f64::MIN_POSITIVE, 1.0)
    }

    /// The fraction of rows a whole predicate is expected to keep.
    #[must_use]
    pub fn predicate_selectivity(&self, predicate: &Expr) -> f64 {
        match predicate {
            Expr::True => 1.0,
            Expr::False => 0.0,
            Expr::Compare { column, op, value } => {
                if value.is_null() {
                    // Comparing with null is unknown for every row.
                    return 0.0;
                }
                match op {
                    CmpOp::Eq => self.equality_selectivity(*column),
                    CmpOp::Ne => 1.0 - self.equality_selectivity(*column),
                    CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
                        self.bounded_selectivity(*column, &[(*op, value)])
                    }
                }
            }
            // Two columns compared with no literal in sight. There is no
            // histogram that would help — that would need a joint
            // distribution — so this is a guess, and an equality between two
            // columns is guessed the way a join's is: one over the coarser
            // of the two distinct counts.
            Expr::CompareColumns { left, op, right } => {
                let coarser = self
                    .column(*left)
                    .distinct
                    .min(self.column(*right).distinct)
                    .max(1) as f64;
                match op {
                    CmpOp::Eq => 1.0 / coarser,
                    CmpOp::Ne => 1.0 - 1.0 / coarser,
                    CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => COLUMN_RANGE_SELECTIVITY,
                }
            }
            // A pattern anchored at the front narrows to whatever share of
            // the column starts that way, which the histogram can answer: it
            // is the range between the prefix and its successor. A pattern
            // that can start anywhere gets a guess, because nothing recorded
            // here says how often a substring occurs.
            Expr::Like {
                column,
                pattern,
                negated,
                insensitive,
            } => {
                let matched = match crate::expr::like_prefix(pattern) {
                    // A case-insensitive prefix is not one span of the
                    // keyspace, so the histogram cannot answer it.
                    Some(prefix) if !*insensitive => self.prefix_selectivity(*column, &prefix),
                    _ => LIKE_SELECTIVITY,
                };
                if *negated { 1.0 - matched } else { matched }
            }
            // Each term independently, which is the same independence
            // assumption the rest of this file makes and is *more* wrong here:
            // words in one document correlate strongly, so two terms of a
            // phrase keep far more than a twentieth of a twentieth. Floored at
            // one row rather than allowed to reach zero, because a plan
            // costing an empty result reads nothing and a search that matches
            // one document is the case this index exists for.
            //
            // No terms is no rows: see `Expr::Contains`.
            //
            // With counted terms each factor is that term's own fraction, so a
            // search naming a rare word is estimated as rare; without, each is
            // `TERM_SELECTIVITY`, which is the estimate this always gave.
            Expr::Contains { column, terms } => {
                if terms.is_empty() {
                    return 0.0;
                }
                let independent: f64 = terms
                    .iter()
                    .take(8)
                    .map(|term| self.term_selectivity(*column, term))
                    .product();
                independent.max(1.0 / (self.row_count.max(1)) as f64)
            }
            // A regular expression says nothing about where its matches sort,
            // whatever it is anchored on — `^abc` is a prefix, but so is
            // `^(a|b)`, and telling them apart means understanding the syntax
            // rather than reading it.
            Expr::Matches { negated, .. } => {
                if *negated {
                    1.0 - LIKE_SELECTIVITY
                } else {
                    LIKE_SELECTIVITY
                }
            }
            Expr::IsNull { column, negated } => {
                let fraction = self.column(*column).null_fraction;
                if *negated { 1.0 - fraction } else { fraction }
            }
            Expr::In { column, values } => {
                let each = self.equality_selectivity(*column);
                (each * values.len() as f64).clamp(0.0, 1.0)
            }
            // Same estimate as `In`, from the deduplicated length. A list with
            // repeats estimated *higher* before it was prepared, which was
            // always wrong — the duplicates select the same rows — so the two
            // forms can disagree here, and the prepared one is the correct
            // side of the disagreement.
            Expr::InSorted { column, values, .. } => {
                let each = self.equality_selectivity(*column);
                (each * values.len() as f64).clamp(0.0, 1.0)
            }
            // Independence: the standard assumption, and the standard way to be
            // wrong. See the module docs.
            Expr::And(parts) => parts
                .iter()
                .map(|p| self.predicate_selectivity(p))
                .product::<f64>()
                .clamp(0.0, 1.0),
            Expr::Or(parts) => {
                let none_match: f64 = parts
                    .iter()
                    .map(|p| 1.0 - self.predicate_selectivity(p))
                    .product();
                (1.0 - none_match).clamp(0.0, 1.0)
            }
            Expr::Not(inner) => (1.0 - self.predicate_selectivity(inner)).clamp(0.0, 1.0),
        }
    }
}

/// Statistics for every table a store serves.
#[derive(Debug, Clone, Default)]
pub struct Statistics {
    tables: BTreeMap<TableId, TableStats>,
}

impl Statistics {
    /// Empty: every table falls back to [`TableStats::assumed`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
        }
    }

    /// Record statistics for a table.
    pub fn set(&mut self, table: TableId, stats: TableStats) {
        self.tables.insert(table, stats);
    }

    /// Record statistics for a table, chaining.
    #[must_use]
    pub fn with(mut self, table: TableId, stats: TableStats) -> Self {
        self.set(table, stats);
        self
    }

    /// What is known about `table`, or the defaults.
    #[must_use]
    pub fn table(&self, table: &TableDef) -> TableStats {
        self.tables
            .get(&table.id())
            .cloned()
            .unwrap_or_else(TableStats::assumed)
    }

    /// Whether anything has been recorded for `table`.
    #[must_use]
    pub fn has(&self, table: TableId) -> bool {
        self.tables.contains_key(&table)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::cast_precision_loss)]

    use super::{TermCounter, VOCABULARY_SKETCH};
    use std::collections::HashMap;

    /// A Zipf-distributed stream of `postings` terms over `vocabulary` words,
    /// with exponent `s`, from a fixed seed — the shape a text column's words
    /// actually have, which is a few very common words and a long tail held by
    /// one or two rows each.
    fn zipf(postings: usize, vocabulary: usize, s: f64) -> Vec<String> {
        let weights: Vec<f64> = (1..=vocabulary)
            .map(|rank| 1.0 / (rank as f64).powf(s))
            .collect();
        let total: f64 = weights.iter().sum();
        let mut cumulative = Vec::with_capacity(vocabulary);
        let mut running = 0.0;
        for weight in &weights {
            running += weight / total;
            cumulative.push(running);
        }
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        (0..postings)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let draw = (state >> 11) as f64 / (1u64 << 53) as f64;
                let rank = cumulative
                    .partition_point(|&c| c < draw)
                    .min(vocabulary - 1);
                format!("w{rank}")
            })
            .collect()
    }

    fn oracle(stream: &[String]) -> HashMap<&str, u64> {
        let mut counts = HashMap::new();
        for term in stream {
            *counts.entry(term.as_str()).or_insert(0) += 1;
        }
        counts
    }

    #[test]
    fn every_count_is_exact_while_the_vocabulary_fits() {
        let stream = zipf(20_000, 500, 1.1);
        let truth = oracle(&stream);
        let mut counter = TermCounter::new(truth.len());
        for term in &stream {
            counter.add(term);
        }
        let stats = counter.finish();
        for (term, &count) in &truth {
            assert_eq!(stats.documents(term), count as f64, "{term}");
        }
        // Exhaustive, so a word it never saw is in no row — not in "a few".
        assert_eq!(stats.documents("never-written"), 0.0);
    }

    /// The two guarantees Space-Saving makes, checked against an exact count
    /// of the same stream. Capacity is a fiftieth of the vocabulary, so most
    /// words are evicted and the tail estimate does real work.
    #[test]
    fn past_capacity_the_counts_keep_space_savings_guarantees() {
        let postings = 200_000;
        let capacity = 1_000;
        let stream = zipf(postings, 50_000, 1.1);
        let truth = oracle(&stream);
        let mut counter = TermCounter::new(capacity);
        for term in &stream {
            counter.add(term);
        }
        let exact_from_the_start: std::collections::HashSet<String> = counter
            .terms
            .iter()
            .filter(|(_, _, error)| *error == 0)
            .map(|(term, ..)| term.clone())
            .collect();
        assert!(
            !exact_from_the_start.is_empty(),
            "the stream held no word from the start"
        );
        let stats = counter.finish();
        let bound = (postings / capacity) as f64;

        assert!(
            stats.untracked <= bound,
            "the tail estimate {} exceeds the bound {bound} no untracked word can",
            stats.untracked
        );
        // A word held since its first occurrence never inherited an error, so
        // its count is not an estimate at all.
        for (term, &count) in &truth {
            if exact_from_the_start.contains(*term) {
                assert_eq!(stats.documents(term), count as f64, "{term}");
            }
        }
        // Every word, tracked or not, within N / capacity. A tracked word is
        // off by the tail's mean less what it held before taking a counter,
        // and both are at most the smallest counter; an untracked word and its
        // estimate are each at most the smallest counter too.
        for (term, &count) in &truth {
            let estimate = stats.documents(term);
            assert!(
                (estimate - count as f64).abs() <= bound,
                "{term}: holds {count}, estimated {estimate}, bound {bound}"
            );
        }
    }

    /// The one condition the ceiling exists for: the sketch reading the
    /// vocabulary low.
    ///
    /// With the vocabulary right, the evicted mass over the words sharing it
    /// cannot exceed the smallest counter — every error is at most that, and
    /// there are at least as many sharers as errors — and a search of 148
    /// streams with a vocabulary just past capacity found none that did. The
    /// sketch is an estimate, though, and a low one shrinks the divisor. So
    /// this counts 1,200 words cycling through 1,150 counters, then makes the
    /// sketch say about 1,023, which is what a low reading looks like.
    #[test]
    fn a_vocabulary_read_low_keeps_the_tail_under_its_ceiling() {
        let postings = 120_000;
        let capacity = 1_150;
        let mut counter = TermCounter::new(capacity);
        for i in 0..postings {
            counter.add(&format!("c{}", i % 1_200));
        }
        let step = u64::MAX / VOCABULARY_SKETCH as u64;
        counter.sketch = (1..=VOCABULARY_SKETCH as u64).map(|i| i * step).collect();
        assert!(
            counter.vocabulary() < 1_100.0,
            "the sketch was not read low"
        );
        let stats = counter.finish();
        let ceiling = (postings / capacity) as f64;
        assert!(
            stats.untracked <= ceiling,
            "tail estimated at {} rows, above the {ceiling} no untracked word can hold",
            stats.untracked
        );
    }

    /// The rule for a word that inherited an error, against the rules it was
    /// chosen over, on the stream where they differed most. An oracle rather
    /// than a fixed number: whichever rule is cheaper to state, this one has
    /// to stay the more accurate.
    #[test]
    fn a_word_that_inherited_an_error_is_estimated_better_than_its_bounds() {
        let stream = zipf(200_000, 5_000, 1.1);
        let truth = oracle(&stream);
        let mut counter = TermCounter::new(1_000);
        for term in &stream {
            counter.add(term);
        }
        let held: HashMap<String, (u64, u64)> = counter
            .terms
            .iter()
            .map(|(term, count, error)| (term.clone(), (*count, *error)))
            .collect();
        let stats = counter.finish();
        let error = |rule: &dyn Fn(&str) -> f64| {
            truth
                .iter()
                .map(|(term, &count)| (rule(term) - count as f64).abs())
                .sum::<f64>()
                / truth.len() as f64
        };
        let ours = error(&|term| stats.documents(term));
        let lower = error(&|term| {
            held.get(term)
                .map_or(stats.untracked, |&(count, error)| (count - error) as f64)
        });
        let upper = error(&|term| {
            held.get(term)
                .map_or(stats.untracked, |&(count, _)| count as f64)
        });
        println!("mean absolute error: ours {ours:.3}, lower bound {lower:.3}, upper {upper:.3}");
        assert!(
            ours < lower && ours < upper,
            "{ours} against {lower} and {upper}"
        );
    }

    /// Within six percent, which is about two standard errors for a sketch of
    /// 1,024 hashes. Tighter than "close" on purpose: without the finaliser in
    /// `term_hash`, FNV-1a alone missed two of these vocabularies by 8.5% and
    /// 9.9%, and a bar of ten percent passed it.
    #[test]
    fn the_vocabulary_is_estimated_to_within_two_standard_errors() {
        let mut streams: Vec<Vec<String>> = [VOCABULARY_SKETCH / 2, 5_000, 40_000]
            .into_iter()
            .map(|distinct| {
                (0..distinct)
                    .flat_map(|i| [format!("t{i}"), format!("t{i}")])
                    .collect()
            })
            .collect();
        streams.push(zipf(200_000, 50_000, 1.1));
        streams.push(zipf(200_000, 5_000, 1.1));
        streams.push(zipf(500_000, 200_000, 1.1));
        for stream in streams {
            let distinct = oracle(&stream).len();
            let mut counter = TermCounter::new(16);
            for term in &stream {
                counter.add(term);
            }
            let estimate = counter.vocabulary();
            let error = (estimate - distinct as f64).abs() / distinct as f64;
            assert!(error <= 0.06, "{distinct} distinct estimated as {estimate}");
            if distinct < VOCABULARY_SKETCH {
                assert_eq!(estimate, distinct as f64, "below the sketch it counts");
            }
        }
    }

    /// The tail estimate against the tail's true mean. Not a guarantee — the
    /// mean of a Zipf tail is a property of the distribution, not of the
    /// algorithm — so this asserts a factor and prints what it found.
    ///
    /// Two streams: a long tail, and a vocabulary half again the counter's
    /// size, which is where the accounting of the evicted mass decides the
    /// answer. The draft's denominator was off by 2.2x on the second and
    /// within the factor on the first, which is why the first alone was not
    /// enough to catch it.
    #[test]
    fn the_tail_estimate_is_near_the_tails_true_mean() {
        for (vocabulary, factor) in [(50_000, 1.5), (1_500, 1.15)] {
            tail_within(vocabulary, factor);
        }
    }

    fn tail_within(vocabulary: usize, factor: f64) {
        let stream = zipf(200_000, vocabulary, 1.1);
        let truth = oracle(&stream);
        let mut counter = TermCounter::new(1_000);
        for term in &stream {
            counter.add(term);
        }
        let stats = counter.finish();
        let untracked: Vec<f64> = truth
            .iter()
            .filter(|(term, _)| !stats.counts.contains_key(**term))
            .map(|(_, &count)| count as f64)
            .collect();
        let mean = untracked.iter().sum::<f64>() / untracked.len() as f64;
        println!(
            "tail: {} words untracked, true mean {mean:.2}, estimated {:.2}",
            untracked.len(),
            stats.untracked
        );
        assert!(
            stats.untracked / mean < factor && mean / stats.untracked < factor,
            "tail estimated at {} against a true mean of {mean}",
            stats.untracked
        );
    }

    #[test]
    fn two_analyses_of_one_stream_agree() {
        let stream = zipf(50_000, 20_000, 1.1);
        let run = || {
            let mut counter = TermCounter::new(500);
            for term in &stream {
                counter.add(term);
            }
            counter.finish()
        };
        assert_eq!(run(), run());
    }
}
