# The stamp named one dependency of four that can move a number — and finding that out exposed a worse thing: `cargo test -q` made every Rust mutation score as a survivor.

- **Date:** 2026-09-22
- **Author:** Claude Code, working #285 (F6e)
- **Touches:** `crates/slate-slatedb/{build.rs,lockfile.rs,src/stamp.rs,tests/lockfile.rs}`, `scripts/{mutate.py,test_mutate.py,test_check_table_provenance.py}`, `docs/performance.md`
- **Kind:** widening the stamp, and a defect in the tool that verifies everything here

## What changed

The build stamp names **four** dependencies instead of one:

```
build: slate-slatedb 0.0.1 | features aws, cache | off dhat-heap | release (opt-level 3, debug false) | x86_64-unknown-linux-gnu | slatedb 0.16.0, foyer 0.22.3, object_store 0.14.1, tokio 1.53.1
```

The lockfile parser moved out of `build.rs` into `lockfile.rs`, `include!`d by
both the build script and a new `tests/lockfile.rs`, so it can be handed a lock
written for the purpose.

And `mutate.py` now **refuses to score** a run it cannot read.

## Why

#269 offered three explanations for the point-read figure it could not
reproduce — a SlateDB release, a block size, the readahead #34 turned on — and
the code behind all three lives in `slatedb` and `foyer`. `foyer` is the cache
whose *absence* was the entirety of finding 8: a build without it issued three
times the object-store requests. A build with a **different version** of it is
exactly as worth recording as a build with none of it, and until now the stamp
could not tell those apart. `object_store` issues the requests every GET count
on that page is a count of; `tokio` schedules the concurrency the
pipelined-read costing is about.

The list stops at four deliberately. `rustls` and `aws-lc-rs` are under every
S3 byte too; nothing here isolates a connection cost, and a line nobody reads
records nothing.

**The `mutate.py` finding is the more important half.** Running the first Rust
mutations for this task, two came back SURVIVED. Checking one by hand showed
the test failing exactly as it should. The cause: the `rust` dialect finds
failures by `^test NAME ... FAILED$`, and **`cargo test -q` never prints that
line** — `-q` suppresses per-test results, leaving only `test result: FAILED.
7 passed; 1 failed`, which satisfies the "did anything run" check and matches
no name.

So: exit 101, a suite reported, no failing test named, **scored a survivor**.
Silently, in the direction that reads as "your tests are weak" rather than
"this tool is not working". I had already written two `expect_survivor`
records against results that meant nothing.

## Alternatives rejected

**Stamp every dependency in the lock.** Six hundred packages, complete by
construction, unreadable in practice — and a line nobody reads is the same as
no line. Four named with a reason beats six hundred named with none.

**Teach the `rust` dialect to read `test result: FAILED`.** It would have
fixed this instance and left the class open: the real problem is that the tool
cannot tell "nothing failed" from "I could not see what failed". The check
implemented is on the *inconsistency* — non-zero exit, something reported, no
name — so it catches the next dialect mismatch too, in Go or node or whatever
comes after.

**Ban `-q` in the spec.** Blunter and worse: `-q` is fine for the baseline and
fine for Python suites, and a rule against a flag is a rule to be worked
around. Refusing the *inconsistency* refuses only what is actually unsound,
and the message names `-q` because that is the cause in practice.

**Leave `parse_locked` in `build.rs` and record the survivors.** That was the
first instinct and it was wrong twice over. A build script is not a library,
so the parser could only ever be run against this repository's own lock —
where all four packages resolve, each exactly once. Neither the
duplicate-version rule nor the missing-package fallback had any test that
could distinguish it from its opposite. "Cannot be tested" was a property of
where the code sat, not of the code.

**A published `mod lockfile` on the crate.** Lockfile parsing in the public API
of a storage crate, for one test. `include!` keeps it private to the two
places that need it.

## Evidence

**Four rounds of mutation, and the rounds are the finding.**

| round | result | what it meant |
|---|---|---|
| 1, with `-q` | 2 survived | **the tool could not read the output** |
| 2, without `-q` | 2 survived | real: the parser had no caller |
| 3, after extracting `lockfile.rs` | 4 caught, 2 survived | real: no malformed-lock case |
| 4, after two more tests | **all caught, none survived** | |

<!-- not a measurement -->

Round 3 also caught a mistake of mine worth naming: the command was
`cargo test --test lockfile --lib stamp`, and the filter `stamp` applies to
**both** targets — so most of the new lockfile tests never ran, and five
mutations "survived" a suite that had not executed them. Two different ways in
one task for a mutation run to report a survivor that is not one.

`mutate.py`'s own suite gained two cases: `unreadable` over its four argument
combinations, and an end-to-end run against a fake that prints exactly what
`cargo test -q` prints on failure. It must exit non-zero, explain itself, name
`-q`, and **never** print "survived". 24 passed, 0 failed.

`tests/lockfile.rs` has nine cases, including two over locks cargo would not
write — a second `name` before the `version`, and a block that never reaches
one. Both pin the same property: on malformed input this answers `unknown`
rather than attributing a version to the wrong package.

`cargo clippy --workspace --all-targets`: **0**. `sh scripts/check.sh`: **47
passed, all of them.** The line was read off a real run, not constructed.

## What this does not do

**It does not stamp the versions of anything above `slate-slatedb`.** The
kernel's own dependencies — `regex`, `chrono` — are invisible, and a regex
engine's performance is inside every ClickBench number.

**A duplicated dependency is reported but has never happened.** Twenty-six
packages in this lock resolve twice; none of these four does. The rule is
tested against a written lock, not observed in ours.

**`mutate.py`'s refusal cannot catch a dialect that reads *too much*.** A
pattern matching lines that are not failures would make every mutation look
caught — the opposite error, equally silent, and nothing here detects it.

**The `-q` defect invalidates earlier Rust mutation runs.** Any recorded in
previous tasks with `cargo test -q` proved nothing. I have not audited which;
they are in entries #241 onward wherever a `rust` dialect spec appears, and
re-running them is not part of this task.
