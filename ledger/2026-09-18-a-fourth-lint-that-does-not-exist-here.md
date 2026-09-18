# CI's clippy rejected a borrow in a format argument, in a lint this container does not have — the fourth of that class, and the first from a scripted edit

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `crates/slate-kernel/tests/decimal_arithmetic.rs`, `CLAUDE.md`
- **Kind:** fix

## What changed

Two `&` removed, and `CLAUDE.md`'s note about the clippy version gap gained
the third lint on its list and a second paragraph about the edit method.

## Why

CI run 137 went red on `main` with 17 of 18 jobs green. The one failure:

```
error: redundant reference in `assert!` argument
   --> crates/slate-kernel/tests/decimal_arithmetic.rs:530:13
    |
530 |             &row.computed
    |             ^^^^^^^^^^^^^ help: remove the redundant `&`
    = note: `-D clippy::useless-borrows-in-formatting` implied by `-D warnings`
```

`cargo clippy --workspace --all-targets` had been run here and was clean,
because the lint does not exist in this container's clippy (0.1.94,
2026-03-25): asking for it explicitly gives `unknown lint:
clippy::useless_borrows_in_formatting`. `CLAUDE.md` already warns about
exactly this, naming two other lints it happened to with; this is the third.

**The more useful half is how the `&` got there.** The test was written against
a guessed API — `row.computed()` — and when the compiler said `computed` is a
field rather than a method, a one-line script replaced `row.computed()` with
`&row.computed` everywhere. That is right inside the `matches!` and wrong in
the format argument two lines below, and nothing local said so. A mechanical
replacement that is correct in most positions is not correct in all of them,
and re-reading the script's output would have caught it in the same second it
took to write.

## Alternatives rejected

**Pin the CI toolchain to this container's.** Removes the whole class, and is
the wrong trade: `dtolnay/rust-toolchain@stable` is how new lints get found at
all, and three of the four they have found were real (this one included — the
borrow does nothing). Pinning would trade a red CI for code that is slightly
worse forever.

**Install a second toolchain locally to check against.** `CLAUDE.md` already
says this is usually not possible here, and the disk note says why; this
session hit ENOSPC four times without a second toolchain in it.

**Add `#[allow(clippy::useless_borrows_in_formatting)]` to the test file.**
Would not even work: the lint does not exist locally, so the allow is an
unknown-lint warning here and a suppression there. The same shape as the `# ty:
ignore` that could not be right in both environments, written up two commits
ago — a suppression whose correctness depends on which machine reads it is
worse than none.

**Say nothing and just fix it.** The note in `CLAUDE.md` gets its force from
the list of real examples, and a fourth instance that is not on the list makes
the list look historical. The edit-method paragraph is new information rather
than a restatement, which is the test for whether an addition earns its space.

## Evidence

- The failing job: run 137, `clippy, test`, `cargo clippy --workspace
  --all-targets`, exit 101. Every other job in that run was green, including
  the full Rust test suite's own compile of the same file — the lint is the
  only thing that objected.
- `cargo clippy --version` here: `clippy 0.1.94 (e408947bfd 2026-03-25)`.
- `cargo clippy -p slate-kernel --all-targets -- -W
  clippy::useless_borrows_in_formatting` here: `warning[E0602]: unknown lint`.
  That is the check that distinguishes "this lint exists and is quiet" from
  "this lint does not exist", and it is worth doing before concluding a local
  clippy has cleared anything.
- After the fix: `cargo test -p slate-kernel --test decimal_arithmetic`, 26
  passed. The two new files from the same session — `decimal_text.rs`,
  `decimal_wire.rs` — were grepped for the same pattern and have none.

## What this does not do

- **Nothing catches the next one.** There is no local check for a lint that is
  not installed, and this entry does not add one. The only thing that changed
  is that the note says to verify a lint *exists* before treating its silence
  as a pass.
- **The other three crates' test code was not audited** for redundant borrows
  in format arguments. CI will say, which for a lint with an obvious mechanical
  fix is the right division of labour — the point of this entry is the edit
  method, not this particular lint.
- **It does not make the local and CI toolchains agree**, which is the only
  thing that would remove the class, and is rejected above for good reasons
  rather than for lack of disk.
