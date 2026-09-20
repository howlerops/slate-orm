# The count a refusal did not write

## What changed

`Tally` in `crates/slate-server/src/session.rs` gained `applied(kind, table,
rows, ok)`, and all seven write arms of `apply` now go through it instead of
calling `add` under each arm's own private rule. The rule it holds is:

> A statement contributes unless it *both* failed and applied nothing.

The insert arm also stopped passing `rows.len()` where the applied count
belongs — `insert_many` validates the whole batch before writing any of it, so
a refused batch applied zero, not the rows it named.

`crates/slate-server/tests/transaction_counts.rs` gained four tests, one per
clause per arm that could get it wrong: a refused insert, a refused
all-or-nothing update, a refused conditional delete, and a conditional update
refused *after* applying a row.

## Why

Found by re-reading my own file from task R1 rather than by any failure.
`insert` skipped the tally when the statement failed; `update` recorded a zero.
Two arms of one enum answering the same question differently, and — the part
that made it worth chasing — a probe that made insert record unconditionally
left **all 22 tests then in `transaction_counts` and `observing` passing**. The
disagreement was not merely undetected, it was undetectable by anything in the
repository.

The standalone path had already decided the question and written down why:
`purge_wire.rs::a_refused_purge_reports_nothing` says a counter that moved on a
refusal would make a permission problem look like data loss. So `update`'s zero
was the wrong one, and `insert`'s skip was accidentally right.

The second clause is the one that is not obvious, and is why the rule is not
"count successes": the conditional update loop applies rows one at a time and
stops at the first refusal, so a refused statement can leave rows in the
transaction's buffer that the caller then commits. Dropping those under-reports
a write that landed.

## Alternatives rejected

**"Count successes" — `if ok { add(...) }`.** One clause, easy to state, and
wrong for the conditional update and the conditional delete, both of which
apply a prefix and then fail. It would silently under-report exactly the
partial write an operator most wants to see. Cost: a metric that is correct for
five arms and quietly low for two, which is worse than one that is wrong
everywhere, because nobody would look.

**"Always count" — drop the `ok` argument.** Also one clause, and it makes a
refusal indistinguishable from a write of zero rows. The standalone path
rejected this on the same grounds three tasks ago; taking it here would have
made the two paths disagree, which is the thing this change exists to stop.

**Leave the arms as they were and just write a test pinning each one's current
behaviour.** Cheaper, and it would have frozen the inconsistency into the test
suite rather than removing it. The counter is one series per (statement, table)
and an operator reads `insert` and `update` off the same dashboard; "a refusal
costs you nothing on insert and a zero on update" is not a rule anyone can hold.

**Make `applied` take a `Result<u64>` instead of `(u64, bool)`,** so the
applied count and the outcome cannot be passed inconsistently. This would have
caught the insert bug below at compile time. Rejected because the seven arms
hold their outcome in four different shapes (`Result<()>`, `Result<u64>`,
`Result<Vec<Row>>`, and a loop-accumulated `Result<()>` beside a separate
counter) and three of them would have had to build a `Result` purely to
destructure it again. Recorded rather than dismissed: if an eighth arm arrives,
this is the change to make.

## Evidence

**The disagreement was unpinned.** Before any fix, making the insert arm record
unconditionally: `transaction_counts` 9 passed, `observing` 13 passed, 22 of 22.

**The fix's first version was wrong, and the new test caught it.**
`tally.applied("insert", …, affected, outcome.is_ok())` passed the *requested*
count, so a refused batch of two contributed two. `insert_many`'s all-or-nothing
claim was in a comment, so I checked it rather than repeating it: inserting
`[2, 1]` into a transaction where key 1 is taken, then committing, leaves key 2
absent on a `Get` — the batch wrote nothing. That check is now an assertion in
the test.

**A mutation survived, and the test was rewritten rather than the mutation
hidden.** The first draft of
`a_statement_that_failed_having_applied_nothing_contributes_no_series` put a
successful insert of key 1 in front of the refusal and expected a total of one.
Dropping the `ok` clause from `applied` — `if rows > 0` → `{ }`, i.e. record
everything — left all 11 tests green, because the tally sums by (statement,
table) and a zero folded into a one is still one. A series that must not exist
cannot be tested where another series can absorb it. The test now stands the
refusal alone and asserts the observer was told nothing new.

**Eight mutations, each caught by a named test** (restored and re-verified
after each):

| mutation | test that failed |
| --- | --- |
| `if ok \|\| rows > 0` → `if rows > 0` | `a_statement_that_matched_nothing_still_reports_its_zero` |
| `if ok \|\| rows > 0` → `if ok` | `a_statement_that_failed_having_applied_some_contributes_those` |
| `if ok \|\| rows > 0` → unconditional | `a_statement_that_failed_having_applied_nothing_contributes_no_series` |
| insert passes `affected`, not the applied count | `a_statement_that_failed_having_applied_nothing_contributes_no_series` |
| conditional update reports `0` instead of `conditional` | `a_statement_that_failed_having_applied_some_contributes_those` |
| conditional update reports `affected` instead of `conditional` | `a_statement_that_failed_having_applied_some_contributes_those` |
| `update` back to the unconditional `add` | `a_refused_update_that_applied_nothing_contributes_no_series` |
| `delete` back to the unconditional `add` | `a_refused_conditional_delete_that_applied_nothing_contributes_no_series` |

The last two are why there are four new tests and not two: with only the insert
test, both survived. One arm's test says nothing about another arm, which is
the same blindness that let them disagree to begin with.

**Suites:** `cargo test -p slate-server --no-fail-fast` — 24 test binaries, all
ok, none failed. `cargo test -p slate-serverd --test observing` — 13 ok.
`sh scripts/check.sh` — 19 of 19.

## What this does not do

- ~~**The plain (non-conditional) delete arm's "failed having applied nothing"
  is still unpinned.**~~ **Closed** — see
  `2026-09-20-the-child-that-made-the-arm-reachable.md`. A child table declared
  `ON DELETE RESTRICT` reaches it, which turned out to cost one new table rather
  than the shared-fixture change estimated below. The original text stands
  unedited from here on, because the estimate being wrong is the useful part.

- **The plain (non-conditional) delete arm's "failed having applied nothing" is
  still unpinned**, and the two routes I tried are now written beside the tests
  rather than left as an absence. An absent key answers `Ok(false)` by design,
  so the loop does not fail. An unauthorized delete is refused *before* the
  session task is dispatched to — measured, not assumed: a principal holding a
  role with no grant on `docs` gets `PermissionDenied`, and mutating this arm
  back to the unconditional `add` leaves that case green, which is how I found
  out the arm is never entered. What remains is a real kernel error mid-loop
  (a foreign-key restriction, a storage failure, a fence); this fixture declares
  no foreign keys, and adding one to a `TableDef` that every test in the crate
  shares is a larger change than the hole is worth. The conditional branch's
  test covers the identical call two lines away. Named because it is a hole, not
  because it is covered.
- **Nothing about the counts an atomic batch reports.** That path collects
  through `Decoded::apply` and reports after `Ok`, and has its own tests; it
  does not go through `Tally::applied`.
- **No measurement.** This is a correctness change to a counter; there is no
  number to report and nothing in it should be measurable in latency.
- **The `Result`-typed signature above is not implemented.** It would make the
  insert-arm mistake a compile error rather than a test failure, which is the
  stronger guarantee; the shape mismatch across the seven arms is the only
  reason it is written down instead of written.
