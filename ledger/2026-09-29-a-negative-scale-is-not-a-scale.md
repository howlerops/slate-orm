# A negative scale is not a scale, and Go does not overflow at eighteen

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `clients/{go/slate/{value.go,schema.go},typescript/src/value.ts}`, `crates/slate-orm/tests/money.rs`, the three explorer adapters, `examples/explorer/CONTRACT.md`
- **Kind:** fix

## What changed

Two things the previous entry left open, and one thing it got wrong.

**The shared decimal table reaches scale 18.** Five rows added to the list all
three adapters render: `(1250, 6)`, `(-75, 9)`, and the two `i64` extremes plus
`1250` at 18, which is `slate_schema`'s `MAX_SCALE`. Twelve rows became
seventeen and the three SDKs still agree on every one — a **null result**, and
the one worth having, because nothing above scale 4 had been compared. Each
client's own suite gained the same three rows, and `slate-orm` gained
`rendering_is_exact_at_the_schema_s_maximum_scale`.

**A negative scale is refused by all four renderers.** Rust always did, by
type. Python always did, with `ValueError`. Go's `StringWithScale` took an
`int` and rendered a negative as scale 0; TypeScript's `unitsToString` clamped
with `Math.max(0, …)`. Go's argument is a `uint8` now — unrepresentable, the
same refusal Rust has — and TypeScript throws `RangeError`. `ColumnDef.Scale`
is a `uint8` too, so the type is consistent from the schema to the renderer.

**And the caveat that asked for the first of these was wrong about why.** It
said a renderer "that broke at scale 18 — where `10**scale` leaves the range of
an `int64` divisor in Go — is invisible", and that Go "would overflow silently".
Measured: `10^18` is 1.0 × 10¹⁸ against `i64::MAX`'s 9.22 × 10¹⁸, so 18 is
exact. So is 19 — `10^19` wraps negative in the `int64` loop and the wrap is
*undone* by the `uint64` conversion two lines later, because `10^19 < 2^64`. 20
is the first wrong answer, and a scale of 20 cannot come from a column. **Go
has no reachable overflow here.** The gap the caveat named was real; the
mechanism it named was not.

## Why

Three caveats from `ledger/2026-09-29-twelve-decimals-and-one-thing-the-three-do-not-agree-about.md`:

> **Scale above 4 is untested.** Nothing in the demo declares one and no row
> here uses one, so a renderer that broke at scale 18 … is invisible.

> **The negative-scale divergence is recorded, not resolved.** Open, with the
> three behaviours measured. Whoever picks a side has to change a shipped
> client's contract on an argument, not on a failing case.

That second one is the honest description of the position it left, and it is
why this took a measurement first. The argument that decided it was already in
this repository, three files from the code it decided against —
`ColumnDef.Scale`'s own comment:

> a wrong scale "reads the *right* column and renders every value a power of
> ten out, for ever, with nothing anywhere reporting it"

Scale 0 *is* such a value. The clamping comment in both clients said a caller
"wants a number they can see is wrong, not a crash in a log line", and the flaw
is in "can see": `1250` rendered from 1250 units at scale −2 looks like a
perfectly good number. The lenient side produces exactly the failure the strict
side of the same codebase already argues is the worst one available.

## Alternatives rejected

**Make Python clamp too, and settle on lenient.** Symmetric, one-line, and the
side three of the four surfaces were already on. Rejected on the
`ColumnDef.Scale` argument above: a silently wrong number in a currency column
is worse than a refusal in a render path, and the repository had already
written that down.

**Panic in Go.** The direct translation of Python's `raise`, and the thing the
comment being replaced specifically argued against — correctly. A rendering
helper that aborts the process because a caller passed −1 is a worse failure
than the one it prevents.

**Return `(string, error)` from Go.** Honest and loud, and it changes the call
shape at every site for a condition that cannot arise once the argument is a
`uint8`. Six call sites here; the cost is not the edit, it is that every future
caller writes error handling for an impossible case, which teaches the reader
that it is possible.

**Leave the Go signature `int` and refuse at run time by returning a marker
string.** `"<bad scale>"` in a table cell. It is the clamping failure with a
different costume: still a rendered value, still flowing into a report.

**A `uint8` in TypeScript.** There is not one, and `number` with a run-time
check is as close as the language gets. That is why the four refuse in three
different ways rather than one, and why the negative case is in each client's
own suite rather than in the shared conformance table: a refusal is not a
rendering, and Go's cannot even be written down as a table row.

**Adding scale 19 or 20 to the shared table.** They would make the three
disagree — the Rust and Go divisors give arbitrary answers up there while
Python and TypeScript stay exact — and the disagreement would be a bug report
about a scale no column can declare. The Rust test pins one such answer at 19
so the saturation is *recorded*; the shared table stops at the schema's ceiling.

