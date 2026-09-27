# The example roster is keyed on "reads a command line", not on the shared parser — and widening it found the same silent pass twice more, in `bucket_layout` and `cost_calibration`.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_examples_roster.py`, `scripts/test_check_examples_roster.py`, `scripts/run_examples.sh`, `crates/slate-slatedb/examples/bucket_layout.rs`, `crates/slate-slatedb/examples/cost_calibration.rs`, `docs/caveat-status.json`
- **Kind:** correctness

## What changed

`check_examples_roster.py` grew a fifth rule — an example that reads its
command line must have a path that refuses an argument it does not understand
— and rules 1-4 stopped keying the refusal roster on `sections::from_args`,
keying it on reading a command line at all.

Two `slate-slatedb` examples failed the new rule and are fixed: both now match
every argument and exit 2 on one they do not know, the shape `s3_server`
already had. Both gained a `refuses()` line, so a run hands each a bad
argument and checks the exit code.

## Why

`ledger/2026-09-26-the-refusal-nothing-ran.md` closed the `head_report --typo`
defect — a mistyped section name printed a header, ran nothing and exited 0 —
and recorded what the fix could not see:

> **Neither guard can see a fourth kind of silent pass.**
> `check_examples_roster.py` knows an example takes sections because its source
> says `sections::from_args`. An example that parsed `std::env::args()` by hand
> — which is what all three did before that module existed — is invisible to
> it, and would go back to printing a header and exiting 0 with nothing to say
> so.

That caveat was written about a regression the headbench examples could make.
It was already true of two examples in a different crate, which is the part
worth writing down: the caveat named a hypothetical and the hypothetical had
shipped. Keying a roster on *how* a thing is done rather than on *what it
does* is why — `sections::from_args` is one way to read a command line and the
roster only ever saw that one.

## Alternatives rejected

**Move `bucket_layout` and `cost_calibration` onto `sections::from_args`.**
Would have satisfied the old key and needed no new rule. Rejected because
neither takes sections: they take one flag each, and `Selection` is a set of
names with an "empty means all" rule that a boolean flag does not have. The
roster's key is the thing that was wrong, and making two examples fit the key
leaves it wrong for the third one that does something else again.

**A `clap`-style parser shared by every example.** The structural fix, and
what a real binary would do. Rejected for the reason the examples have no
dependencies of their own: each is meant to be readable top to bottom as a
demonstration of the record layer, and a derive macro over an options struct
is six lines a reader has to know a crate to read. The hand-written `match`
is four lines and says exactly what it does.

**Check that the refusing path exits 2 specifically.** `REFUSES_UNKNOWN`
matches `process::exit`, not `process::exit(2)`. Rejected because the exit
code is already asserted somewhere stronger: `run_examples.sh` runs the built
binary and requires 2. A regular expression over a literal would duplicate
that check in the weaker place, and the thing the regular expression is for —
an example with no refusing path at all — is exactly what the run *cannot*
catch, because such an example exits 0 and looks like a benchmark that worked.

## Evidence

**The defect, before and after, on a built binary.** `bucket_layout` at
`--release`, on this container:

```
$ ./target/release/examples/bucket_layout --jsonn   # before
seeding 100000 trips and 265 zones…
     11.0 MB  records/compacted/01M3G4WZ4FZWCKQSDX29D3D42N.sst
     …
12 objects, 11.0 MB total
EXIT=0

$ ./target/release/examples/bucket_layout --jsonn   # after
usage: bucket_layout [--json]; `--jsonn` is not an option
EXIT=2
```

Seventeen lines of the human listing for a caller who asked for the site's
JSON artifact, and exit 0. `--json` still parses as JSON (`objects`,
`provenance`) and a run with no arguments still prints the listing.

**`cost_calibration` refuses in 24 ms rather than after the fixture.**
`cold()` is first called from the first case, several minutes and 200,000
inserted rows in, so parsing there would have made a mistyped flag cost the
whole fixture — and in `run_examples.sh`, which runs the example once and then
again with a bad argument, the example's budget twice. The call is hoisted to
the top of `main`, where it also prints which cache mode the run is in:

```
$ time ./target/release/examples/cost_calibration --codl
usage: cost_calibration [--cold]; `--codl` is not an option
real	0m0.024s
EXIT=2
```

**The widening's own bug was caught by the rule it was widening.** The first
draft of `READS_ARGS` was `std::env::args` alone, and the three headbench
examples do not contain that string — `sections::from_args` reads the command
line for them. Rule 4's reverse direction reported all three as rostered for
an example that reads no arguments, which is how the pattern got its second
alternative. Written up because it is the best evidence available that the
reverse halves of these rules do something.

**Mutations.** Nine cases across three runs, no survivors:
`ledger/mutations/20260927T004421-scripts-check-examples-roster-py.json` (five
cases), `ledger/mutations/20260927T004432-scripts-check-examples-roster-py.json`
(two), and `ledger/mutations/20260927T004439-scripts-run-examples-sh.json` (two,
against the new roster lines rather than the guard).

| mutation | outcome |
| --- | --- |
| an example reading argv by hand is not read as reading argv | caught, 4 named cases |
| a hand-written `exit(2)` does not count as refusing | caught, incl. the real tree |
| rule 5 never reports | caught |
| a server is asked for a refusal line after all | caught, incl. the real tree |
| rule 5's never-fires half never fires | caught |
| a refusal line naming a server is not reported as unreachable | caught |
| rule 4's forward half never reports | caught |
| `bucket_layout`'s new refusal line, deleted | caught |
| `cost_calibration`'s refusal argument becomes one it has | caught |

**Suites.** `scripts/test_check_examples_roster.py` 30 passed 0 failed (was
23). `cargo clippy -p slate-slatedb --all-targets` clean. `sh scripts/check.sh`
69 of 69.

## What this does not do

**The two Rust fixes are not mutation-tested, because nothing unit-tests an
example's `main`.** They are demonstrated by running the built binary before
and after, which is a stronger check of the behaviour and a weaker one of the
tests — there are none to break. `run_examples.sh` is what will exercise them,
and on this container it needs the release build above rather than the debug
one the script defaults to. CI's `headbench` job is the first place the two
meet; the same "a check that never fires is a check nobody has debugged" risk
the entry this closes already took knowingly.

**`REFUSES_UNKNOWN` cannot tell a refusing path from an unreachable one.** An
example carrying a `process::exit` in a branch nothing takes passes rule 5 and
would still ignore a mistyped flag. What catches that is the roster line and
the built-binary run, which is why rule 4 was kept as well as widened rather
than replaced by rule 5 — but an example that is a *server* has no roster line,
so for `s3_server` the regular expression is all there is.

**The exclusion of servers from rule 4 is a property of the runner, spelled in
the guard.** `run_examples.sh` returns to the next example as soon as a
handshake lands, so the refusal branch is unreachable for a server. If the
runner ever grew a refusal probe for servers, this guard would keep demanding
that their lines be deleted and nothing would connect the two files.

**Three `slate-kernel` and `slate-orm` examples read no arguments at all**, so
rules 4 and 5 say nothing about them. An example that starts taking one is
caught; one that already ignores something it should not is not, because there
is nothing to ignore.
