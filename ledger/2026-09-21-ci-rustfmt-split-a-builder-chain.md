# CI's rustfmt split a builder chain that the local one left alone

- **Date:** 2026-09-21
- **Author:** Claude Opus 5
- **Touches:** `clients/python/testserver/src/main.rs`
- **Kind:** fix

## What changed

One `.index(IndexDef::builder(...).column("title").text())` call, written on a
single 87-character line, wrapped across five. The diff is verbatim from the
`formatting` job's log.

## Why

`ef5f3bd` turned the `formatting` job red. `cargo fmt --all -- --check` passes
here on `rustfmt 1.8.0` and fails on CI's `1.98.1`, which is the toolchain gap
`CLAUDE.md` describes — and there is no local command that finds it, because
the gap *is* the problem.

Worth noting for the next person: the example in `CLAUDE.md` runs the other
way. There, CI's newer rustfmt *collapsed* a hand-wrapped call onto one long
line, and the advice drawn from it was "write what the newer formatter would
produce; fewer manual line breaks, let it wrap". This case is the mirror — a
single line the newer formatter splits — so the advice is not "never wrap" but
"the local formatter's silence is not agreement". The remedy is the same
either way: read the diff out of the job's log and apply it.

## Alternatives rejected

**Re-run `cargo fmt` locally and push whatever it produces.** It produces
nothing: the local formatter already calls this file clean. Doing it and
concluding CI is wrong is precisely the loop `CLAUDE.md` warns about.

**Install CI's toolchain to check before pushing.** The disk note rules it out
— a second toolchain does not fit here, and this container has repeatedly hit
ENOSPC on one.

**Fold the fix into the next commit.** Rejected: CI is red on a commit that is
already pushed, and a red branch waiting for unrelated work to finish is a
branch nobody can read a signal from.

## Evidence

The job log's diff, applied character for character. `cargo fmt --all --
--check` exits 0 here both before and after, which is the whole point and is
why the real verification is the next CI run rather than anything local.

## What this does not do

Nothing prevents the next one. The gap is structural — CI tracks stable,
this container does not — and the only defence remains reading the failing
job's diff rather than re-running the local formatter. Writing code the newer
formatter would produce helps and is not sufficient; this line was already
inside the default 100-column width.
