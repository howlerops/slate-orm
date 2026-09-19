# The rows counter is read off a real socket now, and the gap two entries claimed was broader than the truth

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `crates/slate-serverd/tests/observing.rs`
- **Kind:** fix

## What changed

One test. `a_scrape_reports_the_rows_a_write_touched` starts the real binary
with a metrics port, does a standalone insert, opens a transaction and inserts
two more, scrapes `/metrics` *before* committing, commits, and scrapes again.
The first reading is 1 and the second is 3.

## Why

Two entries in a row said "nothing scrapes it in a test", and both were wrong.
`observing.rs` has scraped `/metrics` over a real TCP socket from a real
process since the endpoint shipped — for `slate_requests_total`,
`slate_request_failures_total` and the latency histogram. I read the first
entry's gap paragraph, repeated it into the second, and did not look.

What was true underneath is narrower and was worth closing: the *rows* counter
had no end-to-end coverage, and neither did the line in `serve.rs` that
attaches the `WriteObserver` to the head. The observer had a recording double,
`Counters::prometheus` had its own tests, and the line joining them had
nothing — which is exactly the failure the `observing.rs` module docstring was
written about, one counter over.

The mid-transaction scrape is the part that makes this more than a wiring
check. "Reported on commit and nowhere else" is the rule the previous entry
argued for at length, and until now it was asserted only against an in-process
recording double. Reading 1 off a socket while three rows sit in a
transaction's buffer is that rule observed from where an operator stands.

## Alternatives rejected

**Assert only the total after the commit.** Three, from a socket, wiring
proved. It would also pass if the transaction's rows were counted the moment
they were written, which is the mutation the previous entry could not reach
from outside the process and the one an operator would actually be misled by.
Two scrapes cost one line.

**Extend the existing scrape test rather than adding one.** Tempting — the
setup is identical — and it would make one test assert two unrelated things,
so a failure would need reading to attribute. The existing one is about the
request layer; this is about the write observer; they share `scrape` and
nothing else.

**A unit test that calls `serve.rs`'s builder and inspects the head.** Faster,
and it would pin the line without a process. It also could not see the commit
rule, and the whole argument for `observing.rs` existing is that the wiring is
what the unit tests cannot reach.

## Evidence

Two mutations:

| mutation | result |
| --- | --- |
| drop `head.observing_writes(...)` from `serve.rs` | caught — the new test alone, out of eleven |
| report a transaction's tally after every command rather than on commit | caught — the *mid-transaction* assertion, at line 258 |

The second is the one the previous entry could not make from inside the
process. Its in-process version of this mutation was caught by six tests; this
catches it from the other side of a socket, which is where the claim "a
rolled-back transaction contributes nothing" is actually made to an operator.

Eleven tests in `observing.rs` pass unmutated.

## What this does not do

~~**A rolled-back transaction is not scraped.**~~ **Closed.** It was left out
because the test was already the longest in the file, which this entry itself
called "a preference, not a reason". It is its own test now:
`a_rolled_back_transactions_rows_never_reach_the_scrape`, which asserts the
whole `slate_rows_written_total` family is *absent* rather than zero.

~~**`purge_deleted` is still not scraped.**~~ **Closed**, and the hypothesis
this paragraph labelled — "the statement label travels the same path as
`insert`" — is now a test rather than a label. The harness fixture grew a
second table with a soft delete, kept separate from `docs` rather than widening
every row literal in the file.

Both in `2026-09-19-the-two-things-i-left-out.md`.

**The two claims this corrects were in ledger entries, not in code.** Nothing
was broken by them; what they cost is a reader believing a gap existed where it
did not, and me writing a test I might have thought was bigger than it was.
Both are struck through where they stand.
