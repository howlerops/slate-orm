# A grouped chain on the wire, closing a refusal I had just made false

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `crates/slate-server/src/{convert,service,session}.rs`,
  `crates/slate-server/tests/multi.rs`, all three clients' tests, `README.md`
- **Kind:** feature

## What changed

`GroupedSource::Chain`, dispatched at both the transactional and the replica
read paths. Four refusal tests — one Rust, one per client — rewritten into tests
that the thing works.

## Why

The previous commit taught the kernel to group a chain and left the server
saying "the kernel groups a two-table join, and grouping a chain is not built".
That sentence became false the moment it landed, and the test asserting it
would have passed forever while protecting nothing.

This repository's standard is that stale documentation is worse than none
because it is read as current. A stale *refusal* is worse still: it is a
machine-enforced statement that a working feature does not exist, and the four
tests pinning it would have kept it that way.

## Alternatives rejected

**Leaving it for the next protocol pass**, which is what the previous commit's
"what this does not do" proposed. Wrong on reflection: the gap between the two
commits is exactly the window where someone reads the refusal and believes it,
and the change turned out to be one enum variant and two match arms.

**Folding `Chain` into `GroupedSource::Join` with a `Vec<TableId>`.** The kernel
has two entry points — `group_by_join` narrows each side's projection,
`group_by_chain` cannot — so the call site has to choose anyway. Making the
variant carry the distinction puts the choice next to the reason for it.

**Deleting the refusal tests.** They were the only coverage of the three-input
path. Rewritten into "a three-table chain, grouped, agrees with the kernel",
which is the same path asserting the opposite outcome.

## Evidence

941 Rust tests, 138 Python, 30 Go, 37 TypeScript — all passing. `cargo fmt`,
`cargo clippy --workspace --all-targets`, `go vet`, `gofmt`, `tsc --strict` and
`ruff` all clean.

The wire test is a differential: `a_grouped_chain_over_the_wire_agrees_with_the_kernel`
runs the same chain and grouping through `group_by_chain` in process and through
the daemon, and requires the same groups. The head node implements no grouping
of its own, so a disagreement is a conversion bug.

## What this does not do

`EXPLAIN` still cannot describe a grouped chain, or a grouped join. A caller
wanting to know whether their grouped read went index-only has no way to ask —
and for a chain the answer is always no, since the projection is not narrowed.

The three clients can now *reach* a grouped chain, and only the Go and Python
suites exercise a genuine three-table one; the TypeScript case joins `books` to
itself, because that fixture has two tables. It is the same code path.
