# Predicate writes in all three clients, and a staleness guard that had never run

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** all three clients, `examples/explorer/`, `.github/workflows/ci.yml`, `README.md`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`DeleteWhere` and `UpdateWhere`, with `returning`, in Python, Go and
TypeScript. Ten tests in Python, nine in Go, nine in TypeScript, and five
conformance cases comparing the three. `WriteResult` grew a `rows` field in all
three.

Separately, and the reason this commit is not only a client commit: Go's
`TestStubsAreFresh` needs `protoc`, skips without it, and **no CI job installed
one**. `protoc` is installed now, and `SLATE_REQUIRE_PROTOC` turns the skip
into a failure.

## Why

The server had the RPCs and nothing could call them. Each client follows the
shape it already had rather than a new one: Python gets fluent builders beside
`Query`, Go gets plain structs beside `Query`, TypeScript gets plain interfaces.
Three idioms, one protocol.

**The guard is the finding.** The previous entry explained Go passing CI as "it
generates from the proto at build time and has nothing committed to go stale".
That was wrong, and I did not check before writing it — the entry now carries
the withdrawal. Go commits its stubs like Python does, and it has a guard that
regenerates and compares byte for byte. The guard's own comment said "CI
installs `protoc` and this runs there for real". `grep -n protoc
.github/workflows/ci.yml` returns nothing, and did then.

So the check had never run outside a laptop. Its docstring records that the Go
stubs had already gone stale once and "stayed stale through two rounds of
work"; the guard written to stop that recurring has been skipping ever since,
and the stubs went stale again in this very branch — missing `Assignment`,
`DeleteWhereRequest` and `UpdateWhereRequest` — with the Go job green. That is
"a skip is green" from `CLAUDE.md`, in the exact shape the file warns about,
found by accident because `go build` failed on a symbol the stubs did not have.

## Alternatives rejected

**Install `protoc` in CI and stop there.** Fixes today. Leaves the same hole
open for the next workflow edit that drops the step, and that edit would be
silent in precisely the way this one was. `SLATE_REQUIRE_PROTOC` makes the
absence loud where it matters while keeping the skip for a laptop with no
`protoc` — which is the one case where a hard failure really is about the
machine.

**Make the test fail unconditionally without `protoc`.** Simpler, one fewer
concept. It makes `go test ./...` fail on a developer machine for a reason that
is not about the repository, which is how a check gets commented out.

**A fluent builder in Go and TypeScript too, for symmetry across the SDKs.**
Rejected for the reason each client already encodes: Go and TypeScript describe
a read with a plain struct or interface, and a predicate write that was built
fluently would be the only writer-shaped thing in those files. Symmetry between
languages is worth less than symmetry within one.

**One `PredicateWrite` type with an optional assignment list.** Fewer types. It
makes "a delete with assignments" expressible and refused at runtime, where two
types make it unspellable.

**Reuse the fixture rows in the conformance cases.** Less setup. The runner
drives all three adapters against one database, so a case that deleted a
fixture row would make every later case depend on which SDK ran first, and the
corpus would stop being re-runnable. Each case seeds its own id range, like the
transaction probe already does.

## Evidence

Python 202 passed, Go passed, TypeScript 99 passed, **77 conformance cases with
the three SDKs agreeing on all of them**. `site/check/docs.py` clean.

Ten mutations across the three clients:

| mutation | result |
| --- | --- |
| python: `returning` dropped on a delete | killed |
| python: the filter dropped on a delete | killed |
| python: the schema claim dropped | **survived** → killed |
| python: an assignment silently dropped | killed |
| go: `returning` dropped on a delete | killed |
| go: a nil filter becomes an empty expression | killed |
| go: returned rows dropped | killed |
| ts: `returning` dropped on a delete | killed |
| ts: the assignments sent empty | killed |
| ts: returned rows dropped | killed |

The survivor is the same defect the server-side round had, in a second place:
dropping `schema=` from Python's two predicate-write methods changed no answer
in eight tests. It matters more here than elsewhere, because the ordinals in
the *predicate* resolve against the caller's idea of the table — a claim that
disagrees means the filter selects rows by a different column than the caller
wrote, and then deletes them.
`test_the_schema_claim_rides_on_a_predicate_write` fails by name under that
mutation, checked by re-applying it.

The `SLATE_REQUIRE_PROTOC` behaviour was checked both ways: with `protoc` on
`PATH` the test passes, and with a `PATH` holding only Go's bin directory it
fails with the message naming the variable. Without the variable set, the same
`PATH` skips.

**Two adapter bugs the corpus caught, both mine.** The Node adapter returned
`affected` as a `bigint` and `JSON.stringify` refuses one outright; the Python
adapter called `row.values()` where `values` is a property. Go was right first
time. Neither would have been found by a single-SDK test — the Go answers were
correct and complete, so only comparing three implementations showed two of
them failing. That is what the corpus is for, and it is the third time it has
paid for itself in this branch.

## What this does not do

`returning` on a predicate write that matches a great many rows sends them all.
There is no cap, no streaming, and no measurement of where that hurts — the
response is a single message, so a delete of a million rows would build a
million-row response in memory on both sides. The server-side entry noted the
same bound on the kernel's side; this adds a wire-shaped version of it and
still does not measure it.

~~No client exposes predicate writes in a *transaction* except Python, which
gets it free from its session object. Go and TypeScript take a `Session`, which
already carries the transaction if there is one, so the capability is there —
but only Python has a test that rolls one back.~~

**Withdrawn before this entry was committed, and it was a real gap rather than
a testing one.** Go's `Transaction` and TypeScript's are their own types, not a
`Session` carrying a flag, so a method added to the session is simply absent
from the transaction: neither client could do a predicate write inside a
transaction at all. Writing the missing test is what found it —
`tx.DeleteWhere undefined`. Both now have `DeleteWhere` and `UpdateWhere`, and
a test that rolls one back and checks no sequence comes home until commit. The
guess that the capability was already there is exactly the kind this branch has
now been wrong about twice; the lesson is the same both times, which is to run
the thing rather than reason about the type.

The demo frontend does not show predicate writes. The adapters serve
`/api/predicate-write` and the corpus compares it; the SolidJS app has no
button for it, so the feature is visible to the conformance runner and not to a
visitor.
