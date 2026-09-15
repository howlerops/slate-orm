# `chunks_exact` on a constant, caught by a lint this container does not have

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/orm-essentials`
- **Touches:** `crates/slate-kernel/src/migrate.rs`
- **Kind:** fix

## What changed

Decoding the built-index list out of a schema-state record no longer uses
`chunks_exact(4)`. It walks the bytes with `split_first_chunk::<4>()`.

## Why

CI went red on `cf59918` with fourteen of fifteen jobs green and only
`cargo clippy --workspace --all-targets` failing:

```
error: using `chunks_exact` with a constant chunk size
   --> crates/slate-kernel/src/migrate.rs:386:10
   = note: `-D clippy::chunks-exact-to-as-chunks` implied by `-D warnings`
```

`cargo clippy --workspace --all-targets` was clean here before the push. The
lint does not exist in this container's toolchain, and `RUSTFLAGS=-D warnings`
makes the gap fatal rather than noisy. **`CLAUDE.md` names this lint, by name, as
one of three that have already done this** — which is the part worth recording: a
warning written down after the last occurrence did not prevent the next one,
because nothing mechanical enforces it and the code in question was written
without a thought about chunking at all.

## Alternatives rejected

**Clippy's own suggestion, `as_chunks::<4>().0.iter()`.** Rejected because it
would fix the build in CI and could break it here — the container's toolchain is
older, and taking a suggestion from a lint it does not have is taking a
suggestion about an API it may not have either. That is the same asymmetry that
caused the failure, pointed the other way.

**`#[allow(clippy::chunks_exact_to_as_chunks)]`.** Rejected: the attribute names
a lint this toolchain cannot check, so it silences something invisible here and
would outlive the reason for it. It also does not compile as a *reason* — the
code it defends is not better for having been defended.

**Installing a second toolchain to check before pushing.** Not possible in this
container for the reason `CLAUDE.md` gives: disk. That remains the real gap, and
it is not closed by this commit.

## Evidence

`split_first_chunk` is stable, older than either toolchain, and not the subject
of any lint in question. It is also better code on its own terms and that is why
it was chosen rather than tolerated:

- it yields `[u8; 4]` by value, so the copy into a scratch array goes away;
- what is left over at the end *is* the remainder, so the trailing-byte check
  becomes `!rest.is_empty()` instead of `ids.len() % 4 != 0` — two pieces of
  arithmetic that had to agree with each other, and now do not have to.

`cargo clippy -p slate-kernel --all-targets` is clean here, and the fifteen
migration tests still pass, including `an_unreadable_state_record_is_a_refusal_naming_the_table`
which is the one that exercises this decoder.

The real verification is CI, because the failing lint is the one this container
cannot run. That is stated rather than implied: **this fix is unverified locally
against the lint it exists for.**

## What this does not do

**Nothing prevents the next one.** Three lints have now done this. A guard would
have to either run CI's clippy locally, which the disk does not allow, or encode
a list of lints newer than this toolchain, which is a list nobody will maintain.
The honest mitigation is the one `CLAUDE.md` already gives — run the *workspace*
clippy rather than `-p`, and treat green here as necessary rather than sufficient
— and it was followed, and it was not enough.
