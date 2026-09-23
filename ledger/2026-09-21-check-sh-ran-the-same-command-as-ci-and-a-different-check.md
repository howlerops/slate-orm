# `check.sh` ran CI's exact clippy command and a weaker check, because of an env var

- **Date:** 2026-09-21
- **Author:** Claude Code (agent session)
- **Touches:** `scripts/check.sh`, `scripts/test_check_sh.py`, `crates/slate-orm/src/factory.rs`
- **Kind:** fix

## What changed

`scripts/check.sh` now exports the two variables `ci.yml` sets in its
workflow-level `env:` block — `RUSTFLAGS=-D warnings` and
`CARGO_TERM_COLOR=always` — before running anything.
`scripts/test_check_sh.py` gained a fourth check that holds the script's
exports to that block, with an `ENV_ELSEWHERE` escape hatch in the same shape
as the existing `ELSEWHERE`. The guard also now prints `ok`/`FAIL` lines and a
`N passed, M failed` summary like its siblings.

The defect that exposed it is fixed too: `factory.rs`'s word picker indexed a
sixteen-element array with `word % 16`, which `clippy::indexing_slicing`
reports and cannot prove safe.

## Why

CI went red on `a82915b` with two `indexing may panic` errors in a file that
`sh scripts/check.sh` had just reported clean, running what is *literally the
same command*: `cargo clippy --workspace --all-targets`.

The difference is not in the command. `ci.yml` sets `RUSTFLAGS: -D warnings`
once at workflow level, so every step of every job inherits it and no `run:`
line mentions it. `indexing_slicing` is a warn-level lint here; it exits zero
locally and fails the job there.

That makes the guard's whole premise wrong in a way the guard could not see.
`test_check_sh.py` compares *command strings*, and these two strings were
identical. The comment at the top of `check.sh` says a green run there is
"necessary and nowhere near sufficient" — true, and it was silently less
sufficient than it claimed, because the checks it *did* run were not the checks
CI ran.

## Alternatives rejected

**Just fixing the `factory.rs` index and moving on.** That is the third time
this session a commit has gone red on a lint after a green local run, and the
first two were genuinely the toolchain-version gap `CLAUDE.md` documents — a
lint that does not exist in this container. This one was different: the lint
exists here, fired here, and was ignored here. Treating it as another instance
of the known gap would have been wrong and would have left the hole open.

**Setting `RUSTFLAGS` only for the clippy checks.** Rejected: it is set
workflow-wide in CI, so `cargo doc` and `cargo clippy -p slate-orm --features
json` run under it too. Matching per-check would be a second list to keep in
step with the first, which is the thing being fixed.

**Putting `RUSTFLAGS=-D warnings` into the command column.** The column holds
the command `ci.yml` holds, literally, so that `test_check_sh.py` can compare
the two by string. Prefixing it would break that comparison for every Rust
check at once, to express something that is not part of the command.

**A comment in `check.sh` saying "remember the env".** That is what the
existing `CLAUDE.md` notes about clippy and ruff versions are, and they did not
stop this. A guard that fails is worth more than a paragraph that is read once.

**Leaving the guard printing one `ok` line.** It could not be mutation-tested:
`scripts/mutate.py` reported "no test results at all" against it, which is the
script's second failure mode — "nothing ran" reading as "nothing failed". The
same defect was found and fixed in `test_codegen.py` earlier in this session,
by the same means, which is a small piece of evidence that the house output
format is load-bearing rather than cosmetic.

## Evidence

The failing CI job is `clippy, test` on `a82915b`, run 35548282907, two errors
both `crates/slate-orm/src/factory.rs`, `indexing may panic`. Reproduced
locally with `RUSTFLAGS="-D warnings" cargo clippy -p slate-orm --all-targets`,
which is the point: the command was always available and nothing ran it.

After the fix, `sh scripts/check.sh` reports 32 passed with `-D warnings` in
force, and `RUSTFLAGS="-D warnings" cargo clippy -p slate-orm --all-targets` is
clean.

Five mutations through `scripts/mutate.py`, **all five caught** by
`check.sh exports ci.yml's workflow-level env`:

| mutation | file |
| --- | --- |
| `check.sh` stops exporting `RUSTFLAGS` | `scripts/check.sh` |
| `check.sh` exports `-W warnings` instead of `-D warnings` | `scripts/check.sh` |
| `check.sh` stops exporting `CARGO_TERM_COLOR` | `scripts/check.sh` |
| `ci.yml` gains an env var `check.sh` does not export | `.github/workflows/ci.yml` |
| `ci.yml` drops `RUSTFLAGS` while `check.sh` still exports it | `.github/workflows/ci.yml` |

Both directions matter and both are covered: the script falling behind the
workflow is how this happened, and the script staying ahead of it is how a
stale `ENV_ELSEWHERE` reason would survive.

**A process note, because it cost time.** The first attempt at these mutations
was a hand-rolled shell loop with `cp` for backup and restore. The backup path
was a directory, `cp` failed, the restore never ran, and `check.sh` was left
mutated — which then made the *second* case fail to find its anchor and report
something meaningless. That is the failure mode `mutate.py` was written for,
met again by not using it, and `CLAUDE.md` says so in as many words.

## What this does not do

**It does not close the toolchain-version gap**, which is a different problem
with the same symptom. CI's clippy, rustfmt and ruff are newer than this
container's, and a lint that does not exist here cannot be run here whatever
the flags are. This fix covers only the case where the lint exists locally and
was not being treated as fatal.

**It reads only the workflow-level `env:` block.** A `env:` under a job or a
step is not compared, deliberately: it sits beside the command it modifies,
where a reader comparing the two files can see it. If one is added and matters,
nothing here will say so.

**It does not check `Cargo.toml`'s `[lints]`**, `.cargo/config.toml`, or any
other place a flag can come from. Those are in the repository and apply to both
runs equally, which is why they are not a source of divergence — but "equally"
is an assumption nothing verifies.

**It does not make a green local run sufficient.** The closing caveat in
`check.sh` still lists nine things it cannot reach, and that list is unchanged.
