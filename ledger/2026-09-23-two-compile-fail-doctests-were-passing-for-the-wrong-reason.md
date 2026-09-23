# Six of the thirteen `compile_fail` doctests defend what they sit beside. Two were passing whether or not the check existed, because the generated code does not compile either way.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #295 (F6o)
- **Touches:** `crates/slate-derive/src/lib.rs`
- **Kind:** using the tool #294 fixed, on the thing #294's entry said it had not checked

## What changed

#294 made libtest's doctest names readable by `mutate.py` and then verified two
doctests. Its own closing paragraph: *"It does not re-verify the repository's
doctests. `slate-orm/src/lib.rs` alone has five `compile_fail` blocks."*
Thirteen, in fact. Eight had a macro check behind them that could be broken.

Six mutations were caught, each naming the doctest that caught it:

```text
ok  belongs_to local names a field that does not exist
      -> crates/slate-orm/src/lib.rs - Record (line 301) - compile fail
ok  has_many without foreign is accepted          -> ... (line 321) - compile fail
ok  belongs_to without local is accepted          -> ... (line 363) - compile fail
ok  has_many on a composite key defaults across tenants -> ... (line 341) - compile fail
ok  two variants storing the same name            -> ... Enum (line 424) - compile fail
ok  a second only_where is accepted               -> ... (line 190) - compile fail
```

**Two survived**, and the reason is the interesting part. Remove the check that
refuses an `Enum` variant carrying data, or the one that refuses an empty
enum, and both doctests still pass — because for those two shapes the
*generated* code does not compile either. A `compile_fail` block asks only
"does this fail to compile", and cannot ask why.

So the safety property was never at risk: both shapes are refused with or
without the check. What was at risk is the **diagnostic**, which is the entire
reason those two checks exist. Their own comment says so: *"A compile error
here beats a `match` with no arms later."* Nothing tested that the better error
actually arrives.

`expand_enum` takes a `DeriveInput` and returns `syn::Result`, so it can be
called directly. Four unit tests now assert what it *says*, not merely that it
refuses.

## Why

A `compile_fail` doctest is a weaker instrument than it reads as. It passes for
any compile error, including one from a typo in the fixture, so it can hold a
claim it never checks. Two of thirteen here were in that state, and the way to
find out was to break the code and watch.

## Alternatives rejected

**Add `trybuild`.** It is the tool for this — it asserts the *text* of a
compile error — but it is a new dependency with committed `.stderr` files that
go stale on every rustc release, and the repository already pins
`grpcio-tools` for exactly that class of problem. Four unit tests on a function
that is already `pub(crate)` and already returns the error cost nothing and
pin the message directly.

**Leave them, since the shapes are refused anyway.** True, and not the point:
the checks were added to turn a confusing error inside generated code into a
sentence naming the mistake. A guard kept for its message, with nothing reading
the message, is a guard that can rot into uselessness while its test stays
green.

**Delete the two checks as redundant.** They are not redundant, they are
*diagnostic*, and the doctests stay because they pin the user-visible
behaviour — that the shape does not compile — while the unit tests pin the
reason. Both halves are worth having and neither substitutes.

**Mutate the remaining five doctests too.** Three have no macro check behind
them at all — line 143 is protected by Rust's own name resolution (a typo'd
column is simply not in scope), and lines 164 and 278 by the *types* the macro
emits. There is no branch to break, so a mutation would test the language
rather than this repository. Said plainly rather than counted as covered.

## Evidence

Five mutations, **four caught and one survived as recorded**, in
[`ledger/mutations/20260923T140240-crates-slate-derive-src-lib-rs.json`](mutations/20260923T140240-crates-slate-derive-src-lib-rs.json).
The survivor mutates the test helper rather than the code under test: a helper
that returns the four expected substrings without calling `expand_enum`
satisfies every assertion, which is true of any suite and proves nothing. It is
in the spec rather than dropped so the record shows it was tried.

The earlier eight-case run over the doctests is
[`20260923T135433`](mutations/20260923T135433-crates-slate-derive-src-lib-rs.json),
where the two survivors first appeared.

`sh scripts/check.sh`: **48 passed, all of them.**

## What this does not do

**Three of my own mutations this session were equivalent code**, not mutations:
`&input.data.clone()`, and `if true { x } else { x }`, both of which match the
same pattern the original does. Each "survived" and meant nothing, and I nearly
wrote the first up as a finding. A mutation that does not change behaviour is
noise in the same shape as a discovery, and `mutate.py` cannot tell them apart —
it checks that the anchor is unique, not that the replacement differs
semantically.

**It does not cover the `Record` doctests outside those eight.** Five remain
unmutated for the reason above, and "no branch to break" is an argument rather
than a test.

**It does not check the message text a user sees for the other refusals.**
`expand_enum` has four diagnostics and now four tests; `expand_record` has more
than twenty and none of this kind.
