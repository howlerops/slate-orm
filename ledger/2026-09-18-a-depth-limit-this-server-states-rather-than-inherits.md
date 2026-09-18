# The wire's deep-nesting refusal was prost's default; it is now this server's, stated, and a compile-time assertion keeps it reachable

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `crates/slate-server/src/convert.rs`, `crates/slate-server/tests/wire.rs`, `README.md`
- **Kind:** security

## What changed

`convert::MAX_EXPRESSION_DEPTH`, 32, and a depth carried through every
recursive arm of `expr_named` and `scalar_named`. Past it the request is
refused by name. A `const` assertion fails the build if the constant is ever
raised to 100 or above.

## Why

The README recorded it as an open item: the conversion functions recurse over
`Expr` and `Scalar` with no counter of their own, and a pathological request is
stopped only by prost's decode recursion limit of 100.

That was *safe* and it was a bad place to leave it, for two reasons that have
nothing to do with the stack.

**It is a dependency's default.** A prost upgrade that raises or removes the
limit changes this server's contract, and the person making that upgrade has no
reason to read `convert.rs`. Inherited safety is safety nobody is maintaining.

**It is not a stated limit.** A caller who hits prost's gets a decode error
about a malformed message, which is true and useless: it names nothing about
which part of the request was too deep, and a client author reading it has no
way to know a limit exists at all, let alone what it is.

## Alternatives rejected

**Set prost's own `recursion_limit` in the build script and leave the
conversion alone.** One line, and it keeps the check in one place. Rejected
because the error stays a decode error — the caller still cannot tell which
field was too deep — and because it moves the limit to build configuration,
where it is even further from the code that recurses.

**Rewrite the conversion iteratively, with an explicit stack.** Removes the
recursion and so removes the question. Rejected as a large rewrite of two
functions whose recursive shape mirrors the types they convert, in exchange for
a property a counter gets for free. The recursion is not the problem; the
absence of a stated bound was.

**Pick a round number like 128, or 64.** Rejected on the point that makes the
whole item worth doing: a limit at or above prost's 100 is a check that never
fires, because prost refuses first. This repository has a written-down history
of checks that never fired — a workflow that had run zero times, a Python suite
that skipped itself. 32 is an order of magnitude above anything a person
writes and comfortably under 100, and the `const` assertion is there so the
next person cannot quietly undo that.

**A test asserting `MAX_EXPRESSION_DEPTH < 100`.** That is what it was, and
clippy correctly called the assertion constant. The answer to a constant
assertion is to make it a *constant* assertion rather than to silence the lint:
a test lets somebody raise the number and find out from a red suite, and
`const _: () = assert!(…)` stops the build with the reason attached. Verified
by setting it to 128 and watching the build fail with exactly that message.

## Evidence

**Six mutations, all killed** — two of them only after the tests were improved:

| mutation | outcome |
| --- | --- |
| the expression depth check is removed | killed |
| a scalar's children do not count as deeper | killed |
| a `CASE` condition restarts the budget | killed |
| the limit is raised to 128 | killed — **at compile time**, once it became a `const` assertion |
| a conjunction's children do not count as deeper | **survived**, then killed |
| a negation's child does not count as deeper | killed |

**The survivor is the finding.** `nested_not` built its chain out of
`Negation`s only, so the `Conjunction`/`Disjunction` arm — a different match
arm, with its own `depth + 1` — was never reached. Dropping the increment
there changed no test. An `And` chain is also the shape a query builder
actually produces, so it was the likelier attack of the two. The helper now
takes a `Wrap` and the test runs all three; the mutation dies.

**The pair of assertions is the case**, in both tests: a chain at exactly the
limit converts, and one deeper is refused with `INVALID_ARGUMENT` and a message
containing "nests more than". Only the second would pass against a limit of
zero, and only the first against no limit at all.

**The one crossing between the two recursions** is a `Scalar::Case`'s branch
condition, which is an `Expr`. They cannot compose into unbounded depth —
`Expr` has no node holding a `Scalar`, so the crossing goes one way — but a
`when` that restarted at zero would let a 32-deep chain of `CASE`s each carry a
32-deep condition, and a limit whose real ceiling is the product of two limits
is not the limit it says it is. Tested from both sides: at the limit it is
refused from inside a `CASE`, one shallower it fits.

`cargo test -p slate-server --no-fail-fast`, `cargo clippy -p slate-server
--all-targets`: green.

## What this does not do

- **It does not measure anything about the stack.** The claim is that the limit
  is stated and reachable, not that 32 was derived from a frame size. A
  measurement would be the wrong tool: the number is chosen to be far below
  both prost's limit and any real query, and a stack-derived number would be a
  platform-specific figure pretending to be a contract.
- **Prost's limit still applies first for anything past 100**, and that is
  fine — this one fires at 32, so a caller reaches it long before.
- **Nothing bounds an expression's *width*.** A flat `And` of a million terms
  is depth 1 and is not refused here; the per-request work and memory limits
  from the daemon's own budget are what covers that, and they are a different
  mechanism for a different shape.
- **The three clients do not know the limit.** A client can build a too-deep
  expression and find out from the server. Publishing it would mean publishing
  a constant clients must restate, which is the thing the fingerprint's module
  doc argues against.
