# Five column types no pinned table carried, in four implementations of one hash

- **Date:** 2026-09-28
- **Author:** Claude, continuing the pass over what the caveat tracker still listed
- **Touches:** `crates/slate-server/tests/review_fingerprint.rs`, `clients/go/slate/schema_test.go`, `clients/typescript/test/schema.test.ts`, `clients/python/tests/test_fixture.py`
- **Kind:** fix

## What changed

A fourth pinned table, `readings {id u64, ok bool, raw bytes, weight f64,
tag uuid, point vector}`, whose fingerprint `0x9eb9cc433c353eb1` is written
down independently in all four implementations. The Python test swaps each of
the five types for `STR` in turn and asserts the number moves.

And a guard against the *shape*: `test_every_value_type_appears_in_a_pinned_table`
derives the covered set from the pinned tables, so adding a member to
`ValueType` fails until some pinned table carries it.

## Why

The `SchemaCheck` fingerprint has four implementations — Rust, Go, TypeScript,
Python — and they are held together by constants each writes down separately.
Two tables were pinned: `docs` is u64/str/i64 and `shelves` is
u64/array/decimal. Between them, **five of the ten column types appear in no
pinned value**: `bool`, `bytes`, `f64`, `uuid` and `vector`.

A port that misspelled one of those in its hash produced a fingerprint the
server refuses — for exactly the tables that use that type, and no others. The
failure is a client that works until somebody adds a `uuid` column. That is
the residual `ledger/2026-09-20-an-array-on-the-wire-and-in-three-clients.md`
recorded: two tables are pinned, not the type surface.

## Alternatives rejected

**Five tables, one per type.** Localises a failure — "this port's `uuid` is
wrong" rather than "this port disagrees" — and costs five constants in four
files, twenty numbers to keep in step. Rejected because the fingerprint hashes
each column's type name in turn, so any single wrong type moves the one
number; localisation is what the *diff* gives you when the test fails. The
Python test's loop buys the localisation where it is cheap.

**Generate the pinned tables from the type enum, so a new type is pinned
automatically.** The tidier answer and the wrong one here: the whole value of
these constants is that they were computed by a *different* implementation and
written down by hand. A generated expectation is the port agreeing with
itself, which `TestTheFingerprintMatchesTheServers` already argues is not
evidence.

**Pin the Python client against the Rust server instead of its own output.**
Considered because Python is the reference the other three quote, so its own
assertion is self-agreement. Not taken, and the reason is that it is already
resolved the other way round: Rust, Go and TypeScript each write
`0x9eb9cc433c353eb1` down separately, so Python asserting it is Python pinned
against three independent ports. Inverting the direction would move which one
is unchecked, not remove it.

## Evidence

The constant was **not** taken from any implementation under test. It is the
Python client's, computed here, with the recipe verified first by reproducing
both existing pins exactly:

```text
docs     0x97c3c1256af4cfdb   (matches the pin in Go and TypeScript)
shelves  0xdf013a5ccb6808c0   (matches the pin in Go and TypeScript)
readings 0x9eb9cc433c353eb1
```

Each of the five types swapped for `str` in turn moves it, which is why one
table pins five types:

```text
bool   -> str : 0x5319b40a56b90adc
bytes  -> str : 0x3ebdc3b368792ace
f64    -> str : 0x3109883fb7d3de75
uuid   -> str : 0x9cd85f08a94cc695
vector -> str : 0xa4061a3a6944f5a7
```

Rust, Go and TypeScript then each produce `0x9eb9cc433c353eb1` from a table
built in their own syntax, and all three pass.

**Seven mutations, six caught and one survivor now closed** —
`ledger/mutations/20260928T190303-clients-go-slate-schema-go.json`,
`ledger/mutations/20260928T190521-clients-python-tests-test-fixture-py.json` and
`ledger/mutations/20260928T190557-clients-python-tests-test-fixture-py.json`:

| mutation | caught by |
|---|---|
| `TypeVector` spelled `"vec"` | `TestTheRemainingTypesArePinnedToo` |
| `TypeUUID` spelled `"guid"` | the same |
| `TypeBool` and `TypeBytes` swapped | the same |
| a type drops out of every pinned table | 2 Python tests |
| **the coverage guard's computation replaced by `[]`** | **nothing — survivor** |
| the computation returns nothing (after the fix) | `test_the_coverage_guard_fires_when_a_type_is_unpinned` |
| the computation treats every type as covered | the same |

**Only the new test fires for the first three**, which is the demonstration
rather than a coincidence: the two pre-existing pins cover no column of those
types, so all three would have survived the Go suite's fingerprint tests
before this.

**The survivor is the more useful finding.** The coverage guard as first
written computed `missing` inline and asserted it empty — and replacing the
computation with `[]` left the suite green. That is exactly the "a check that
never fires is a check nobody has debugged" shape `CLAUDE.md` names, produced
here while writing a guard *against* that shape. The computation is now
`_unpinned_types(*tables)`, and
`test_the_coverage_guard_fires_when_a_type_is_unpinned` calls it with only
`docs` pinned and requires it to name the seven types `docs` does not carry.

Suites: `go test ./slate` green, `npm test` 196 pass, `pytest` green,
`cargo test -p slate-server --test review_fingerprint` 5 pass, `gofmt -l`
clean.

## What this does not do

**It does not pin a type against the *server's* acceptance.** These four
numbers agreeing proves the four implementations compute the same hash; that
the hash is the one `fingerprint::check` wants is proved by the live client
suites, which is the arrangement that predates this entry and is unchanged.

**`decimal` and `array` are pinned only through `shelves`.** Their element
type and scale are what that table exists for, and this one deliberately has
neither — a column is never both, so covering ten types would have meant a
table that is not a plausible schema. Two tables and no overlap is a choice
about readability, not a claim that one table could not have held everything.

**The coverage guard is Python's alone.** `test_every_value_type_appears_in_a_pinned_table`
reads `ValueType`, so it fails when a *Python* type arrives unpinned. A type
added to the Go, TypeScript or Rust enum and not to Python's would not trip it
— though such a type could not reach a fingerprint the server accepts either,
since the server's own list is the one that matters. One guard in the port
that is the reference, rather than four that can disagree about what the list
is.

**Nothing checks that the three other ports pin the same *tables*.** Each
writes down `0x9eb9cc433c353eb1` beside a table it builds itself, and a port
that pinned the right number against the wrong table would pass. The tables
are three lines each and visibly the same shape; nothing enforces it.
