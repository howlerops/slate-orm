# The wire learns the two things the kernel had and no client could reach

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5), working `docs/orm-comparison.md`'s W list
- **Touches:** `crates/slate-server` (`records.proto`, `convert.rs`,
  `service.rs`, `session.rs`, `tests/`), `clients/python/testserver`
- **Kind:** feature

## What changed

`Value` gains a tenth arm, `decimal_value`, an `int64` count of the column's
smallest unit. `UpdateRequest` gains `repeated Row expected`, the rows as the
caller last saw them; when it is set the server dispatches to the kernel's
`update_if_unchanged` instead of `update_many`, on all three write paths — the
lone RPC's autocommit, the session actor inside a transaction, and a batch
operation. An `expected` that is neither empty nor exactly as long as `rows` is
refused before anything is written. Two test fixtures gain a `prices` table
with a decimal column at scale 2.

## Why

`docs/orm-comparison.md` recorded these as the largest remaining gap, and both
in the same shape: built in the kernel, exercised by the Rust ORM, reachable
from none of the three clients. `Value::Decimal` had been on the wire's
`_`-arm since it was added — `value_to_proto` turned it into the string
`<unrepresentable decimal>`, and `tests/decimal_wire.rs` existed to *pin that*,
with a comment saying the tests should fail when the field arrived. They did.
`update_if_unchanged` had no wire form at all, so the only optimistic-concurrency
story for a remote caller was "read, decide, write, and hope".

The two shipped together because they are used together. A conditional update
exists for a read-modify-write on a running total; a decimal column is what a
running total that must not drift is stored in.

## Alternatives rejected

**Sending a decimal as `int64_value` and letting the schema sort it out.** No
new field, no regeneration in three languages. It is wrong because the kernel's
ordering is type-first: `Value::I64(1250)` and `Value::Decimal(1250)` are
different values, they do not compare equal, and a predicate built from a row
read back would select nothing. `a_decimal_is_not_an_integer_on_the_wire` is
that argument as a test.

**Carrying the scale in the value** — `{ units, scale }` rather than a bare
integer. It makes a value self-describing, which is genuinely nicer for a
client rendering one. Rejected because it lets a client and the catalog
disagree about what a stored number means, which is the single thing a decimal
type exists to prevent: two writers at different scales and the column holds
numbers that are not comparable. The kernel put the scale on the column for
that reason and the wire keeps it there.

**`sint64` rather than `int64`.** Zigzag wins on negative units and loses on
positive ones, and prices, balances and quantities are mostly positive. The
deciding argument is not the varint though — it is that `int64_value` sits one
arm above and a reader comparing the two should not have to know why they
differ.

**A version column for the conditional update**, which is what every ORM that
has this feature uses and is far cheaper to compare — one integer against a
whole row. Rejected in the kernel already and the wire follows: a version
column only detects changes made by writers who remembered to bump it, so it is
a convention every call site has to keep rather than a property of the data.
Comparing the whole row detects every change, needs no schema support, and
works on tables that already exist. It costs nothing extra in round trips
because `update` reads the row anyway to enforce the row policy.

**A separate `UpdateIfUnchanged` RPC.** Cleaner as a signature, and it would
have made "conditional" visible in the method name a request log prints. It
doubles the write path: the same schema check, the same table authorization,
the same three dispatch sites, twice, for a difference that is one optional
field. The nineteen request messages are already the protocol's largest cost.

**`optional Row` per row, or a parallel `repeated bool`.** Both say "this row
is guarded and that one is not" in one request. Nothing wants that: a caller
doing a read-modify-write guards all of them or none, and a mixed request is
much more likely to be a bug than an intent. `repeated Row expected`, empty or
full length, has exactly the two states that are real.

**Zipping `rows` and `expected` and stopping at the shorter**, which is what
`zip` does for free and needs no check. It turns a caller's mistake into a
*silent partial condition*: five rows and three expectations means two rows
written unguarded, which is precisely the lost update the field exists to
catch. `expected_must_name_one_row_per_update` refuses it with the two counts
in the message.

**Batching the conditional update** the way `update_many` batches the plain
one. There is no `update_many_if_unchanged`, and writing one means deciding
what happens when row three of a wave fails after rows one and two were
applied. The answer is "refuse the statement", which the row-at-a-time loop
already gives, one round trip per row later. The caller who wants the wave
sends an unconditional update; the caller who wants the check pays for it.

**Appending a decimal column to `docs` or `sales`** rather than adding a
`prices` table to both fixtures. Six test files write those tables, and their
width assertions would have turned into tests of this change. The relationship
fixtures set the precedent and say so where they are declared.

## Evidence

`crates/slate-server/tests/decimal_wire.rs`, rewritten from the pins it
replaces: 16 tests, four over the conversion in isolation and twelve through a
real socket. The conditional update is exercised on all three write paths
separately, because a feature wired into one and not the others is this
repository's recurring shape.

Six mutations, five killed outright and one that was not:

| mutation | outcome |
| --- | --- |
| `apply` ignores `expected` entirely | KILLED (4 tests) |
| the session actor ignores `expected` | KILLED (2 tests) |
| the arity check removed | KILLED `expected_must_name_one_row_per_update` |
| a decimal goes out as `int64_value` | KILLED (4 tests) |
| a decimal comes back as `Value::I64` | KILLED (4 tests) |
| **the session actor's loop does not stop at a refusal** | **SURVIVED** → killed |

The sixth is the finding. The actor loops over the rows and replies with an
outcome; without the `break`, the *last* row's outcome is what the caller
hears. A stale first row and a current second row would therefore report
success — the lost update, reported as a successful conditional update, which
is worse than not having the feature. Every test was green because the only
multi-row case ran on the autocommit path and the only transaction case had one
row. `a_stale_first_row_refuses_the_rest_inside_a_transaction` is the missing
test, and it puts the stale row *first* on purpose.

`cargo test -p slate-server --no-fail-fast`: 21 binaries, all green (274
tests). `cargo fmt --all --check` clean. `RUSTFLAGS=-Dwarnings cargo clippy
--workspace --all-targets` clean — after it caught a `redundant_guards` on the
first version of the dispatch, which was right: the match guard is now an `if`
inside one arm and reads better for it.

Disk hit ENOSPC once (the linker error `CLAUDE.md` describes); the ledger's
dedup snippet freed 3.33 GB.

## What this does not do

- **No client speaks either of these yet.** This is the protocol and the server
  only. The three SDKs, their fixtures and the conformance corpus are the next
  commit; until it lands, the gap the comparison records is half closed and
  should be read that way.
- **No decimal arithmetic anywhere.** `Scalar` has no decimal operations, so a
  computed column cannot add two of them, and the SQL front end has no decimal
  literal. Both are on the README's open list and neither moved.
- **The wire carries no scale, ever.** A client renders a decimal by reading
  the scale off the column it declared locally. Nothing checks that the two
  agree, and the schema fingerprint deliberately does not hash the scale —
  a scale addresses no column, so a client with it wrong still reaches the
  right one and prints the wrong number. That is a real hole and it is the
  price of not publishing a schema.
- **A conditional update is one round trip per row.** Measured in nothing: the
  cost is a read per row where `update_many` reads in one wave, which is
  arithmetic rather than a measurement, and no benchmark here exercises it.
- **`delete_if_unchanged` does not exist**, on the wire or in the kernel. The
  README lists it; a conditional delete is a different decision (what does
  `expected` mean for a row you are removing?) and it was not made here.
