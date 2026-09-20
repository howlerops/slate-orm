# Measuring the two changes that were justified without measuring

- **Date:** 2026-09-20
- **Author:** Claude, running the benchmark an earlier entry said it had not run
- **Touches:** `ledger/` only — no code
- **Kind:** performance

## What changed

Nothing in the tree. A measurement, and a null result.

`ledger/2026-09-20-reviewing-my-own-newest-code.md` hoisted a grant check out of
`write_many`'s per-row loop and inverted `grants`/`authorize` so the question
stopped building an error it discarded. It was explicit that neither was
measured, and named the shape to run:

> If someone wants the number, the shape to run is a wide `upsert_many` as a
> non-superuser, where the old code allocated per row.

Run. There is no difference.

## Why

"Not measured, and here is how you would" is a gap with a tool sitting next to
it, which makes it a gap worth an hour rather than a caveat worth carrying.

And the result matters more than a confirmation would have. The same entry
reasoned:

> the pre-existing `permits_row_with` builds a filter expression per row, which
> is certainly the larger cost and which this does not touch.

That prediction is what the numbers support, and it is the useful half: the
per-row cost on this path is dominated by building an `Expr` tree for every row,
and a grant scan beside it is invisible. Anyone who wants this path faster should
go after the filter build, not the thing I moved.

## Alternatives rejected

**Report the change as a speedup anyway.** The entry was careful not to — "the
hoist moves work that cannot affect an answer; the inversion stops building a
value that is discarded" — and it did say a thousand-row upsert "allocated and
dropped a thousand errors", which invites a reader to assume a cost. This entry
is what stops that assumption hardening into a claim nobody checked.

**Revert the changes, since they buy nothing.** They buy nothing *measurable*
and they are still right: loop-invariant work belongs outside the loop, and a
predicate should not construct an error to throw away. Reverting would trade
clear code for no gain in either direction, and would leave `grants` with a
shape whose only defence was that the cost was too small to see.

**Use `slate-headbench` rather than a scratch test.** The proper home, and it
measures a *deployed* head node — RPC, gRPC codec, storage — which is three
layers of noise over an effect this small. An in-process kernel test with the
allocator as the only variable is the sharper instrument, and it still found
nothing.

**Push to a bigger batch until a difference appears.** It would appear
eventually, and it would be a fact about a batch size nobody sends. 5,000 rows
is already an order of magnitude above a realistic bulk write.

## Evidence

`update_many` over 5,000 rows, as a non-superuser against a catalog with 65
grants — deliberately many, so the scan the hoist removed is not free by
accident. Release build, seven runs each, microseconds:

| shape | runs | min | median | max |
| --- | --- | --- | --- | --- |
| **after** (hoisted, `grants` inverted) | 7 | 5819 | 5967 | 6865 |
| **before** (per-row scan, allocating on "no") | 7 | 5784 | 5949 | 7156 |

The ranges overlap almost entirely and the *before* shape is marginally faster
at both min and median. **That is not a finding that the old code was faster —
it is a finding that the difference is inside run-to-run noise**, which on this
machine is the ~1.3ms spread between each shape's own min and max, twenty times
the gap between the two medians.

**A process note, and the third time today.** The first attempt at the "before"
measurement produced numbers indistinguishable from the baseline — because the
patch had not applied. The anchor string had moved under a `cargo fmt`, the
script's `assert count == 1` fired, and I read the assertion rather than the
numbers. Without it I would have had two runs of identical code and called them
a comparison. Every measurement in this table comes from a patch that printed
`both applied` first.

## What this does not do

**One machine, one shape, one batch size.** No `slate-headbench` run, nothing
deployed, nothing over object storage, no variation in row width or index count.
A reader should take this as "the effect is too small to see here", not as a
bound on every workload.

**It does not measure the `RESTRICT` change** from earlier today, which now
gathers blockers into a `Vec` before refusing rather than returning on the
first. That allocates on a path that previously did not — bounded by the number
of referencing rows, which is the number the read already collected — and is
unmeasured. It only runs when a delete is about to be refused, so it is on the
error path rather than the hot one, which is the argument for not measuring it
and not a measurement.

**It does not reopen the filter build.** Identifying `permits_row_with`'s
per-row `Expr` construction as the dominant cost is the interesting part of this
result and nothing here acts on it. Caching a filter per (context, table,
action) across a batch is the obvious move and it is a change to the security
layer's contract, not a tidy-up.
