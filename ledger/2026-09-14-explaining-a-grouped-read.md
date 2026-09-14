# EXPLAIN for a grouped join, a grouped chain, and a grouped table

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `crates/slate-kernel/src/{read,record,explain}.rs`,
  `crates/slate-schema/src/columns.rs`,
  `crates/slate-server/{proto,src/{service,session,convert}.rs}`,
  `crates/slate-server/tests/{multi.rs,common/mod.rs}`,
  `crates/slate-kernel/tests/{grouped_chain_oracle.rs,snapshots/plans.txt}`,
  all three clients, `README.md`
- **Kind:** feature

## What changed

A caller can now ask what plan a *grouped* read would run under.

* Kernel: `explain_grouped`, `explain_grouped_join`, `explain_grouped_chain`.
* Wire: `rpc ExplainAggregate(ExplainAggregateRequest) returns
  (AggregateExplainResponse)`, taking the same `AggregateQuery` that
  `Aggregate` takes.
* Clients: `explain_aggregate` (Python), `ExplainAggregate` /
  `ExplainAggregateJoin` (Go), `explainAggregate` / `explainAggregateJoin`
  (TypeScript).
* `ExplainResponse` gained `decodes`, for the reason below.

## Why

`EXPLAIN` described the read; the aggregate runs a different one. Grouping
narrows each input's projection to the group keys plus what the aggregates
read — that is what lets `COUNT(*)` over an indexed join touch no row at all —
so `Explain` and `ExplainJoin` on the underlying `Query`/`JoinQuery` describe a
plan the `Aggregate` will not execute. "Did my grouped read go index-only" was
not askable, which was recorded in the 2026-09-13 chain entry and is what this
closes.

## The load-bearing decision

**Running and explaining go through one function.** `plan_grouped`,
`plan_grouped_join` and `plan_grouped_chain` each return the narrowed read
*beside* its plan, and both `SecuredReads::grouped*` and the `explain_grouped*`
entry points call them.

Not tidiness. An `EXPLAIN` that narrows separately is an `EXPLAIN` that can
describe a plan nothing runs, which is the one thing an `EXPLAIN` must never
do — and the drift would be invisible, because both halves would still be
plausible plans for plausible joins. Sharing the function makes the two
impossible to separate rather than merely tested to agree.

`explain_grouped_chain` returns `(Chain, ChainPlan)` for the same reason:
rendering a step needs the step's own query, and using the caller's would
report a limit and offset the grouped read discards.

## What it found

**The wire could not show the difference.** The first version of the
server-side test asserted that the grouped and ungrouped plans differ, and it
failed — the two rendered byte-identical strings:

```
Nested Loop Inner Join  (rows=0 cost=1.24)
  -> Table Scan on authors  (rows=0 cost=1.00)
  -> Table Scan on books  (rows=3 cost=1.00)
```

Correctly, as it turned out. On a table with no usable index, narrowing the
projection changes what is *decoded* and nothing about how rows are *reached*,
and `ExplainResponse` published only the access path. So the new RPC would have
answered "did my grouped read narrow" with a string indistinguishable from the
answer to a different question — on exactly the plans where a reader most needs
to be told.

Hence `decodes`: the plan's `output_columns`, on the wire and in the `Display`
form. It is the field the assertion is now written against, in all four suites.

**The Go client could not name an aggregate's column across a join.**
`Aggregate.Column` was an `Ordinal`, and a bare ordinal resolves against input
0, so `MaxOf(6)` came back as *"an aggregate names column 6 of table `authors`,
which has 3 columns"*. Python and TypeScript both took a qualified reference
from the start. Changed to `Column`; the two existing call sites were in one
test, which had never reached across a join.

## Alternatives rejected

**Extending `ExplainJoinRequest` with a grouping.** Would leave the one-table
grouped case unanswerable, and that case narrows too. Taking the whole
`AggregateQuery` means the message explained is the message that would run,
which is a property, not a convention.

