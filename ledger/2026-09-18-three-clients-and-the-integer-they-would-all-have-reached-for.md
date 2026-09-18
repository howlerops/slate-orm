# The three SDKs are compared on money arithmetic, and an expression they get wrong is the caller's fault rather than the server's

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `examples/explorer` (all three adapters, `CONTRACT.md`, the conformance corpus), `crates/slate-server/src/status.rs`, `crates/slate-server/tests/decimal_wire.rs`
- **Kind:** fix

## What changed

Three group-by expressions in the explorer's `/api/aggregate`, built
independently by the Go, TypeScript and Python adapters:

| name | expression | what it compares |
| --- | --- | --- |
| `discounted` | `books.price - 0.50` | a decimal literal, which takes the column's scale |
| `doubled` | `books.price * 2` | money times a whole number, still money |
| `badPrice` | `books.price + books.year` | **refused** — money plus a count of nothing |

The corpus grew three cases for them, taking it from 89 to 92.

And `KernelError::DecimalScale` got a status code. It had none, so it fell to
the wildcard and reached callers as `INTERNAL`/`UNCLASSIFIED`. It is now
`INVALID_ARGUMENT` with the reason token `DECIMAL_SCALE`.

## Why

**`discounted` is the case worth the most, and it is not about arithmetic.** A
decimal literal has no scale of its own and takes the column's, so fifty cents
beside a scale-2 price is `Decimal(50)` — not the integer `50` that Go,
TypeScript and Python each reach for first, and each spells differently
(`slate.Int(50)`, `int(50)`, `i64(50)`). A client that sends the integer is
*refused* rather than answering differently, which means a three-way comparison
of answers would never have seen it. The corpus compares refusals too, which is
what makes this case work.

**`badPrice` is that refusal on purpose.** All three must surface the same one,
so an adapter that validated locally — or one that let the expression through
and rendered a column of nulls — fails.

**The classification was wrong and a caller could not tell.** `INTERNAL` reads
as "the server broke" and invites a retry; a malformed expression will fail
identically forever. The same reasoning is already written down beside
`InvalidCursor`, which reached the wire the same way.

## Alternatives rejected

**A `/api/decimal-arithmetic` endpoint of its own**, like
`/api/conditional-update`. Rejected because `/api/aggregate` already has the
extension point — a table of named expressions, each a different `Scalar`
family — and a new endpoint would mean three more handlers, three more
contract sections, and a fourth place for the adapters to drift. Money
arithmetic is a *kind of scalar*, which is exactly what that table enumerates.

**A positive case only.** Cheaper and it would have passed. It would also have
missed the point: the thing three clients actually get wrong about decimals is
the literal's type, and that failure shows up as a refusal rather than as a
different number. Without `badPrice` and without the refusal comparison, the
mutation below would have survived.

**Validating the expression in each adapter before sending it.** Tempting —
three clear local error messages instead of one server round trip. Rejected for
the reason the `decade` comment already gives one case up: hand-doing it in the
adapter is the adapter doing the database's job, and it would make the
conformance claim about three adapters agreeing with each other rather than
about three clients agreeing with the server.

## Evidence

**92 cases, the three SDKs agree on all of them.** Two mutations, each
restored:

| mutation | outcome |
| --- | --- |
| Go's `discounted` sends `slate.Int(50)` instead of `slate.Units(50)` | **caught** — the runner reports Go getting `adding or subtracting a decimal and a number that is not one` while node and python return groups |
| Python's `doubled` sends `lit(Units(2))` instead of `i64(2)` | **caught** — python gets `multiplying two decimals: one side is a decimal literal…` while go and node agree |

The second is also the first sighting on the wire of the `Mul` fix from the
kernel commit: before it, `price * Decimal(2)` planned cleanly and returned a
column of nulls, so this mutation would have produced three *different answers*
rather than a refusal, and a reader would have had to work out which was right.

**The classification guard did its job and I had not run it.**
`every_error_is_classified.rs` reads `error.rs` and `status.rs` as text and
fails when a variant has no arm. Reverting the fix and running it gives:

```
these `KernelError` variants have no status code and fall to the wildcard,
where a caller reads them as the server having broken: ["DecimalScale"]
```

CI would have caught this; I found it first because the conformance runner
prints the wire error and `"kind": "internal"` was visible in it. Worth
recording as a process note rather than a code one: adding a `KernelError`
variant in `slate-kernel` means running `slate-server`'s tests, and running
only the crate you edited is not enough when a guard lives downstream of it.

**The arm itself**, which that guard cannot check because it reads text:
`an_inexpressible_decimal_is_the_callers_fault` in
`crates/slate-server/tests/decimal_wire.rs` sends `amount + id` — a scale-2
decimal plus a `u64` — and asserts `INVALID_ARGUMENT` and the `DECIMAL_SCALE`
detail.

`cargo test -p slate-server --no-fail-fast`, `cargo clippy --workspace
--all-targets`, `./run.sh --conformance` (92), `./run.sh --e2e` (22): green.

## What this does not do

- **No client-side knowledge of scale.** Each adapter still writes `Units(50)`
  by hand, having read the schema in `head.toml`. Nothing in any client checks
  that 2 against the server's, and the schema fingerprint does not cover scale
  — so a client that believes `price` is scale 4 sends numbers a hundred times
  wrong and every one of these cases still passes. This is the largest
  remaining hole in the decimal story and it is not closed here.
- **The demo's UI does not show these groupings.** They are in the corpus and
  the contract; the web front end's grouping picker lists the older ones. A
  reader clicking through the demo will not meet a decimal expression.
- **No `sum(price)` anywhere in the corpus.** `/api/aggregate` returns counts
  only, so the one aggregate that returns a decimal — and the one a reader
  would actually write over money — is compared by no client case. The SQL
  front end's `HAVING sum(price)` covers it in the browser; the three SDKs do
  not.
- **`Value::decimal_to_string` is Rust-only.** Go, TypeScript and Python each
  keep their own renderer, which is what the corpus's `rendered` field compares.
  Sharing one would mean a code generator, and three short functions that a
  corpus compares are cheaper than that.
