# A 55-second poll produced no lag and a 10-second poll produced plenty; the experiment says the interval governs and the example's null result was a timing artefact

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `crates/slate-slatedb/tests/replica.rs`, `examples/deployed/README.md`
- **Kind:** fix

## What changed

One test, `the_manifest_poll_interval_is_what_a_replica_can_see`, and the
paragraph in `examples/deployed/README.md` that asked for it. No production
code moved.

## Why

Two measurements in this repository disagreed, and the disagreement had been
sitting in a README marked "left open" with two candidate explanations and a
note that the example could not settle it.

- `slate-serverd`'s `process.rs`: a ten-second `manifest_poll_interval` against
  a 250 ms `catch_up` made **64 of 64** read-your-writes reads fall through to
  the writer. Lag, exactly as the interval predicts.
- `examples/deployed`: a **55-second** poll against a 60-second `catch_up` —
  a poll 27 times longer than the one above — produced no observable lag at
  all, on eight unpinned reads at two dataset sizes.

The candidates were: the window is much narrower than the configuration
suggests, or `manifest_poll_interval` no longer governs what a `DbReader` sees.
The second would have meant the daemon's whole `poll_interval`-derives-from-
`catch_up` rule was cargo cult, and the `process.rs` measurement a coincidence
— which is worth an hour to rule out.

## Alternatives rejected

**Leave it open.** It had been, and an open question with two candidates one of
which invalidates a shipped design rule is not a stable place to leave things.
The README even names the experiment; not running it was the only thing
stopping the answer.

**Reproduce it in `examples/deployed`.** Where the question was found, so the
obvious place. It cannot: the example runs one daemon with its own config, and
the experiment needs two readers configured *differently* against one store, at
the same instant, with a write between them. That is a `slate-slatedb` shape.

**Make the lazy reader's interval derive from the test's own sleep**, so the
numbers obviously line up. Rejected because a poll interval tuned to the test's
timing proves the test was tuned. 300 seconds is picked to be absurd against a
two-second window and is not a function of it.

**A longer wait — the full 300 seconds — to be sure.** Rejected: a test that
takes five minutes is a test that gets `#[ignore]`d, and 100× the eager
reader's own catch-up establishes the claim. The assertion that the window is
at least 50× the eager time is what keeps that honest; see below.

## Evidence

**The answer: the interval governs.** Two readers, one object store, differing
in nothing but `manifest_poll_interval`, both opened after a first commit so
neither is racing its own startup:

| reader | `manifest_poll_interval` | sees the second commit |
| --- | --- | --- |
| eager | 20 ms | 7.827, 8.705, 8.915, 9.816, 11.164 ms (five runs) |
| lazy | 300 s | not after 2 s; still at its prior sequence |

The lazy reader also reads the *first* row and not the second, which is what a
stale replica means — asserted, because a sequence that stalled while the data
arrived anyway would be a different and much worse bug.

**So the deployed example's null result is a fact about that example.** Its
unpinned reads happen after a load of tens of thousands of rows, by which point
a 55-second interval has fired many times over. The README's second explanation
is withdrawn and the first stands, narrower than it was stated: it is not a
window, it is elapsed time since that reader's last poll.

**Two mutations, and the second is the one worth having:**

| mutation | outcome |
| --- | --- |
| the lazy reader polls at 20 ms like the eager one | killed |
| the lazy reader is given a budget of **zero** | **survived** |

The second survivor is the more interesting result in this entry. A reader that
cannot advance also cannot advance in no time at all, so "it did not move"
was true for a reason that had nothing to do with the poll interval — the test
would have passed with the sleep deleted. It now asserts that the window is at
least fifty times the eager reader's own catch-up before concluding anything
from its silence, and that assertion kills the mutation with the message *"the
window is 0ns against an eager catch-up of 9.95ms, which is not enough for its
absence to be evidence of anything"*.

**An incidental finding, recorded where it will be met.** SlateDB refuses a
`checkpoint_lifetime` under twice the `manifest_poll_interval`, and reports the
interval *doubled* when it does: a 600-second interval with a 900-second
lifetime is refused as `lifetime=900s, interval=1200s`. Confusing enough to
cost a few minutes, so there is a comment beside the options.

`cargo test -p slate-slatedb --test replica`: 7 passed.

## What this does not do

- **It does not measure the interval's edge.** The claim is "300 seconds holds
  a reader back for at least two", not "a reader advances at exactly its
  interval". Whether a poll fires on schedule, early, or late under load is a
  different experiment and one this does not attempt.
- **`InMemory` object store.** So it says nothing about whether a real S3's
  latency, caching or consistency changes the picture. It does not need to: the
  hypothesis under test was about the *reader's* polling, and a store that
  answers instantly is the cleanest way to isolate it.
- **The `process.rs` measurement was not re-run.** It is the one this agrees
  with, and re-running it would have been confirming the side that was never in
  doubt.
- **Nothing in `slate-serverd` changed.** `poll_interval` still derives from
  `catch_up` and a poll at or above it is still refused — which this now
  supports rather than merely asserting.