**Striking the wrong sentence in the previous entry.** This repository's
withdrawal idiom is `~~…~~` around the paragraph plus a `**Withdrawn, …**` note
below, and it was not used here: the caveat itself was true — scale above 4
*was* untested — and only its parenthetical reasoning was wrong. Striking the
paragraph would erase a caveat that is being closed with a witness, and would
report the gap as never having existed. The correction is stated here instead,
which is what an append-only ledger is for.

## Evidence

- **The measurement that withdrew the claim.** Go's renderer, transcribed and
  run over scales 0, 2, 4, 6, 9, 18, 19, 20 with `1250`, `-75` and both `i64`
  extremes. Exact through 19; at 20, `i64::MAX` renders
  `1.01457092405402533887` against a true `0.09223372036854775807`.
- `cargo test -p slate-orm --test money`: **5 passed, 0 failed.**
- `clients/go`: `go test ./...` — **ok**, 12.9s.
- `clients/python`: `python3 -m pytest -q` — **343 passed.**
- `clients/typescript`: `npm test` — **200 pass, 0 fail.**
- `examples/explorer ./run.sh --conformance`: **140 cases, the three SDKs agree
  on all of them** — with the five new rows in, so the agreement above scale 4
  is measured rather than assumed.
- **Ten mutations across four files, nine caught, one a recorded survivor.**
  - `ledger/mutations/20260929T035004-crates-slate-orm-src-field-rs.json`
    (`clean`, 2): wrapping instead of saturating, and a padding width that
    stops following the scale — both caught by the new scale-18 test.
  - `ledger/mutations/20260929T035202-clients-typescript-src-value-ts.json`
    (`clean`, 4): the refusal deleted, each half of the condition deleted
    separately, and the padding width.
  - `ledger/mutations/20260929T035401-clients-go-slate-value-go.json`
    (`clean`, 2) and
    `ledger/mutations/20260929T035412-clients-python-src-slate-values-py.json`
    (`clean`, 2): a divisor and a padding width that stop following the scale
    above six and four, caught only by the new rows.
- **A test of mine that defended nothing, demonstrated rather than reasoned
  about.** The TypeScript refusal case first asserted `RangeError` by class,
  which is vacuous: `10n ** BigInt(-1)` and `BigInt(1.5)` each throw a
  `RangeError` of their own, so every case passed with the guard deleted. I
  changed it to match the message before running anything, which would have
  left the claim unmeasured — so the class-only version was put back and the
  mutation run against it:
  `ledger/mutations/20260929T035319-clients-typescript-src-value-ts.json`,
  one case, `survived-as-recorded`. The tree carries the message match.
- `ledger/mutations/20260929T035046-…` and `…035101-…` are `baseline-red` and
  are not evidence of anything: the TypeScript harness refused a
  `SLATE_SERVERD` older than `field.rs`, correctly, because the Rust mutation
  run had just restored that file and bumped its mtime. `cargo build` then
  declined to rebuild — its fingerprint is content-based and the content was
  identical — so the binary was current and the guard's mtime heuristic was a
  false positive. Touched the binary and moved on.

## What this does not do

**The Go type change is not mutation-tested, and cannot be.** `scale uint8`
refuses a negative by failing to compile, so there is no run to observe: the
mutation that would demonstrate it is `uint8` → `int`, which does not make any
test fail, it makes a caller that does not exist compile. The three run-time
refusals are tested; Rust's and Go's are asserted by the type system and by
this sentence.

**It does not make `ColumnDef.Scale` a `uint8` in the other two clients.**
Python's `ColumnDef` already refuses a negative scale at construction;
TypeScript's carries a `number`. The Go change was needed because the renderer
took the field's type; the TypeScript renderer refuses its own argument, so the
field would be a second guard for the same thing.

**Nothing refuses a scale above 18 in a renderer.** The schema does, at build
time, so no column can produce one. A caller who passes 19 to any of the four
gets a number: exact in Python and TypeScript, saturated in Rust, wrapped in Go
above 19. Making the renderers refuse would mean each holding `MAX_SCALE`,
which is a schema constant they have no other reason to know.

**The demo UI's `money` is still a fourth renderer and is not in the table.**
Deliberate — it is nine lines in a browser that is not a slate client — and it
is also the one that now differs: it has no negative-scale check and would
render scale 0 as `1250.` with an empty fractional part. Neither is reachable,
because it is called with a constant 2, and neither is tested.

**The `-1` and `-2` rows the two clients used to carry are gone rather than
converted.** Go's could not survive the type change and TypeScript's moved into
a refusal test. Nothing now asserts what a *clamping* renderer would produce,
which is correct and worth naming: if somebody reintroduces clamping, the
refusal test fails, but no test says what the clamped output would have been.
