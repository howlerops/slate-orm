# `sql.rs` told readers a join takes one GROUP BY key. It had taken a list for days, and nothing in this crate executed the claim either way.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `crates/slate-sql/src/sql.rs`, `crates/slate-sql/tests/front_end.rs`
- **Kind:** a stale doc comment, confirmed by test rather than by reading

## What changed

The grammar block at the top of `sql.rs` said:

```
-- on a join: one GROUP BY key, and ORDER BY needs it
```

The first half is false and the second is true, so the line is now:

```
-- on a join: ORDER BY needs a GROUP BY
```

And two tests, because the reason it went stale is that nothing ran it:

- `a_join_groups_by_more_than_one_key`
- `a_single_table_groups_by_more_than_one_key`

## Why

`JoinSpec::group_by` is a `Vec<u32>`, and its own field comment says the
single-key limit was not a decision — "the single-table path grew a second key
and the joined path was never revisited". That was fixed. The grammar comment
forty lines away was not, and it is the part a reader reaches first.

I found it while triaging the ledger's caveats: three entries record "GROUP BY
on a join still takes one key", and marking them closed meant checking whether
they were. The kernel signature said yes. **A signature is not behaviour**, so
this was recorded as an unconfirmed verdict in
`2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md` until a build was
possible, and this is that confirmation.

## The second test is the interesting one

Mutating the **join** loop to stop after one key was caught. Mutating the
**single-table** loop to do the same thing **survived** `cargo test -p
slate-sql`.

That is not a missing feature, it is a missing test in the right place. The
coverage exists — in `slate-wasm`'s 226 tests, which reach this parser through
`Playground`. That is exactly the coupling this file's own header argues
against: those tests exercise the front end *as the browser happens to call
it*, so a change breaking a non-browser caller is caught only if the browser
cares about the same thing. `slate-serverd` is the second caller now.

So the single-table case is here, in this crate's suite, and the mutation that
survived is caught by name.

## Alternatives rejected

**Delete the line rather than correct it.** The `ORDER BY` half is true and
load-bearing — an ungrouped join cannot be ordered at all — and a reader
scanning the grammar block would then have to find that out from a refusal.

**Leave the comment and mark the caveats closed on the signature.** What I did
first, and recorded as unconfirmed. The whole class of defect here is a claim
nobody executed; closing it with another unexecuted claim would have repeated
it one level up.

**Add the test in `slate-wasm` beside the existing grammar tests.** Where the
226 are, and wrong for this one: the point is that this crate's suite did not
cover it. Adding a 227th browser test would have left the gap exactly where it
was.

**A guard that checks the grammar comment against the parser.** The comment is
prose and the parser is code; matching them means parsing English. What
catches this class cheaply is a test per grammar claim, which is what this adds
two of.

## Evidence

`cargo test -p slate-sql` — 9 tests in `front_end.rs`, 3 suites, none failing.
`cargo fmt -p slate-sql -- --check` clean.

Mutations, via `scripts/mutate.py`:

| mutation | result |
| --- | --- |
| a join's `GROUP BY` stops after the first key | caught — `a_join_groups_by_more_than_one_key` |
| a single table's `GROUP BY` stops after the first key | caught — `a_single_table_groups_by_more_than_one_key` |

The second was run **before** its test existed and survived; that survival is
why the test is here.

**The runs themselves**, as `mutate.py` recorded them:

- `ledger/mutations/20260925T101030-crates-slate-sql-src-sql-rs.json` — crates/slate-sql/src/sql.rs, the run that survived
- `ledger/mutations/20260925T101058-crates-slate-sql-src-sql-rs.json` — crates/slate-sql/src/sql.rs, the join case
- `ledger/mutations/20260925T101302-crates-slate-sql-src-sql-rs.json` — crates/slate-sql/src/sql.rs, the single-table case once its test existed

## What this does not do

**It does not audit the rest of the grammar block.** Eleven other lines make
claims about what the parser accepts, and I checked one. The same reading is
owed to the rest, and the two tests here are a pattern for it rather than
coverage of it.

**It does not check `ORDER BY needs a GROUP BY` either.** The half I left
alone is asserted by prose further down the same file and by a refusal test in
`slate-wasm`, and not by anything in this crate — which is the gap the single
-table test above exists to illustrate, left open one line over.

**No chain case.** `ChainSpec::group_by` is a `Vec<u32>` too and the grammar
block says nothing specific about it, so there was no claim to falsify. Whether
a chain accepts two keys is untested here.

**The `slate-wasm` suite was not run.** It was still compiling when this was
committed, and nothing here depends on it: the point of both tests is that
they do not.
