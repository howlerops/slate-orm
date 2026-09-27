# `go test` replayed a cached result and `mutate.py` scored the replay as a surviving mutation. The `go` dialect now refuses a `(cached)` line, so the run says NOTHING RAN instead.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `scripts/mutate.py`, `scripts/test_mutate.py`
- **Kind:** fix, in the tool that decides whether a finding is real

## What changed

The `go` dialect's "a suite reported" pattern already refused
`FAIL <pkg> [build failed]` and `[setup failed]`. It now refuses
`ok <pkg> (cached)` the same way, so a replayed result scores as zero suites
reported — which `mutate.py` treats as a hard error naming the command — rather
than as a clean run. A case in `scripts/test_mutate.py` drives the fake `go
test` into printing a cached replay and asserts NOTHING RAN and not SURVIVED.
The module docstring's list of ways a mutation run lies gains a sixth.

## Why

Because it told me a real test was missing when the test was there and working.

Mutating `if !grouping.sort.is_empty()` to `if false` in the kernel, then
running the Go client suite, reported:

```
  !!   a grouping's sort is never applied: SURVIVED (1 suites ran).
```

One suite ran, nothing failed, and the script's own advice is that a survivor
means a missing test or redundant code. Both readings were wrong. `go test`
caches a package's result on that package's *Go* inputs; `clients/go` builds
`slate-serverd` out of the Rust tree and talks to it over a socket, so a
mutation in `crates/` changes nothing the cache hashes. The run replayed the
previous verdicts — `--- PASS`, with the *old* durations, which is the tell I
missed — and printed `ok <pkg> (cached)`.

`-count=1` on the same command, same mutation, failed two named tests
immediately. So the mutation was caught all along, and the only thing the run
established was that `go test` is allowed to not run.

This is failure mode 2 from the docstring — "nothing runs, and the empty output
reads as a pass" — arriving through a door nobody had thought about, and mode 4
as well: the wrong code was judged. Neither is a bug in `go`. The cache is
correct about its own inputs; the assumption that a Go package's inputs bound
its behaviour is what fails in a repository where the subject under test is a
server in another language.

What made it survivable is that a survivor exits non-zero and interrupts. Had
`mutate.py` been the hand-rolled `sed` and `grep FAILED` that `CLAUDE.md`
warns against, the output would have been an unremarkable green and the
conclusion would have been written up as a coverage gap in the kernel.

## Alternatives rejected

**Put `-count=1` in the docstring and leave the dialect alone.** A rule a
reader has to remember is the thing this script exists to replace, and the cost
of forgetting is not a visible error — it is a survivor that reads as a
finding. Three of the five existing protections are here precisely because a
convention failed silently.

**Have `mutate.py` inject `-count=1` into a `go` command itself.** Tempting,
and wrong in the same way a helpful default usually is: the command is the
caller's, it may not be a bare `go test` (this one was a `sh -c` with a pipe
through `grep`), and rewriting somebody's argv on a guess is how a tool starts
lying about what it ran. Refusing to score is honest and needs no parsing.

**Treat a cached line as a report and compare it against the baseline.** The
baseline run is also cacheable, so both sides can be replays of the same stale
verdict and agree perfectly. Comparing two replays is not evidence.

**Set `GOFLAGS=-count=1` in the environment for the whole repository.** It
would fix this and would also turn off caching for every ordinary `go test` a
developer runs, which is a real cost paid all day to fix a problem that arises
only under mutation. The suite takes about seven seconds uncached; the cache is
worth keeping for the ninety-nine runs that are not mutations.

## Evidence

**The defect, demonstrated both ways.** Same mutation, same file, same package,
one command apart:

```
$ go test ./slate/ -run 'OrderingAChains…|GroupsAreOrdered…' -v
--- PASS: TestGroupsAreOrderedLimitedAndOffset (76.06s)
--- PASS: TestOrderingAChainsGroupsByItsComputedValue (0.04s)
ok  	github.com/howlerops/slate-orm/clients/go/slate	(cached)

$ go test ./slate/ -count=1 -run '…same…' -v
    join_test.go:434: groups are not ascending by count: [{[1] [3]} {[2] [1]}]
--- FAIL: TestGroupsAreOrderedLimitedAndOffset (7.29s)
    join_test.go:875: descending by the computed decade: got [1990 2000 2010]
--- FAIL: TestOrderingAChainsGroupsByItsComputedValue (0.04s)
FAIL
```

The 76.06 seconds in the first block is the giveaway and is itself a replay:
that test does not take 76 seconds, it took 76 seconds once, on the run that
also compiled the server.

**The fix, demonstrated.** With the dialect unchanged the run above scored
SURVIVED. With it changed, the same spec — no `-count=1` — now reports:

```
  !! cached-run check: NOTHING RAN — []
```

and with `-count=1` added:

```
  ok   a grouping's sort is never applied  ->  TestOrderingAChainsGroupsByItsComputedValue
```

**`scripts/test_mutate.py`: 29 passed, 0 failed** (28 before). The new case
fails against the old pattern, which is the only thing that makes it a test
rather than a description.

**No mutation run on this change**, and the reason is not laziness: the change
*is* a mutation-detection pattern, and the case added exercises it directly by
feeding the script output no other case produces. Mutating the pattern — say,
dropping `\(cached\)` back out — is exactly what the new case catches, and
running `mutate.py` against itself to prove that is how #285 found its own
defect. Doing it here would restate the test.

## What this does not do

**Nothing stops the next `go` mutation being written without `-count=1`.** It
will now fail loudly instead of lying, which is the whole change, but the
author still has to add the flag and re-run. A `go` command with no `-count=1`
could be refused up front; it is not, because the command may be a shell
pipeline and pattern-matching argv to guess whether caching is in play is the
kind of cleverness that fails on the one command that matters.

**The other dialects are not audited for their own version of this.** `pytest`,
`node` and `cargo test` have no result cache of this kind — `cargo` rebuilds
when the Rust source changes, which is the case that broke `go` — but
`pytest-xdist`, a `--lf` flag, or a `node --test` runner with caching would
each reintroduce it. I checked none of them; I checked `go`, because `go` is
where the lie was met.

**The in-flight marker's post-restore check inherits the hole's shape.** When
the restore verification also cannot score, the run prints "the tree did not
come back clean" with an empty failure list, which reads as an unclean tree and
means "I could not confirm either way". It is a hard error and not a false
pass, so it is honest; it is not clear, and rewording it is not done here.

**The audit of earlier runs came back clean, and that is luck rather than
process.** `ledger/mutations/` holds **five** `go`-dialect runs. The two from
before today both carry `-count=1` *and* mutate a file under `clients/go`, so
the cache would have invalidated on its own either way; the three without it
are this morning's, two of which are the defect and the third its confirmation.
So nothing earlier was spoiled — but the records keep the command and the
verdict, not the raw output, so a cached run and a genuine survivor are
indistinguishable in the file. Had the answer gone the other way it would have
been unknowable, which is the same gap #287 closed one layer up and has not
closed here.
