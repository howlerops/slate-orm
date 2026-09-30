# Ten runs on one machine is not a property

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `crates/slate-serverd/tests/ceilings.rs`,
  `crates/slate-serverd/src/config.rs`, `docs/security-review.md`,
  `site/docs/limits.html`, three ledger entries, `docs/caveat-status.json`
- **Kind:** fix

## What changed

`a_zero_request_timeout_does_not_cancel_a_query_answered_at_once` is deleted.
It asserted, from ten runs on this container, that `request_timeout = "0ms"`
leaves a two-row in-memory query answered. **CI run 443 failed it on the first
try** with `Cancelled: Timeout expired` — same test, same configuration, a
different machine.

In its place, `a_generous_request_timeout_serves_the_query`: a 60-second
timeout serves, which cannot go either way. Together with the existing
no-timeout control it shows the setting is read and applied, which is all that
can be asserted without asserting a race.

The claim drawn from those ten runs was published in six places and is
corrected in all six: the test's docstring, the `request_timeout` doc comment
in `crates/slate-serverd/src/config.rs`, the remedy paragraph in
`docs/security-review.md`, the limits page, and three ledger entries from
earlier today, each struck or annotated in place rather than rewritten.

## Why

The mechanism was right and the conclusion was too strong. `GrpcTimeout::poll`
does poll the inner future before the sleep, so a handler that returns on its
*first* poll cannot be cancelled. What I wrote was "a handler that returns
without ever pending is never cancelled", and then treated a fast in-memory
query as an instance of that — which it is not. Whether that query's future
pends once, on a socket read that is not ready yet, is the scheduler's
business. On this container it did not, ten times. On a CI runner it did.

So the honest statement is worse for a user than the one I published, not
better. I said `request_timeout` is a weaker bound than its name suggests but
harmless to set. It is not harmless: a value below the time a request spends
waiting will cancel it sometimes and not others, on the same code, depending on
the machine. A deployment that sets a small value to be careful is buying a
flake. That belongs on the limits page in those words, and now is.

The process failure is the one `CLAUDE.md` names twice. "Measure, do not
assert" was followed. "Report spread, not a single number" was not: five out of
five and five out of five is a *rate*, reported as though it were a fact, from
one machine. The variance that mattered was between machines and the experiment
had no second machine in it. CI was the second machine, and it disagreed on its
first sample — which is the strongest possible evidence that ten was not many.

There is a second lesson and it is about what I pinned. The deleted test's own
comment said it existed "because if tonic ever swaps those two polls this test
goes red and somebody reads this comment". That is a good reason to pin a
surprising behaviour and a bad reason to pin a *nondeterministic* one: the test
would have gone red for the tonic change and also for a busy runner, and the
second reading would have trained whoever met it to re-run CI. A test that
fails for two unrelated reasons teaches people to ignore one of them.

## Alternatives rejected

**Loosen the assertion to "either outcome is acceptable".** It compiles, never
fails, and tests nothing — the definition of a check that cannot fire, which
this repository has a paragraph about. Worse than deleting it, because it looks
like coverage.

**Run the zero-timeout case a hundred times and assert a rate.** A rate needs a
population and the population here is *machines*, not runs: a hundred runs on
this container would report 100/100 and be exactly as wrong as ten. There is no
threshold that is right on both machines because the behaviour is not a
frequency, it is a scheduling dependency.

**Keep the test and mark it `#[ignore]`.** It would stay in the tree as a
record of the surprise while never running. Rejected because an ignored test is
a comment with a compiler attached, and the comment is better written where the
setting is configured — which is where it now is, at length, in `config.rs`.

**Assert the CI behaviour instead** — that `0ms` *does* cancel. Symmetrically
wrong, and it would go red here, five times out of five.

**Retry the CI job and see if it passes.** It probably would have, sometimes,
and that is the trap. `CLAUDE.md`: "'Flake' is not a root cause." The failure
named a real disagreement between two machines about a claim I had written into
a docs page; re-running until green would have kept the wrong page.

## Evidence

- CI run 443 on `aaeb5e7`, job `clippy, test`:
  `a_zero_request_timeout_does_not_cancel_a_query_answered_at_once` FAILED,
  `Status { code: Cancelled, message: "Timeout expired" }` at
  `crates/slate-serverd/tests/ceilings.rs:122`. The other four cases in the
  file passed there, and 197 `slate-serverd` unit tests passed alongside, so
  the failure is that assertion and not the fixture.
- This container, before the deletion: `0ms` answered 5/5 and `1ms` answered
  5/5, recorded in
  `ledger/2026-09-28-a-request-timeout-does-not-bound-a-fast-request.md`. Two
  machines, opposite answers, no change in between — which is the whole
  finding.
- `cargo test -p slate-serverd --test ceilings`: 5 passed, 0 failed, with the
  generous-timeout case in place of the deleted one.
- `sh scripts/check.sh`: 72 passed, all of them.

## What this does not do

**It does not establish when the handler pends.** The difference between the
two machines is real and unexplained: it could be core count, tokio's scheduler
under load, socket readiness, or the runner's timer resolution. Knowing which
would say whether a *small but nonzero* timeout is safe on a machine of a given
shape, and nothing here investigates it. The advice on the limits page is
therefore conservative by necessity — "set it generously or not at all" — and
not derived from a model of when the race is lost.

**It does not check the other measurements from this session for the same
weakness.** `crates/slate-orm/tests/read_counts.rs`, committed an hour ago,
counts calls rather than timing anything, so it has no race of this kind; the
wasm timing test has its own long history of exactly this problem and its own
batching fix. Whether any *other* single-machine number in the ledger would
reverse on a second machine is unexamined, and after today it is a question
worth asking of every one of them.

**It leaves `max_concurrent_requests` where it was.** The per-connection claim
about it is a reading of the tonic method's name and was never a timing
measurement, so this finding does not touch it — but it is now the only
unmeasured half of that pair, and the reason it is unmeasured (no handler that
blocks on command) is unchanged.