**A `oneof` for `input`/`join` in the response.** Two optional fields with the
invariant stated instead. `@grpc/proto-loader` surfaces an unset message field
as `undefined` either way, so the client code is identical, and a `oneof` in
proto3 would have added a wrapper type to every generated language for a
constraint one sentence covers. Each client asserts the *other* field is unset,
which is the part worth checking.

**Adding a `projection` field rather than `decodes`.** The projection alone is
not what a plan reads: the residual's columns are decoded too, and on this
engine the residual carries the security filter. `decodes` is
`Plan::output_columns` — what the executor will actually pull out of a row,
which is the number an operator is reasoning about.

**Leaving `Explanation::Display` alone and putting `decodes` only on the wire.**
The kernel's own snapshot corpus would then have no way to notice a projection
regression. `plans.txt` moved 63 lines, all of them the appended
`decodes=[...]`, and no plan changed — which is itself the evidence that adding
the field moved nothing.

## Evidence

`cargo test --workspace --no-fail-fast`: green.

Kernel, `grouped_chain_oracle.rs`, three new tests. The interesting one ties
the explanation to *I/O* rather than to another plan: with the same chain and
two groupings, the explanation claims index-only or not, and the same read is
then run over a counting store — zero row reads when index-only was claimed,
non-zero when it was not. Four mutations, each killed by a named test:

| mutation | killed by |
| --- | --- |
| `explain_grouped_join` plans the caller's join | `explaining_a_grouped_join_answers_a_different_question…` |
| `explain_grouped_chain` skips `authorize_explain` | `a_reader_may_group_but_may_not_explain_the_grouping` |
| the grouped chain stops narrowing | that test **and** `a_grouped_chain_reads_only_what_something_downstream_needs` |
| the grouped join stops narrowing | `explaining_a_grouped_join_answers_a_different_question…` |

The authorization test needed a new identity: `r` held `Action::EVERYTHING` and
would have passed with the check deleted. `no-plans` holds `Action::ALL` — the
four data actions, not `Explain` — and is the only identity that can tell a
present check from a missing one. The same gap existed in the server fixture,
where a `grouper` role was added for it.

Server, `multi.rs`, four new tests; two mutations killed (explaining the plain
join instead of the grouped one; `decodes` never reaching the wire).

Clients: Go 2, Python 3, TypeScript 2. Mutations: the client dropping the
grouping before sending it, and the client never reading `decodes` — killed in
each.

**A mutation that survived, and what it changed.** Dropping `group_by` from the
Go request left every case passing. Grouping by nothing narrows *harder* — to
the aggregates' columns and the join keys — so "narrower than ungrouped" is
satisfied by a request that lost its grouping entirely. All four suites now
group by a column that is deliberately *not* a join key and aggregate over
another that is not either, and assert those exact ordinals appear in the
right input's `decodes`. That kills it.

## What this does not do

The demo's `/api/explain` still explains the ungrouped read; the explorer's
chart panel shows a grouped join and its plan panel does not describe it. The
adapters' contract would need a field for the grouped plan, and none of the
three implements one. *(Closed by
`2026-09-14-the-grouped-plan-in-the-demo.md`, the next commit.)*

`AggregateExplainResponse.display` is the sub-plan's text with a `Group by
[...] computing [...]` line prefixed, and that line prints ordinals and the
`Debug` form of each aggregate. It is legible next to a schema and not on its
own; naming columns would need the display side to hold a `TableDef`, which
`JoinExplanation`'s does not.

`decodes` is per input, and for a chain it is per step — but nothing reports the
*joined* ordinal a group key was written in. A reader comparing a group key to
`decodes` has to do the shift themselves, which is the arithmetic the
`ColumnRef` model exists to avoid everywhere else.

No client exposes `ExplainAggregate` inside a transaction except Go, where
`Transaction.ExplainAggregate` exists because the Go session already threaded
the id. Python and TypeScript reach only the non-transactional form; the server
implements both.
