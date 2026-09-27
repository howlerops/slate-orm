# `head_report --typo` printed a header, ran nothing and exited 0. Three examples shared the hole; one module closes it, and a second refusal catches the roster that closes it going stale

- **Date:** 2026-09-26
- **Author:** Claude, working from the standing instruction to address every caveat
- **Kind:** fix
- **Touches:** `crates/slate-headbench/src/{lib.rs,sections.rs}`, `crates/slate-headbench/examples/{head_report,head_concurrency,stream_step}.rs`

## What changed

`slate_headbench::sections` reads an example's arguments as section names and
**refuses one the example does not have**, exiting 2 with the list of names
that exist. The three examples that took bare section names now use it.

```
$ head_report --typo
no section named --typo. This example has: rpc, stream, commit, routing, lease, views.
Run it with no arguments for all of them.
$ echo $?
2
```

It carries a second refusal. `Selection::wants` records every name it is asked
about, and `Selection::confirm`, called after the last section, **panics if the
roster and the body have drifted** — a name in the list that selects nothing,
or a section run that the list does not have. Without it the fix is one hand-
maintained list away from refusing a name a reader typed correctly.

## Why

`ledger/2026-09-21-the-benchmarks-run-now-and-a-fourth-was-broken.md` recorded
it as a caveat and left it:

> **A nonsense section argument runs nothing and exits 0.** `head_report
> --typo` matches no section, prints its header and stops. `--smoke` passes no
> section so it is not reachable from CI, but it is a hole in the same family
> as everything above and it is not closed here.

"The same family" is the right diagnosis and understates it. An exit code of 0
over an empty report is a **skip that reads as a pass**, which is the failure
`CLAUDE.md` names outright — and it is worse in a benchmark than in a test
runner, because what a reader takes away is a table, and a *missing* table
reads as a section that had nothing to say rather than as a section that never
ran. Somebody sweeping four crates with a shell loop and one typo gets a clean
exit and a short report.

All three examples had grown the same two lines by copying:

```rust
let requested: Vec<String> = std::env::args().skip(1).collect();
let wanted = |name: &str| requested.is_empty() || requested.iter().any(|s| s == name);
```

That is the shape this repository keeps finding: a snippet copied three times
is a defect fixed one-third of the time. One module, three call sites.

## Alternatives rejected

**`clap`, or any argument parser.** It would give `--help`, a usage line and
the refusal for free. It is a dependency on the build path of a benchmark whose
whole argument grammar is "zero or more names from a list of six", and
`slate-headbench` exists (per its own header) partly so that a benchmark can be
built without pulling in a test tree. The refusal is fifteen lines; the parser
is a crate and a compile.

**Refuse in each example.** Three copies of the same fifteen lines, which is
the thing that produced the defect. Cost of the module: one more file in a
crate of six.

**Return an error from `from_args` and let the example decide.** Every example
would `eprintln!` and exit, which is what `from_args` does, and one of the
three would eventually forget. Exiting from a helper is wrong in a library and
right in a benchmark harness whose `lib.rs` already says so: *"a benchmark
harness is allowed to be direct about failure: there is no caller to hand an
error to"*. The constructor that does not exit is public beside it, and the
tests use it.

**Derive the roster instead of writing it.** It is the honest instinct, and it
is what `Selection::confirm` is for. There is nothing to derive from: each
section is a function called behind an `if`, and no static check knows which
string literals in a `.rs` file are section names. The roster is hand-written
and a runtime assertion keeps it true — the same trade as
`scripts/check_handlers.py`'s `AUTHENTICATORS`, made for the same reason.

**Exit 1 rather than 2.** `scripts/run_examples.sh` reads any non-zero exit as
"the benchmark failed", and "you asked for the wrong thing" is a different
event. Nothing distinguishes them today; 2 is there so that something can, and
the comment says exactly that rather than implying the distinction is already
made.

## Evidence

**The defect, before and after.** Before, `head_report --typo` printed its
header and exited 0. After:

```
$ ./target/debug/examples/head_report --typo ; echo $?
no section named --typo. This example has: rpc, stream, commit, routing, lease, views.
Run it with no arguments for all of them.
2

$ ./target/debug/examples/stream_step strem widht ; echo $?
no section named strem, widht. This example has: nagle, base, width, widenagle, memory, split, single, short.
Run it with no arguments for all of them.
2
```

Both unknown names, not the first: a reader who mistyped two wants both.

**All three rosters verified by running the examples, not by reading them.**
`confirm()` only fires at the end of a run, so each was run to completion with
one section selected and the cheapest knobs the example offers:

| example | command | exit | sections printed |
|---|---|---|---|
| `head_report` | `head_report views` | 0 | 1 |
| `head_concurrency` | `HEADBENCH_RUNS=1 HEADBENCH_WINDOW_MS=60 HEADBENCH_LEVELS=1 … nagle` | 0 | 1 |
| `stream_step` | `HEADBENCH_RUNS=1 HEADBENCH_BATCHES=8 … base` | 0 | 1 arm |

Each evaluates `wants` for every section and runs one, so a roster that
disagreed with the body would have panicked. None did. Debug builds — the
numbers they printed are not comparable with `docs/performance.md` and none is
quoted here.

**`cargo test -p slate-headbench --lib`: 9 passed**, all new.

**Mutation run**,
`ledger/mutations/20260926T022609-crates-slate-headbench-src-sections-rs.json`
— **seven cases, seven caught**, each by a named test:

| mutation | the test that failed |
|---|---|
| `if !unknown.is_empty()` → `if false` | *a_mistyped_section_is_refused_rather_than_running_nothing* (and two more) |
| `.filter(…)` → `.filter(…).take(1)` | *every_unknown_name_is_reported_not_only_the_first* |
| the refusal's `known.join(", ")` → `String::new()` | *the_refusal_lists_the_sections_there_are* |
| drop `self.requested.is_empty() ||` | *no_arguments_means_every_section* |
| `wants` → `true` | *a_named_section_runs_and_the_others_do_not* |
| `assert!(known == asked, …)` → `assert!(true, …)` | *a_section_in_the_roster_that_never_runs_is_refused*, *a_section_run_that_is_not_in_the_roster_is_refused* |
| `wants` stops recording the name | four tests, including both drift cases |

Each replacement changes behaviour rather than spelling, checked before running
per `CLAUDE.md`'s note on the equivalent mutation: four are control-flow or
predicate inversions, one truncates an iterator, one empties a message, one
removes a side effect the drift check reads.

`cargo clippy -p slate-headbench --all-targets`: clean. `cargo fmt -p
slate-headbench` applied.

## What this does not do

**Nothing in CI runs an example with a bad section.** The refusal is
demonstrated by hand above and by unit tests over `Selection`; no automated run
passes `--typo` to a built binary and asserts exit 2. `scripts/run_examples.sh`
builds and runs the examples and has no negative case, which is the same shape
as the caveat this closes, one level up.

**`confirm()` is only reached by a run that finishes.** An example that panics
in a section never checks its roster, so the drift guard is weakest exactly
when the example is already broken. It could run first, from the roster, before
any section — that inverts the design (the roster would drive the run rather
than describe it) and is a larger change than this one.

**The two `SECTIONS` rosters that are not `head_report`'s were verified once,
today.** Each run costs minutes even at the smallest knobs, so nothing re-runs
them; a section renamed tomorrow is caught by `confirm()` the next time
somebody runs the whole example, which is rarely.

**It says nothing about `slate-slatedb`'s nine examples**, which take no
section arguments at all today. If one grows them it will grow this hole too,
and the module is there rather than the habit being fixed.
