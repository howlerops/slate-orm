# The caveat tracker counted 767 and could name none of them. Triaging 677 needed a fifth verdict and a way to print the answer.

- **Date:** 2026-09-25
- **Author:** Claude Code, working #301 (F7b), first instalment
- **Touches:** `scripts/caveats.py`, `docs/caveat-status.json`
- **Kind:** using a tool enough to find what it was missing

## What changed

Two things in `scripts/caveats.py`, both found by doing the triage rather than
by reading the file:

- **`--open`, and one flag per verdict.** The header says the problem it exists
  for is that "asked *what is left to do*, the only honest answer was to read
  196 files". It then printed four counts. To see the 48 open caveats I wrote a
  throwaway Python one-liner against the JSON — which is the same workaround as
  reading the entries, only shorter. A count is not a list.
- **A fifth verdict, `moment`.** 677 caveats do not sort into open / closed /
  deliberate without lying about roughly one in twelve.

And 175 verdicts recorded so far, against a starting 677 untriaged.

## Why `moment` exists

Some caveats were never standing claims. "It does not prove CI is green." "The
`deployed` job's Node and Go steps have now run exactly once each." "The panel
has no second-key control" — written in an entry that says, two lines later,
that the panel no longer exists.

None of these is work. None is a decision with reasoning to preserve. None is
closed by anything, because nothing was ever going to close them; they stopped
being current on their own. `open` would have parked them on a backlog to be
read as work forever, which is the failure mode this tracker exists to prevent
— it would have manufactured debt out of honest reporting, and punished the
entries that were most candid about what a single run had shown.

`by` is not required for a `moment`: the entry's date is the context, and
demanding a citation for "this was true that afternoon" would be theatre.

## Alternatives rejected

**Force them into `deliberate`.** The verdict's own definition is "a decision
with reasoning written down", and `by` must name where the reasoning lives.
"CI was red that morning" is not a decision and there is nowhere to point. It
would also corrupt the one verdict whose count means something: `deliberate` is
how many design choices this project has written down and stands behind.

**Force them into `closed` with `by` naming the entry.** Closer, but `closed`
means *done since*, and a reader filtering for what changed would get a list of
things that never changed. It would also inflate the closed count, which is the
number most likely to be quoted.

**Leave them `untriaged` forever.** Honest about the model failing, and useless:
`untriaged` means "not yet read", so it hides the fact that these *were* read
and found to be nothing. The next person re-reads them.

**Widen `open` and filter in the reader's head.** That is what having no fifth
verdict amounts to. The tracker's whole point is that a human should not have
to hold the ledger's history in their head to know what is left.

## Evidence

`scripts/test_caveats.py` passes unchanged — the existing suite pins the four
original verdicts and the refusals around them, and `moment` was added without
disturbing any of it. `python3 scripts/caveats.py` reports
`767 caveats: 101 open, 67 closed, 89 deliberate, 502 untriaged`, and
`--open`, `--closed`, `--deliberate`, `--moment` each print their list and
count. An unknown flag exits 2 with a usage line.

Every `closed` verdict in this instalment names something that exists now and
was checked: `site/check/quickstarts.py`,
`crates/slate-wasm/tests/bucket_provenance.rs`,
`crates/slate-serverd/src/main.rs:370` calling `verify`,
`decimal_value` in `records.proto`, `Grouping.group: Vec<Ordinal>`,
`clients/go/slate/scalar.go`, `clients/typescript/src/scalar.ts`.

## What this does not do

**502 caveats are still untriaged.** This is the first instalment of #301, not
its completion. The 175 done are the oldest, 2026-09-13 to 2026-09-15.

**No test covers `moment` or the listing.** `test_caveats.py` was not extended.
The listing is exercised only by my having run it; the fifth verdict is
exercised by 12 caveats carrying it and the counts adding up. Both should get a
case in that suite, and a `moment` with a `by` should probably be refused the
way a `closed` without one is — I did not decide that.

**No mutation run.** Same reason as the last entry: 1.2 GB free against a 19 GB
`target/`. The listing and the verdict are Python, so `mutate.py` could reach
them via `test_caveats.py` without a Rust build — that is a real gap in this
entry's evidence rather than a container limit, and I am recording it as one.

**A verdict is my reading, not a proof.** `closed` was checked against the tree
in every case; `deliberate` and `moment` are judgements about what an entry's
own prose means, and a second reader would move some of them. The key is the
bullet's opening text, so a reworded caveat surfaces as an orphan rather than
silently reverting — that is the safety net, and it is the only one.

**`sql.rs:34` may be stale, and I did not confirm it.** Its grammar comment says
"on a join: one GROUP BY key", while `Grouping.group` is a `Vec<Ordinal>` and
`lower.rs` applies no join-specific limit. I marked the caveats that say "one
group key per join" as closed on the strength of the kernel signature and the
completed work that widened it. If the comment is right and the parser still
refuses, those verdicts are wrong. Confirming it needs `cargo test -p slate-sql`,
which needs disk this container does not have.
