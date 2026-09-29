# Seventy-two `deliberate` verdicts read, and two of them were false

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/caveat-status.json`
- **Kind:** process

## What changed

Seventy-two `deliberate` verdicts, drawn as a seeded random sample from the 995
that carry a live bullet, were read against the tree. Two were false and are
re-recorded:

- `2026-09-27-two-verdicts-rested-on-premises-that-had-moved.md` :: *"The
  `narrowed` residual is the measurement, and nothing schedules it."* →
  **`open`**.
- `2026-09-28-the-demo-showed-a-question-mark-for-four-wire-types.md` ::
  *"`NOT_IN_THE_UI` is now empty"* → **`moment`**.

The open count did not move: one verdict left `open` in the same commit for an
unrelated reason, so 126 became 126. That is a coincidence and not a result.

## Why

`deliberate` is the largest bucket in the tracker by a factor of eight and the
only one nothing re-reads. `open` and `narrowed` carry a `checked` date and get
swept; `closed` carries a witness a script can verify; `deliberate` carries a
`reviewed` date and a `by` that is an *argument*, and no mechanism grades an
argument. Before today it had been sampled once, at fifteen, with one found
false — a rate of 7% on a sample too small to distinguish from 0% or 30%.

Two cheaper routes into the bucket were tried and rejected earlier today: a
path-dependency flag and a citation flag, both of which found mostly noise.
Reading is what is left, and it had not been done at any scale.

**The first finding is the shape of the falsity rather than the rate.** Both
false verdicts were true when written and were overtaken. The scheduler one was
written on the 27th saying *"there is no cron, no queue, nothing that runs
between sessions"*, and `.github/workflows/mutations.yml` acquired a
`schedule:` on the 29th — two days later, in this repository, by a session that
had no reason to look at a caveat about scheduling. The `NOT_IN_THE_UI` one
recorded a roster as empty; it has eleven endpoints in it now.

Neither is a bad judgement. Both are a verdict outliving the fact it rested on,
which is the same failure that
`2026-09-27-two-verdicts-rested-on-premises-that-had-moved.md` is itself about — including, as it turns out, one of its own.

## Alternatives rejected

**Read all 995.** The honest answer to "is the bucket sound", and roughly forty
times this session's reading. Not taken because the sample answers the question
the bucket actually raises — *is `deliberate` a place where things quietly
rot* — and the answer is yes at a few per cent, which is enough to justify
re-reading as a standing habit without needing the exact figure. Reading all of
them to get a precise rate and then not changing what anyone does with it would
be the expensive way to learn nothing new.

**Stratify by age, on the theory that older verdicts are staler.** Tempting and
it would have concentrated the yield. Rejected because it prejudges the
mechanism, and the sample says the prejudgement is wrong: both false verdicts
were **two days old**. Overtaking by a later session in the same week is what
breaks these, not slow decay, so an age-stratified sample would have
systematically avoided the failures it was meant to find.

**Write a guard that re-derives each `by`.** The thing that would make this
unnecessary. Not possible in general: a `by` like *"the alternative is a
scheduler, and there is none"* is a claim about the world that a script cannot
evaluate without knowing what a scheduler is. A guard for the narrow subclass
that names a file could be written, and the citation flag measured earlier
today is that guard — it found mostly noise, which is why it was rejected.

**Re-record the two as `open` and stop.** What a smaller version of this would
have done. Not taken for the second one: `NOT_IN_THE_UI` being non-empty is not
undone work, it is a passing observation whose moment passed, and `moment` is
the verdict for exactly that. Filing it as `open` would put a non-task on the
open list, which is the thing that makes an open list stop being read.

## Evidence

Seventy-two verdicts read, sampled with a fixed seed from the 995 with a live
bullet, in six batches of twelve. Two false: **2.8%**, and that number should
be read with its interval rather than on its own — 2 in 72 is consistent with
anything from about 0.3% to 9.7%, which includes the earlier 1-in-15.

Extrapolated across 995 at the point estimate that is about **28 hidden
verdicts**, and at the top of the interval about 97. The honest summary is
"tens, not hundreds, and not zero".

Of the seventy-two, twelve were checked mechanically against the tree rather than
read for plausibility, and all twelve held:

| claim | checked against | holds |
| --- | --- | --- |
| `year()` is constant on this sample, and a test pins it | `crates/slate-wasm/tests/datetime.rs`, `BTreeMap::from([(2024, 100_000)])` | yes |
| `--web` emits no types, checks or foreign keys | `scripts/codegen.py`, `web_module` | yes |
| the `IN (SELECT …)` elision is untested by the Rust suite | it is in `site/workbench.js`, not Rust at all | yes |
| the `rust` job runs `cargo test --workspace` | `.github/workflows/ci.yml` | yes |
| no client gets a seed factory | the three client packages | yes |
| `slate-wasm`'s SQL front end has no array literal | no `array` in `crates/slate-sql/src/` | yes |
| the Python suite cannot prove the server logs what it sent | `clients/python/testserver/src/main.rs` has no request log | yes |
| a join's `WHERE` still takes `AND` only | `crates/slate-sql/src/sql.rs`, lines 197–199 | yes |
| `DropIndex` prints an index id, not a name | `crates/slate-serverd/src/main.rs`, `index.0` | yes |
| no corpus sample carries the load-failure wrapper | `scripts/test_mutate.py`, `CORPUS` is 3-wide | yes |
| a path inside a longer word is skipped, and skips nothing today | re-derived: **0** such words remain | yes |
| fourteen zones, not six hundred | `crates/slate-kernel/src/zones.rs`, exactly 14 | yes |

The last two are worth separating out. The path count was re-derived rather
than trusted because that entry is from **today** and
`scripts/check_cited_files.py` was widened today: a count written before the
widening would have been exactly the overtaking failure this sample was looking
for. It was written after, and re-deriving returned 0, which is what the entry
claims.

The zone one nearly produced a **false** finding in the other direction. The
first check read `taxi_zones.csv` — 265 rows — and concluded the claim was
wrong by a factor of nineteen. `taxi_zones.csv` is NYC taxi pickup zones;
`zones.rs` is IANA timezones; they are different things that share a word. The
claim was verified against the right artefact before anything was recorded,
which is the only reason this is a paragraph rather than a retraction.

## What this does not do

**It is a sample, and the rate has a wide interval.** 2 in 72 is not "2.8% of
`deliberate` is false" in any sense that survives being quoted. It is "a few
per cent, probably; tens of verdicts across the bucket, probably".

**Fifty-eight of the seventy-two were read for plausibility, not verified.**
Only the twelve in the table were checked mechanically. The rest are judgement
calls whose `by` is an argument — *"a section number pointing at the same
argument is prose against prose"* — and reading one and finding it still
reasonable is weaker evidence than the table's. A false verdict in that
majority would very likely have been missed, so the real rate is a lower bound
rather than an estimate.

**It does not change how `deliberate` is maintained.** No `checked` field, no
sweep, no guard. The bucket is exactly as unreviewed tomorrow as it was
yesterday, minus the seventy-two read here.

**It says nothing about `closed`.** That bucket has a witness mechanism and its
own audit; this sample did not touch it.

**The sampling script is not committed.** It lives in this session's scratch
directory, so the sample is reproducible only in the sense that the seed and
the method are written down here. The next person re-runs the idea, not the
program.
