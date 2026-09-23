# Two of three groupers had a ceiling nobody had tested, and the defaults had none

- **Date:** 2026-09-20
- **Author:** Claude, probing findings 7 and 8 after saying the last sweep had only read them
- **Touches:** `crates/slate-kernel/tests/security_probe_resources.rs`, `docs/security-review.md`
- **Kind:** fix

## What changed

Three tests. The `max_groups` ceiling is now asserted on the `Grouper` a join
builds and on the one a chain builds, not only the single-table one; and the
shipped defaults are asserted to be finite. Three stale doc comments in the
same file, left behind when finding 7's fix landed, corrected. No production
code — the ceilings were already enforced everywhere. This is coverage.

## Why

The previous entry said: *"It does not probe findings 7 and 8... Finding 6
looked single-path too, from reading."* This is that probe, and it produced one
real gap, one smaller-than-claimed gap, and a hypothesis I had to withdraw.

**The withdrawal first, because it shaped everything after.** I mutated
`ExecutionLimits::new_default` to return `unbounded()` — removing all three of
finding 7's ceilings — and it **survived all 55 kernel suites**. I wrote that
up as "the fix for finding 7 is entirely unexercised" and started writing tests
for all three ceilings. That claim was wrong: a `mod limits` block in the same
probe file already asserts every one of them, with the error variant, the limit
it names, and the `LIMIT`-uses-the-bounded-heap mitigation. Three of the five
tests I had written were duplicates and are gone.

What the surviving mutation actually shows is narrower and still worth a test:
**every one of those tests builds its store with `with_limits(...)`, so none of
them touches the defaults.** A change making `new_default` unbounded ships a
node with no ceilings and every suite stays green. That is one test now.

**The real gap was about reach, not enforcement.** `SecuredReads` builds three
`Grouper`s — one per input shape — and the `limits` module exercises the
single-table one. The other two were read and judged fine. That is the same
standard that missed `TokenIdentity` one commit ago, where a fix covered one of
two implementations of a trait, and the standard that left finding 1's refusal
covering one catalog constructor of two the commit before. Three in a row is a
pattern, so the join and chain paths are asserted now. Both pass; the ceilings
were reaching them all along.

**`SELECT DISTINCT` turned out to need nothing**, which is the one pleasant
result. It is not an operator in the SQL front end — it compiles to a
`GROUP BY` over exactly the selected columns, a decision taken for a different
reason (*"a second way to say the same thing, with its own planning and its own
bugs"*) — so it is under `max_groups` for free. A fourth unbounded accumulator
would have been a genuine finding; this is a design choice that paid off
somewhere its author was not aiming.

## Alternatives rejected

**Leave the defaults untested and trust the constants.** They are `const`s with
documented values, so what is there to break? The mutation answers it: nothing
*reads* them in a test, so a refactor that stops routing them into
`RecordStore::new` is invisible. The test asserts finite and non-zero rather
than the exact numbers, because the constants are documented as untuned and
pinning them would make a deployment's tuning a test failure.

**Assert the exact default values.** Stronger and wrong for the reason above.
The invariant is "there is a ceiling", not "the ceiling is 1,000,000".

**Add a fourth ceiling for `SELECT DISTINCT`.** Reached for before reading how
it compiles. It would have been dead code guarding a path that already has a
ceiling, plus a second number to keep consistent with `max_groups`.

**Write the join and chain tests as an oracle against the single-table one.**
The repository prefers an oracle, and here it would test the wrong thing: the
question is not whether the three paths agree on results, which
`grouped_join_oracle` and `grouped_chain_oracle` already cover at length, but
whether each consults the configured limit. That is a property of one call site
each, and a hand-written case per site is the honest shape.

**Keep the three duplicate tests anyway, since they pass.** They assert what
the `limits` module already asserts, and a second test of the same thing makes
the suite slower to run and the coverage harder to reason about — the next
person deleting one has to check whether the other is really identical.

## Evidence

**The mutation that started it**, and the corrected reading of it:

```
$ mutate new_default -> unbounded(), over the whole kernel
baseline: 55 suites reported, none failing
  !!   every default limit becomes unlimited: SURVIVED (55 suites ran)
```

Now caught:

```
ok  the shipped defaults become unbounded      -> the_default_limits_are_not_unbounded
ok  the group ceiling is raised out of reach   -> the_default_limits_are_not_unbounded
```

**Each new path test is load-bearing**, shown by breaking the site it covers
rather than by it passing:

```
ok  the grouped-join Grouper stops taking the store's limits
      -> a_grouped_join_past_the_ceiling_is_refused_too
ok  the grouped-chain Grouper stops taking the store's limits
      -> a_grouped_chain_past_the_ceiling_is_refused_too
```

Swapping `self.limits` for `ExecutionLimits::unbounded()` in either call site
fails exactly the test written for it and nothing else — which also says the
two tests are not covering for each other.

`cargo test -p slate-kernel --no-fail-fast` green.
`cargo clippy -p slate-kernel --all-targets` clean.
`cargo fmt --all -- --check` clean. `scripts/check.sh` 23/23, including the
cited-test guard over the review's new test names.

**Three stale comments, corrected.** The probe file's module header still said
*"A join has `DEFAULT_BUILD_LIMIT`... Nothing else that holds unbounded state
per request has an equivalent"*, and two test comments said *"with no cap"* and
*"nothing caps that"*. All three were true when written and were falsified by
the fix they were written to motivate. The header now says what the file's
three kinds of test are for, which is what the next person adding one needs.

## What this does not do

**Finding 8 is still only read.** Schema disclosure to an authenticated caller
with no grant: I did not probe it, and this entry is the second in a row to
say so. The honest reading is that I have now twice deferred it while doing the
adjacent work, not that it is low risk.

**The three ceilings are the three that were found.** Nothing here enumerates
what else holds per-request state. `Limits::max_transactions` bounds open
transactions; the join has its build limit; beyond that I checked `Grouper`,
`Accumulator::Distinct` and the sort buffer because the review named them, and
found no fourth by search rather than by reading every accumulator.

**Nothing tests the daemon's `max_concurrent_requests` or `request_timeout`.**
They are deliberately unset by default, so there is no default to assert, and
I did not check that setting them in TOML actually takes effect end to end.
That is a different kind of test — a live server — and it is not here.

**The join and chain tests use one join type and one step count.** An inner
join and a one-step chain. Whether an outer join or a three-step chain routes
through a different `Grouper` construction I did not check; there is only one
`with_limits` call in each function, so I expect not, and expecting is what
this entry is about.

**The `the_default_limits_are_not_unbounded` test does not check the daemon
applies them.** It asserts `ExecutionLimits::default()` is finite.
`slate-serverd` reads its own optional settings and falls back, and that
fallback path is covered by the daemon's own config tests rather than by
anything here — I read that, I did not run it.
