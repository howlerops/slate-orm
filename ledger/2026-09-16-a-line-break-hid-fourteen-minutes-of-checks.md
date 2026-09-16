# Formatting gets its own CI job, because as step one of the Rust job a missing line break reported the same red as a broken kernel.

- **Date:** 2026-09-16
- **Author:** an agent working through the record-layer task list
- **Touches:** `.github/workflows/ci.yml`, `CLAUDE.md`, and the formatting of
  `slate-orm` and `slate-wasm` sources this session had already committed
- **Kind:** process

## What changed

`cargo fmt --all -- --check` moved out of the `rust` job — where it was the
first of eight steps — into a job of its own that installs `rustfmt`, checks,
and stops. The `rust` job keeps clippy, the workspace tests, the `json`-feature
pass, the tuple-codec soak and `cargo doc`, and no longer installs `rustfmt`.
The same commit reformats five files this session committed unformatted, which
is what made the problem visible, and adds `cargo fmt` to the local command
list in `CLAUDE.md` — which listed clippy and the tests and not the check CI
ran first.

## Why

A session's worth of work went in without `cargo fmt` ever being run. CI caught
it, correctly. What it then did was the problem: because the check was step one
of the job, the failure ended that job before clippy or any workspace test had
run, so the run said "the Rust job failed" and said nothing at all about
whether the code compiled clean or the tests passed. The other fourteen jobs
are independent and went green around it, which is why it read as a failure
that blocked nothing — it blocked the fourteen minutes of checks standing
behind it, silently, and that is the part nobody could see.

Split, the two failures are distinguishable at a glance: `formatting` red means
whitespace, `clippy, test` red means code. A contributor who pushes a
formatting miss still learns whether their tests pass.

## Alternatives rejected

**Move the fmt step to the end of the `rust` job.** One less runner, and the
tests would run first. But a step that fails at the end of a ten-minute job is
a ten-minute wait to learn about a line break, and the job's name would still
be the only thing a reader sees on the summary page.

**Add `cargo fmt --check` to the pre-commit hook.** This is the fix that stops
the red ever reaching CI, and it was rejected on a stated design rule: the hook
is deliberately a dependency-free `sh` script, because "a hook that needs a
toolchain to run is a hook that stops running the first time somebody commits
from a container that does not have it". Guarding it with `command -v cargo`
would satisfy the letter of that and reintroduce the failure the repository
already has a rule about — *a skip is green*: the one environment where the
guard silently does nothing is a container with no Rust, which is exactly where
somebody is most likely to hand-edit a source file. The `CLAUDE.md` line is the
weaker fix and the honest one.

**Leave it and just run `fmt`.** That fixes this instance and not the class.
The structural fault — one cheap check gating fourteen expensive ones — is
there whether or not the next person remembers.

## Evidence

`cargo fmt --all -- --check` reported diffs in five files
(`crates/slate-orm/src/lib.rs`, `crates/slate-orm/tests/json.rs`,
`crates/slate-orm/tests/types.rs`, `crates/slate-wasm/tests/distinct.rs`,
`crates/slate-wasm/tests/playground.rs`); after `cargo fmt -p` over the crates
touched this session it reports none. Every diff is line breaking around long
`assert_eq!` arguments — no behaviour, which is the point: the run that went
red had nothing wrong with it but this.

The workflow parses and the job list is the expected sixteen, in order:
`fmt, rust, binaries, go, python, typescript, frontend, hooks, layout,
quickstarts, playground, deployed, demo, release-build, minio` plus the new
`fmt` at the head (`python3 -c "import yaml; ..."` over `ci.yml`).

No mutation testing: the change is a workflow split with no logic in it, and
the thing it would have to prove — that a fmt failure no longer ends the job
that runs the tests — is proven by the job boundary itself, not by a test that
could be broken.

## What this does not do

It does not stop an unformatted commit being made; it only stops that commit's
CI run from hiding everything else. The pre-commit hook still does not check
formatting, for the reason above.

It also does not address the related and larger gap named in `CLAUDE.md`: CI's
clippy is a newer toolchain than this container's, so a locally clean
`cargo clippy --workspace` is necessary and not sufficient. Splitting `fmt` out
does nothing about that, and the same class of surprise remains available from
`rustfmt` — a formatting rule that stabilises upstream will land here as a red
`formatting` job on a commit that changed nothing.
