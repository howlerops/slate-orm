# `cargo test --workspace` cannot finish in this container, and now says so

- **Date:** 2026-09-20
- **Author:** Claude, after losing three runs to it in one session
- **Touches:** `CLAUDE.md`
- **Kind:** docs

## What changed

One bullet in the practical notes. `CLAUDE.md` listed `cargo test --workspace`
under "slow, and the disk note"; it is not slow, it is impossible here, and the
three ways it announces that all look like broken code.

## Why

Three runs died today, from a start with 11 GB free:

```
error: linking with `cc` failed: exit status: 1
error: could not compile `slate-server` (test "multi")
error: failed to write file .../incremental/.../dep-graph.part.bin:
       No space left on device (os error 28)
```

The existing note covers the *symptom* — "a linker `Bus error` … is almost
always ENOSPC, not your code" — and gives the dedup snippet. What it does not
say is that the workspace-wide command is the reliable way to reach that state,
so the advice reads as "clean up and try again", which is what I did, twice,
losing a full rebuild each time. The fix is not a bigger clean, it is not
running that command.

The other half is what the successful shape actually is, because "run less"
is not actionable at 2 a.m.: twelve members in four or five groups, dedup
between them, `CARGO_INCREMENTAL=0` on the larger ones. That is how the whole
workspace was verified green twice today, and it is worth six lines so the
next session does not rediscover it.

`scripts/test_check_sh.py` already lists `cargo test --workspace` in
`ELSEWHERE` as "slow, and the disk note in CLAUDE.md". That entry is now
pointing at a note that says something more useful than it did, which is the
whole mechanism those cross-references exist for.

## Alternatives rejected

**Make `scripts/check.sh` run the crate-by-crate loop.** Tempting and wrong:
`check.sh` promises to need no built binary, no browser, no container and no
network, and it is that promise that makes it worth running before every
commit. A twenty-minute test loop inside it would get switched off.

**Add a `scripts/test-all.sh` that does the grouping.** A real option, and the
reason against it is that the grouping is not stable — it depends on how much
is already built, which crates you touched, and how much another session has
left on disk. A script would encode one session's answer and be wrong for the
next; a note explains the constraint and lets the reader group for their own
situation. If someone hits this a fourth time, the script becomes the right
call and this paragraph is the argument they have to beat.

**Leave it: the ENOSPC note is two bullets up.** That is the status quo and it
is what cost three runs. The note describes a *symptom* anybody can meet; this
names the *command* that causes it, which is the actionable half.

## Evidence

Three failures in one session, each after a clean start:

| attempt | free before | how it died |
| --- | --- | --- |
| `cargo test --workspace --no-fail-fast` | 11 GB | `linking with cc failed`, four crates "could not compile" |
| `cargo test -p slate-orm -p slate-schema -p slate-derive` | 7.2 GB | `dep-graph.part.bin: No space left on device` |
| `cargo test -p slate-slatedb -p slate-clickbench` | 6.5 GB | disk to 100%, the run's own output lost to a full tmpfs |

The last one is worth its own line: the tmpfs holding tool output filled too, so
the command's result was *not reported at all* rather than reported as a
failure. A run whose output you cannot read is not a run, and it is the reason
the third attempt was repeated rather than trusted.

The shape that works, twice: kernel; orm + schema + derive; server; serverd +
tuple + wasm; slatedb + clickbench; testserver — with the dedup snippet between
groups (it freed 5.46 GB, 2.98 GB and 6.15 GB on three separate calls) and
`CARGO_INCREMENTAL=0` on the two largest.

## What this does not do

**It does not fix the disk.** The container's allowance is what it is, and
nothing here reclaims more than the existing snippet already does.

**The grouping is not measured, only observed to work.** I did not try to find
the minimal number of groups, or check whether a release profile or
`--no-run` would change the picture. Five groups is what succeeded twice, not
what is optimal.

**It says nothing about CI**, where the workspace command runs on a fresh
runner with its own disk and has never failed this way. This is a note about
*here*, which is why it is in the practical-notes section rather than beside
the list of what CI runs.
