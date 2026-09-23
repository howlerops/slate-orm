# Every doctest in this repository was invisible to the mutation runner, and one of the two cross-tenant refusals in `#[derive(Record)]` looked untested because of it.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #294 (F6n)
- **Touches:** `scripts/mutate.py`, `scripts/test_mutate.py`
- **Kind:** a one-line pattern fix, found by using the tool, and the audit it then made possible

## What changed

`mutate.py`'s `rust` dialect found a failing test by `^test (\S+) \.\.\. FAILED$`.
libtest gives a test four name shapes and **two of them contain spaces**:

```text
test a_unit_test ... FAILED
test a_unit_test - should panic ... FAILED
test crates/slate-orm/src/lib.rs - Enum (line 387) ... FAILED
test crates/slate-orm/src/factory.rs - factory (line 15) - compile ... FAILED
```

`(\S+)` matches the first only. So a mutation caught by a `should_panic` test,
or **by any doctest at all**, came back `UNREADABLE` — which reads as "your
command is wrong" and sends the next person to fix a command that was already
right. `(.+?)` reads all four.

## Why

Found twice by using the tool, an hour apart. First in #293, mutating an
assertion whose only witness was a `should_panic` test. Then, auditing whether
that blind spot had hidden anything, on this:

```rust
// prefix that matches rows from every other tenant.
let keys = fields.iter().filter(|f| f.primary_key).count();
if keys != 1 {
```

Replacing that with `if false` **survived** — so `#[derive(Record)]` would
default a `has_many` local column to the first column of a composite primary
key, relating across every tenant, and nothing would notice.

It is not true. A `compile_fail` doctest at `lib.rs:341` covers exactly that
case, and my mutation command ran `--test derive_relations`, which does not run
doctests. Under `--doc` the mutation is caught. But the first version of this
entry said the guard was untested, and it took re-reading the file to find the
doctest that had been there since #149.

That near-miss is the argument for the fix. A runner that cannot name what
caught a mutation will eventually be believed when it says nothing did.

## Alternatives rejected

**Match `(\S+)` plus an alternation of the known suffixes.** The first fix, and
too narrow by exactly the amount that caused the problem: it handled
` - should panic` and would still have been blind to every doctest. Widening
the *capture* rather than enumerating *suffixes* is what makes the next shape —
libtest's, not mine to control — arrive readable.

**Keep the suffix in the reported name.** ` - should panic` is stripped and
` - compile fail` is not, because the name exists so a reader can re-run the
test: `cargo test a_unit_test` works and `cargo test "a_unit_test - should
panic"` does not, while a doctest's name is its location either way.

**Write the missing test for the `has_many` refusal.** There was no missing
test. Writing one would have added a second copy of a rule already covered and
recorded a defect that did not exist.

**Audit the past runs this blind spot could have spoiled.** Attempted and
largely impossible, which is #287's finding standing where it was left: records
begin on 2026-09-22 and nothing before that left a trace. What could be checked
was: no existing record touches a file with a `should_panic` test, and none of
the six ledger entries carrying an `expect_survivor` records one on
doctest-covered or `should_panic`-covered code — they are a partition-sharing
cost, an unreachable builder branch, and the `-q` defect #285 already fixed. So
no *recorded* survivor is a false one from this cause. Runs before the records
existed cannot be spoken for.

**Verify by reading libtest's source.** Every one of the four shapes was taken
off a real run: the first two from `rustc --test` on a two-test file, the last
two from `cargo test -p slate-orm --doc`.

## Evidence

Two new cases in `test_mutate.py`, **28 passing**, one per unreadable shape,
each asserting both that the mutation is scored a catch and that `UNREADABLE`
does not appear.

Three mutations, **all caught**, in
[`ledger/mutations/20260923T134628-scripts-mutate-py.json`](mutations/20260923T134628-scripts-mutate-py.json):
narrowing the capture back to `(\S+)`, swallowing the should-panic suffix into
the name, and emptying the report pattern.

End to end, on the thing that started it — both cross-tenant refusals in
`#[derive(Record)]`, mutated under `cargo test -p slate-orm --doc`:

```text
ok  has_many defaults its local column on a composite key, matching across tenants
      ->  crates/slate-orm/src/lib.rs - Record (line 341) - compile fail
ok  belongs_to defaults its foreign column on a composite key
      ->  crates/slate-orm/src/lib.rs - Record (line 240)
```

Both were always defended. Before this, neither could be shown to be.

`sh scripts/check.sh`: **48 passed, all of them.**

## What this does not do

**It does not re-verify the repository's doctests.** It makes them *readable*
by the runner; whether each one actually defends what it sits beside is a
mutation run per doctest, and this ran two. `slate-orm/src/lib.rs` alone has
five `compile_fail` blocks.

**It does not clear the history.** Any mutation run before 2026-09-22 that was
caught only by a doctest scored as a survivor — silently, before #285 added the
UNREADABLE signal — and there is no record of those runs to check. If a test
was written, or an `expect_survivor` recorded, against such a result, it is
still there and this cannot find it.

**The four shapes are the four I found.** libtest has others I did not provoke —
`ignored`, benchmark output, `--format json`. The capture is wide enough that a
new shape is likely to fall in, which is an argument about the pattern rather
than a test of it.
