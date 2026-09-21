# Two more crates had examples nothing ran — the same defect a third and fourth time — and the guard that found them keeps every floor exact.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #274 (F5t)
- **Touches:** `scripts/{check_examples_roster.py,test_check_examples_roster.py,run_examples.sh,check.sh,test_check_sh.py}`, `.github/workflows/ci.yml`
- **Kind:** guard, and the defect it found

## What changed

`scripts/check_examples_roster.py` is new. It holds three rules about
`scripts/run_examples.sh`, each checked in both directions:

1. Every workspace member with an `examples/` directory has a floor.
2. Every floor **equals** the number of examples on disk.
3. An example whose source never returns has a handshake line, only such an
   example has one, and the line it waits for occurs in that source.

Rule 1's first run reported `slate-kernel` (4 examples) and `slate-orm` (1).
Both now have floors, and a `kernelbench` job runs them.
`scripts/test_check_examples_roster.py` is 16 cases over workspaces it writes.

## Why

This is the third and fourth time. #265 found `slate-headbench`'s five
benchmarks built by clippy and run by nothing — four of the five broken. #271
found `slate-slatedb`'s eight in the same state, two of them broken. Both were
fixed by adding the crate to a runner, and both times the runner's own list was
the thing nobody would keep current.

The previous entry's caveat said as much and got the shape wrong. It worried
that *adding a third crate* would be caught only by the script refusing an
unknown name, and called that refusal deliberate. It is deliberate and it
protects nobody: nothing ever calls the runner with a crate it does not know,
so that arm fires for no one. Meanwhile there were already four crates with
examples and the script named two.

A benchmark nobody runs is a constant nobody re-measures. `POINT_READ_COST` was
three times its measured value when somebody finally ran one (#266).

Rule 2 is the quieter half. `least` is compared with `-lt`, so a floor that
drifts *below* the count stops catching a deleted benchmark and the run stays
green. Equality rather than a bound, because being exact about how many there
are is the floor's entire job.

## Alternatives rejected

**Add the two floors and stop.** The fix without the guard, which is what the
last two tasks did, and the reason there was a fourth occurrence to find.

**Glob `crates/*/examples` rather than read `Cargo.toml`.** Simpler and wrong:
a directory under `crates/` that is not a workspace member is compiled by
nothing, so demanding a floor for its examples would demand a job that runs
code `cargo` never builds. A mutation making this change is caught by a fixture
with a detached crate in it.

**Roster the server examples by name.** The handshake table already is one.
Checking it against a *roster* would be a third list; checking it against a
property of the source — `future::pending` or `signal::ctrl_c` — means a second
server benchmark is caught by being one rather than by somebody remembering.
The cost is that a third way of awaiting forever is invisible until `FOREVER`
learns it, which is why the guard fails outright when the pattern matches no
example anywhere.

**A `run.sh` wrapper for each new crate, matching the other two.** Those exist
so the paths already quoted in `docs/performance.md` stay true. Nothing quotes
a path for these two, so CI calls the shared runner directly and there are two
fewer four-line files to keep in sync.

**Put the two new smoke runs in `scripts/check.sh`.** They are the fast ones —
about thirty seconds between them once built — but the cost that matters is the
build. `check.sh` needs no built binary, which is exactly what makes it worth
running before every commit. They are in `ELSEWHERE` with that reason, and
`scripts/test_check_sh.py` enforces that they are accounted for either way.

## Evidence

`sh scripts/check.sh`: **40 passed, all of them** (38 before; this adds two).
`python3 scripts/test_check_examples_roster.py`: **16 passed, 0 failed.**
`python3 scripts/test_check_sh.py`: 4 passed — *81 steps, 6 blocks and 2 env
vars, all accounted for*.

The guard's first run, before any fix, which is the finding:

```
`slate-kernel` has 4 example(s) and no floor in run_examples.sh, so nothing
runs them — they are compiled by `cargo clippy --all-targets` and executed by
no job.
`slate-orm` has 1 example(s) and no floor in run_examples.sh, [...]

2 problem(s)
```

and after:

```
ok    18 examples across 4 crates, every floor exact, 1 handshake(s) rostered
```

**All five of the newly-rostered examples already worked**, which is a null
result and the opposite of what the last two tasks found. Run individually in a
debug build:

| example | exit | seconds |
|---|---|---|
| `slate-orm/multi_tenant` | 0 | 0 |
| `slate-kernel/concurrency_probe` | 0 | 0 |
| `slate-kernel/perf_report` | 0 | 6 |
| `slate-kernel/correlation` | 0 | 9 |
| `slate-kernel/flatten_cost` | 0 | 16 |

and through the runner: `sh scripts/run_examples.sh slate-kernel --smoke` — *4
passed, 0 failed*; `slate-orm` — *1 passed, 0 failed*. Thirty-one seconds
between them, which is why they share one CI job.

**Mutations via `scripts/mutate.py`, seventeen, over two files.** Thirteen in
the guard, four in `run_examples.sh`'s own tables; both runs exit 0.

One survived the first attempt: deleting the *"no example anywhere awaits
forever"* never-fires guard changed no verdict, because the shared fixture
crate every case sits on is a server, so `servers` was never empty. A case that
runs without it catches it. That is the same finding as yesterday's, in a new
place: **a guard whose only subject is a consistent fixture tests almost
nothing**, and the fixture has to be able to be inconsistent in the one way the
rule is about.

The four against the real tree are the ones that matter for staleness: the
kernel's floor dropped to 3, the ORM's floor deleted, the handshake waiting for
`READY`, and the handshake naming `s3_daemon`. All four caught by *the real
tree's floors and handshakes agree*.

## What this does not do

**Nothing runs any of the four crates at its recorded size.** Unchanged from
#271's entry, and these five do not even have a knob to shrink: none reads an
environment variable, so `--smoke` runs them at full size and `docs/
performance.md`'s figures are still taken by hand.

**`FOREVER` is two spellings of one idea.** An example that blocks on a
`recv()` that never arrives, or joins a thread that never exits, would not
match — it would hang a smoke run and this guard would not have warned. The
`if not servers` branch means a rewrite that stops matching *everything* fails
loudly; a rewrite that stops matching *one* does not.

**The floor rule cannot tell an example from a helper.** Anything named
`*.rs` in `examples/` counts, which is right for `cargo` and would be wrong if
somebody put a shared module there. Nobody has.

**The new CI job is untested in CI.** It is written and green locally; whether
`kernelbench` passes on a fresh runner is not known until this pushes, which is
the same caveat every job here carries the first time.
