# Every pinned fingerprint was of a table with no decimal and no array

- **Date:** 2026-09-27
- **Author:** Claude Code, closing the larger half of a caveat from
  `ledger/2026-09-20-an-array-on-the-wire-and-in-three-clients.md`
- **Touches:** `crates/slate-server/tests/schema_check.rs`,
  `clients/go/slate/schema_test.go`, `clients/typescript/test/schema.test.ts`
- **Kind:** test

## What changed

Three ports of the schema fingerprint now pin a table carrying both properties
a column can hold beyond its name and type — an `array<string>` and a
`decimal(2)`:

| port | test | value |
|---|---|---|
| Rust | `the_canonical_form_is_pinned_against_an_implementation_in_another_language` | `0xdf01_3a5c_cb68_08c0` |
| Go | `TestTheScaleAndTheElementTypeArePinnedToo` | `0xdf01_3a5c_cb68_08c0` |
| TypeScript | `the scale and the element type are pinned too` | `0xdf013a5ccb6808c0n` |

The Rust test also pins `prices`, the existing decimal fixture, at
`0xdab8_8564_81bc_4a6d`.

And the reference implementation in that Rust test's docstring — which calls
itself "the whole specification" — gained the two lines it was missing:

```python
if ctype == "decimal": out += d(extra)    # the scale
if ctype == "array":   out += s(extra)    # the element type
```

## Why

The caveat said the four implementations are held together only by the live
suites, and named the reason: the three pinned values have neither type in
them. That is sharper than it sounds. Measured, by running the reference
implementation with and without both arms:

| table | with the arms | without |
|---|---|---|
| `docs` | `0xf8bdead5a04b5bb5` | `0xf8bdead5a04b5bb5` |
| `prices` | `0xdab8856481bc4a6d` | `0x6598712619356ade` |
| `shelves` | `0xdf013a5ccb6808c0` | `0x3080aa76df723ab2` |

So any of the four ports could have deleted both arms and every pinned test in
the repository would still have passed. What would have caught it is a client
suite writing to a table with a decimal or an array — real, but it catches the
disagreement at the point where a whole client refuses every request, rather
than at the constant.

The stale docstring is the same defect one level up, and it is why this was
invisible: the transcript is what a fourth port would be written from, and a
port written from it would have disagreed with all three of the others about
any table with a decimal in it.

## Alternatives rejected

**Add an array column to `docs`.** One fixture instead of two, and it would
have pinned both arms in every existing assertion. Rejected for the reason
`common::prices` already records: six test files write `docs`, so widening it
makes those files test a schema change, and the failure presents as their width
assertions rather than as anything about arrays.

**Put `shelves` in `common/mod.rs` beside `prices`.** The precedent points that
way. Rejected because nothing writes to it — it is hashed and never stored — and
a fixture the rest of the crate can reach is a fixture somebody eventually puts
a row in, at which point it needs a table id that does not collide, a grant, and
a place in the catalog. Built inline in the one test that uses it.

**Pin the Python client too.** It is the reference the other three are pinned
against, so a pin there is one port agreeing with itself — the thing the
original test's docstring says is not evidence. What checks Python is its live
suite against a real server.

**Pin one table per port per type, rather than one with both.** More precise
failures: a broken element arm would fail a differently named test from a broken
scale arm. Rejected because a column is never both at once, so one table with
two columns exercises both arms independently, and the mutations below confirm
each is caught on its own.

## Evidence

- Three implementations agree on `0xdf013a5ccb6808c0` for `shelves`: the
  reference transcript run by hand, `slate.schema.fingerprint_of` in the Python
  client, and — after the pins — `fingerprint::of_table`, `TableDef.Fingerprint`
  and `fingerprint()`. The same three agree on `0xdab8856481bc4a6d` for
  `prices`.
- The reference transcript reproduces the two values pinned before this change,
  `0xf8bdead5a04b5bb5` for `docs` and `0x4fdef41322672bf0` for its two-column
  prefix, which is what makes it the specification rather than a paraphrase.
- Six mutations, one per arm per port, recorded as
  `ledger/mutations/20260927T022439-crates-slate-server-src-fingerprint-rs.json`,
  `ledger/mutations/20260927T022754-clients-go-slate-schema-go.json` and
  `ledger/mutations/20260927T022811-clients-typescript-src-schema-ts.json`.
  Every one was caught, and each
  by the new test: the Rust and Go pins name only the new test, and the
  TypeScript scale mutation additionally fails `a decimal's scale is part of
  the fingerprint`, which was already there and is a relative assertion rather
  than a pinned value.
- `cargo test -p slate-server --test schema_check the_canonical_form` — 1
  passed. `GOTOOLCHAIN=local go test ./slate -run TestThe -count=1` — ok.
  `npx tsx --test test/schema.test.ts` — 14 pass, 0 fail.
- `sh scripts/check.sh` — 71 of 71.

## What this does not do

**It pins two tables, not the type lattice.** `vector`, `uuid`, `bytes` and
`f64` appear in no pinned value, and a port that spelled `uuid` as `guid` would
still be caught only by a live suite writing such a column. That is a weaker
gap than the one closed — those types carry no extra state, so the spelling is
the only thing to get wrong and it is one string in a table each port lists
explicitly — but it is the same shape, and saying otherwise would be claiming
the type surface is pinned when two tables of it are.

**The Python client is still unpinned by construction.** It is the reference,
and what checks it is its own suite against a live server. If Python drifts,
all three pins drift with it at the next re-derivation and nothing says so —
the protection is that a Python drift also fails its live suite, which is the
same protection the caveat called insufficient for the other three.

**Nothing stops the docstring going stale again.** It went stale because the
canonical form grew two arms and the transcript did not, and no check compares
them — a guard would have to parse Python out of a Rust doc comment and
execute it against the same fixtures, which is a real thing to want and is not
here. What is here is that the transcript now reproduces five pinned values
instead of three, so the next arm added without updating it breaks two of them.
