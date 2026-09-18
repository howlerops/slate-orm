# All three clients can now hold money and refuse a lost update

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5), working `docs/orm-comparison.md`'s W list
- **Touches:** all three clients and their suites, `crates/slate-serverd`
  (`config.rs`, `schema.rs`, `value.rs`), `crates/slate-schema`
- **Kind:** feature

## What changed

A decimal surface in each client — `Units` in Python and Go, `units()` in
TypeScript — carrying a count of the column's smallest unit, plus a renderer
that takes the scale as an argument. Each client's table declaration gains an
optional `scale`. Each client gains a conditional update: `update(...,
expected=...)` in Python, `UpdateIfUnchanged` / `updateIfUnchanged` in Go and
TypeScript.

And `slate-serverd` learned to *declare* a decimal column at all: `type =
"decimal"` with a required `scale`, refusing a decimal without one and a scale
on anything else. Until this, a decimal column could be built in Rust and not
configured in the binary anybody actually runs.

## Why

The protocol grew both fields earlier today and no client spoke either. That is
half a feature: the gap `docs/orm-comparison.md` records is "reachable from the
Rust ORM and from none of the three clients", and the wire alone does not close
it.

The daemon's configuration was the part that was easy to miss. `value_type`
listed eight types and `ValueType` has nine, so a `[[tables]]` entry saying
`type = "decimal"` was refused with "is not a type" — which means the feature
existed in the library and was unreachable from the deployment. It was found by
trying to write a test fixture.

## Alternatives rejected

**Accepting a `decimal.Decimal` (and a JS `number`, and a Go `float64`) and
scaling it.** The obvious ergonomic choice, and every ORM with a decimal type
does it. It requires knowing the column's scale at the moment of conversion,
and the protocol publishes no schema — so the client would have to trust its
*local* declaration, and a local declaration that is wrong turns 12.50 into
1250 or 0.12 with nothing anywhere reporting it. The fingerprint does not hash
the scale (a scale addresses no column), so even the schema check would not
catch it. Refusing is the same choice `to_value` already makes for a bare
`int`, for the same reason, and the refusal names `Units`.

**A client-side `Decimal` type that carries units *and* scale**, so a value is
self-describing inside the client even though the wire's is not. It would make
rendering safe without a second argument. It also invents a second place a
scale lives, which then has to be checked against the column on every write, and
the check it would need is exactly the one the protocol refuses to make
possible. `Units` plus `scale_of` keeps one source and makes the reader pass it.

**Defaulting a decimal column's `scale` to 0** in the daemon's config, the way
every other optional field defaults. It is the most dangerous default available
here: a column that meant scale 2 and got 0 stores every value a hundred times
too large, is internally consistent, passes every test, and is discovered by an
accountant. Required instead, with the error saying why.

**Accepting a `scale` on a non-decimal column and ignoring it.** Harmless in
the data and harmful in the head: somebody wrote it because they believe that
column holds a fixed-point number, and silence confirms the belief.

**A float default for a decimal column** — `default = 12.50` in TOML. Rejected
because TOML parses it as a binary double and rounds it before the config code
ever sees it, which is the exact loss a decimal type exists to prevent. The
default is written as its units, `1250`.

**One spelling of the conditional update across all three clients.** Python's
`update(table, rows, expected=...)` is the natural one and Go and TypeScript
cannot have it: both `Update`s are variadic over their rows. `UpdateIfUnchanged`
taking `RowUpdate{Row, Was}` pairs is the Go and TypeScript answer, and pairing
is better than two parallel arrays anyway — the wire's shape, two repeated
fields that must match in length and order, is exactly the shape a caller gets
wrong. The asymmetry is real and is documented in all three READMEs rather than
smoothed over.

**Adding the decimal column to `docs`** in the Go and TypeScript harness
configs rather than a `prices` table per test file. Every other test in those
suites asserts against `docs`, and widening it makes this change show up as
their failures. Both suites already take extra tables per file; this uses that.

## Evidence

24 Python tests, 11 Go, 11 TypeScript, all against a real node. The
`slate-serverd` configuration gains four unit tests, including both refusals.

