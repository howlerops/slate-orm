# Go and TypeScript now order a chain's groups by the value the chain computes, and both do it descending so a dropped sort changes the answer. Nine more open caveats were read against the tree and found still true.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `clients/go/slate/join_test.go`, `clients/typescript/test/join.test.ts`, `docs/caveat-status.json`
- **Kind:** tests, and the re-triage pass that asked for them

## What changed

`ledger/2026-09-15-a-chains-computed-value-and-a-type-tag-in-a-label.md`
recorded that "`ORDER BY` a chain's computed value is untested from a client":
grouping a chain *by* its computed decade was tested in Go and TypeScript, and
ordering those groups was tested only for a two-table join. Both clients now
have the ordering case. `docs/caveat-status.json` marks that caveat closed and
nine others read the same morning as still true, with today's date.

## Why

Because the caveat was accurate and cheap to retire. The grouped-chain path
narrows each step's projection (`narrowed_chain`) and resolves the grouping in
the joined space, where a computed slot sits past every table — the one shape
where a sort key could plausibly resolve against the wrong thing. Nothing
executed the combination end to end, in any client.

The nine read alongside it were not retired, and that is the ordinary outcome:
the measured rate of a stale open caveat is about one in eight
(`ledger/2026-09-26-two-ways-to-find-a-stale-caveat-that-do-not-work.md`), so a
batch of ten is expected to yield roughly one. This batch yielded one.

## Alternatives rejected

**Assert ascending, which reads more naturally.** The kernel's own group order
*is* ascending by encoded key, so an ascending assertion passes unchanged
against a server that dropped `Sort` on the floor. That is the shape of test
that records a feature as covered while covering nothing, and the existing
`TestGroupsAreOrderedLimitedAndOffset` says so in a comment written for the same
reason. Descending, with an ascending run beside it to show the two are
reverses, is the version that can fail.

**Python too.** The Python suite has no chain-with-computed-value test to extend
and adding the fixture is most of the work; Go and TypeScript already had the
grouped case sitting there. The caveat said "from a client", which two of three
satisfies. It would be more honest as "from Python", and that is recorded below
rather than quietly counted as done.

**Compare `[]int64` with `reflect.DeepEqual`.** It works and pulls `reflect`
into a test file that does not otherwise need it, for three integers.
`fmt.Sprint` on the slice is already imported and reads the same.

## Evidence

**A mutation, run twice, caught both times** — and the first attempt at it found
a defect in the tooling, which is the other half of this morning and has its own
entry (`ledger/2026-09-26-a-cached-go-run-is-not-a-run.md`).

| mutation | dialect | caught by |
|---|---|---|
| `if !grouping.sort.is_empty()` → `if false` in `crates/slate-kernel/src/aggregate.rs` | `go` | `TestOrderingAChainsGroupsByItsComputedValue` |
| the same | `node` | `groups are ordered, limited and offset`, `ordering a chain's groups by its computed value` |

The Go run is the one worth reading closely: the pre-existing
`TestGroupingAChainByItsComputedValue` ran in the same command and **did not
fail**, so the new test is the only thing in that pair defending the sort. With
the mutation in place the descending assertion reported
`got [1990 2000 2010]` — the encoded-key order exactly, which is what makes the
descending choice above load-bearing rather than stylistic.

**Suites.** `clients/typescript`: **193 passed, 0 failed.** `clients/go`:
`go test ./...` green, with `-count=1` (see the other entry). No Rust code
changed.

**The nine read and left open**, each checked against the tree rather than
re-read from the entry:

| caveat | what was checked |
|---|---|
| `kernel_ms` still includes `render()` | `render(...)` at `crates/slate-wasm/src/lib.rs:904` and `:1142`, inside the region `kernel_ms: elapsed` closes at `:946` and `:1211` |
| the write paths are not timed | no `kernel_ms` anywhere in the insert/update/delete region |
| the render cap is 1,000 and not configurable | `RENDER_CAP = 1000` in `site/workbench.js:464`, `rows.slice(0, RENDER_CAP)`, and no pager anywhere in the file |
| no per-input computed value on a chain | still only on a two-input join |
| no outer-join step in a chain carries a computed value | the chains in every suite are inner throughout |
| nothing checks the other value-rendering sites | unchanged |
| nothing else audited for the `file:` dependency | three `file:` deps (`examples/explorer/backends/node`, `examples/deployed/node`, `examples/batchbench/node`) and no guard |
| the warning is the only warning | exactly one `warnings.push` in `crates/slate-sql/src/sql.rs` |
| no test asserts what the caution looks like | unchanged |

## What this does not do

**Python still cannot order a chain's groups in a test.** The caveat is closed
on the strength of two clients, not three, which is the same standard
`ledger/2026-09-25-every-client-sends-a-disjunction.md` had to go back and
raise. If a third client is the bar, this is a new and smaller caveat rather
than a closed one.

**Only the group key is sorted, not an aggregate over a chain's computed
grouping.** `ORDER BY count(*)` over groups keyed on a computed value would
exercise `Agg(0)` against the same narrowed path, and does not have a case
here or anywhere.

**The fixture is four books and three decades.** Large enough that ascending and
descending differ, small enough that a comparator wrong only on ties, or only
past some length, would pass. The assertion is over a written-out list, not a
property.

**Nine read is not 247 read.** That is the size of the unstamped open list after
this batch, and the reading pass is the only method shown to shrink it.
