# On macOS every retry backoff was exactly half its ceiling

- **Date:** 2026-10-03
- **Author:** Claude Code (session: picking up from the handoff prompt, on macOS)
- **Touches:** `crates/slate-kernel/src/retry.rs`
- **Kind:** fix

## What changed

`jitter` in `retry.rs` used to take its randomness from the clock's
sub-second nanoseconds modulo 1,000. It now mixes three inputs through
SplitMix64's finaliser: the clock's whole nanosecond reading, a process-wide
sequence number, and the address of that sequence number. The pure part is
split into `mix` and `spread`, so it can be tested with a clock that does not
move.

## Why

**macOS's `SystemTime` counts in microseconds.** `subsec_nanos() % 1000`
therefore returned 0 on every call there: one distinct value from 64 calls,
measured on this machine. So every backoff came out at exactly half its
ceiling, and writers that collided once woke in lockstep and collided again.
The module doc names that as the one thing the jitter exists to prevent.
Linux's clock has the low digits, which is why CI never saw it.
`backoff_grows_then_stops_growing` in `crates/slate-kernel/tests/writer.rs`
failed on the first Mac it ran on, at the assertion `backoff was not
jittered`. It failed identically on unmodified `9b060f8`, so this was not
caused by the T3 work that found it.

This is product code, not tooling: any deployment on macOS, which includes a
developer's machine running the server, had no jitter.

## Alternatives rejected

- **Use the clock's microseconds instead** (`% 1000` of `subsec_micros`).
  This works until a platform with millisecond resolution appears, and it
  still fails for two calls inside one tick. That second case is the one that
  matters, because colliding writers retry at nearly the same instant.
- **A `rand` dependency.** It would be the first random-number dependency in
  the kernel, for a value whose only job is to differ. The kernel's
  dependency closure also has to stay inside what `slate-wasm` can build,
  which `scripts/check_wasm_runtime.py` guards.
- **`std::process::id()` to separate processes.** It panics on
  `wasm32-unknown-unknown`, which this crate builds for. The address of a
  static is what address-space randomisation already varies per process. It
  costs no syscall, and on wasm it is merely constant.
- **`RandomState`** (`std::collections::hash_map::RandomState::new()` hashed
  once). It is seeded per process, but it reads the OS entropy source on first
  use, and whether wasm has one depends on the target.
- **Fix only the integration test**, for example by sleeping between samples.
  That would make the test pass and leave the bug in place.

## Evidence

`rustc -O` probe on this machine: 64 consecutive `SystemTime::now()` calls
gave one distinct value of `subsec_nanos() % 1000`. After the fix,
`cargo test -p slate-kernel --test writer` passes 7 of 7 on macOS, where it
had failed 1 of 7 on every run.

Mutations, `ledger/mutations/20261003T193358-crates-slate-kernel-src-retry-rs.json`:

| mutation | caught by |
| --- | --- |
| the sequence number left out of the mix | `a_clock_that_does_not_move_still_spreads_the_backoff` |
| the counter's address left out of the mix | `two_processes_in_step_wake_apart` |
| back to a clock that contributes nothing and no sequence | `backoff_grows_then_stops_growing` |
| `spread` ignoring its entropy | all three |

The first two are caught by unit tests that fix the clock's value. Those
tests mean the same thing on Linux, so CI can now see the failure mode that it
previously could not.

`cargo check -p slate-wasm --target wasm32-unknown-unknown` compiles.

## What this does not do

- **Two processes in step are told apart by ASLR.** Where address-space
  randomisation is off, which includes `wasm32`, two processes whose clocks
  read the same nanosecond and whose sequence numbers agree get the same
  backoff. The clock's full reading makes that a same-nanosecond coincidence
  rather than the certainty it was.
- **The clock is still read on `wasm32-unknown-unknown`**, where
  `SystemTime::now()` panics. This was true before the change and was not
  touched. A wasm caller that retries with a non-zero backoff would panic in
  both versions.
- **Nothing scans for other uses of a clock's low digits as randomness.**
  `grep -rn subsec_nanos crates` found this one use.