**Twenty-three mutations, twenty-one killed outright and two that were not.**

Python, nine, all killed by a named test:

| mutation | killed by |
| --- | --- |
| `Units` encodes as `int64_value` | `test_units_encode_to_the_decimal_arm` |
| a bare int in a decimal slot is not coerced | `test_a_bare_int_takes_the_declared_type_of_its_slot` |
| a decimal comes back as a plain `int` | `test_a_decimal_comes_back_as_units_and_not_as_an_int` |
| a `Decimal` gets the generic refusal | `test_a_python_decimal_is_refused_with_a_reason` |
| `expected` is built and never sent | 3 tests |
| the client's arity check removed | `test_a_short_expected_is_refused_by_the_client` |
| the scale is dropped when rendering | `test_rendering_against_a_scale` |
| a negative renders without its sign | `test_rendering_against_a_scale` |
| `scale_of` answers for a non-decimal | `test_the_declared_scale_is_readable_and_only_for_a_decimal` |

Go, seven, and TypeScript, seven — the same list either side of the wire. Two
findings came out of them.

**The first is a fixture that hid a kill.** The Python module seeded its rows
in an `autouse` fixture, so the mutation that made `Units` encode as an `int64`
was caught by the *server refusing the seed* — every test in the module errored
in setup rather than asserting. That is the suite falling over, not a named
test failing, and it would have hidden a real regression in the pure encoding
just as well. The fixture is now requested by the tests that need a server, and
the encoding tests run without one.

**The second is the same shape as the request id's, and it appeared twice.**
Deleting the *transaction* path's `expected` left Go and TypeScript entirely
green. Both had a transaction test, and both tested the happy path — an
unconditional update of an unchanged row and a conditional one produce
identical outcomes, so the test could not tell them apart. Only a *stale* row
distinguishes them.
`TestAStaleConditionalUpdateInsideATransactionIsRefused` and its TypeScript
twin are the missing tests, and they kill it.

**A measurement that corrected a comment.** Go's renderer said it took the
magnitude as `-uint64(v)` "so that `math.MinInt64` has a magnitude that fits,
which `-v` does not". The second half is wrong: Go defines signed negation as
two's-complement wrapping, so `uint64(-v)` and `-uint64(v)` produce identical
bits for every `int64` — measured across the range, including both extremes.
Mutating one spelling to the other is an *equivalent mutation* and survives,
correctly. The comment now says what is true: the magnitude must be a `uint64`
because `int64` has no room for `MinInt64`'s, and which side the conversion
sits on does not matter.

Green: Python 272 (`ruff` and `ty` clean), Go all, TypeScript 146,
`slate-serverd` 214. `gofmt`, `go vet`, `tsc --noEmit`,
`cargo fmt --all --check` and `clippy --workspace --all-targets` under
`-D warnings` all clean.

Disk hit ENOSPC twice more (the linker error `CLAUDE.md` describes); the
ledger's dedup snippet freed 2.64 GB and 3.33 GB.

## What this does not do

- **Nothing checks a client's declared scale against the server's.** The
  fingerprint deliberately does not hash it, because a scale addresses no
  column — so a client that declares `scale=2` against a `scale=4` column
  reaches the right column and renders every value a hundred times too small,
  for ever, with no error at any layer. This is the sharpest edge in the
  feature and it is the price of a protocol that publishes no schema. Writing
  it down is all this change does about it.
- **No decimal arithmetic.** `Scalar` has no decimal operations in any client,
  so a computed column cannot add two of them, and the SQL front end has no
  decimal literal. Both are on the README's open list.
- **No three-SDK conformance case yet.** The conformance corpus and the
  explorer's three adapters do not exercise either feature, so "the three
  clients agree" is asserted by three separate suites rather than by comparing
  their answers. That is the next commit.
- **A conditional update is one round trip per row.** Unmeasured: the cost is a
  read per row where `update_many` reads in one wave, which is arithmetic, and
  no benchmark here exercises it.
- **Python's spelling differs from the other two**, as above. A caller porting
  between them has to notice.
