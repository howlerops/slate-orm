# Probing the two handlers the last entry admitted it had only read

- **Date:** 2026-09-20
- **Author:** Claude, closing two caveats written an hour earlier in the same file
- **Touches:** `crates/slate-server/tests/security_probe.rs`, `crates/slate-server/tests/common/mod.rs`
- **Kind:** fix

## What changed

Two tests and one fixture role. `explain_join` and `explain_aggregate` are
demonstrated to refuse before converting and to name `explain`; a three-input
chain is demonstrated to authorise its third input. No production code — the
four handlers were fixed an hour ago and are correct. This is the part that
said "asserted by reading".

## Why

The previous entry closed with two caveats in its own voice:

> **Only `join` and `aggregate` are probed; their `explain` twins are not.**
> …I asserted that by reading — which is the sentence this entry exists to be
> embarrassed by.

> **The chain handlers were assumed to be the join handlers.** …read from the
> converter, not demonstrated with a three-input request.

Both were honest and both were the same failure the entry was about. Writing
"I read this rather than ran it" in a document does not make it run. The whole
sequence today — seven handlers across three passes — happened because a claim
about untested paths was written down instead of checked, so leaving two more
of them written down would be the fourth instalment.

Both pass. That is a null result for the code and not for the tests: four
mutations on the two explain handlers, including both `Explain`→`Read` swaps,
are now caught where before nothing would have noticed.

## Alternatives rejected

**Leave them; the helpers are shared and the reading was sound.** It was
sound — both handlers do call the same helpers with the right action, and the
probes confirm it. The objection is not that the reading was wrong here, it is
that reading is what was wrong four times today, and the two cases were named
in a ledger entry as known-unchecked. A caveat that is cheap to close and stays
open is a caveat nobody believes.

**Assert only that the explain twins refuse, not which action they name.** The
shorter test, and it would have passed against both. The single-table `explain`
path already showed that swapping `Explain` for `Read` is invisible unless the
action is asserted by name — it survived a mutation until one line was added.
Writing the same test without that line would reproduce a hole this session
already found and closed once.

**Use `reader_only` for the chain probe** — what the first version did, and it
is what makes this entry worth writing. That role reads `users` and nothing
else, so a three-input request is refused at input **1** and never reaches the
third. The test passed, and the `take(2)` mutation **survived** it: the probe
could not reach the code it was about. This is the second vacuous probe today,
after the one-input join refused by arity, and both were caught only because a
mutation was run against them.

**Add no role and self-join `users` to itself** so inputs 0 and 1 are both
readable by `reader_only`. It would work and it tests table aliasing at the
same time as authorisation, so a failure would have two candidate causes. The
fixture's four single-action roles exist for exactly this reason — the comment
beside them says so — and a fifth follows the established pattern.

## Evidence

**The explain twins.** Four mutations, each caught by the one test:

```
ok  explain_join converts before authorising
ok  explain_join authorises Read instead of Explain
ok  explain_aggregate converts before authorising
ok  explain_aggregate authorises Read instead of Explain
```

**The chain, before and after the fixture role.** With `reader_only`:

```
!!  the authorisation loop stops after two inputs: SURVIVED
```

With `two_table_reader` — `Read` on `users` and `docs`, nothing on
`mentions` — the same mutation:

```
ok  the authorisation loop stops after two inputs
      -> the_third_input_of_a_chain_is_authorised_too
```

The test also asserts the in-range refusal **names `mentions`**, which is what
says inputs 0 and 1 were let through rather than the request dying early. Both
of today's vacuous probes would have been caught by a control assertion of that
shape, and both now have one.

`cargo test -p slate-server -p slate-serverd --no-fail-fast` green;
`cargo clippy -p slate-server --all-targets` and `cargo fmt --all -- --check`
clean; `scripts/check.sh` 25/25. The security probe suite is 13 tests.

## What this does not do

**The client suites were not re-run.** This commit adds tests and one grant to
a test-only fixture; no server code changed since the run that reported
`295 passed` and a green Go suite an hour ago. That is a judgement about the
diff, not a run.

**`two_table_reader` is a fifth role in a shared fixture.** It is additive, so
no existing test sees it, and the comment says what it is for. It does add a
grant to the catalog every test in the crate builds, which is a cost I judged
negligible and did not measure.

**Three inputs, not four.** The loop is `for input in &wire.inputs`, and a
three-input case rules out `take(1)` and `take(2)`. It does not rule out a
bound nobody would write, and the test would need a fourth role to try.

**Nothing probes the `explain` twins through a *transaction*.** Both handlers
branch on `request.transaction`, and every probe here takes the empty-string
path. The authorisation runs before that branch, so the branch cannot matter —
which is a reading, stated here rather than in a sentence claiming otherwise.
