# An array on the wire, in `slate-serverd`'s config, and in all three clients

- **Date:** 2026-09-20
- **Author:** Claude, finishing the F3 gap row
- **Touches:** `crates/slate-server/{proto,src/convert.rs,src/fingerprint.rs,tests/wire.rs}`, `crates/slate-serverd/src/{config.rs,schema.rs,value.rs,seed.rs,main.rs}`, `clients/{go,python,typescript}`, `clients/python/testserver`, `scripts/{mutate.py,test_mutate.py}`
- **Kind:** feature

## What changed

`Value.array_value` on the wire, carrying an `ArrayValue` of `Value`s.
`type = "array", element = "str"` in `slate-serverd`'s TOML, with both halves of
that pairing refused without the other. `element_type` in `--print-schema` and
in the `SchemaCheck` fingerprint — four implementations of it, one per
language. `slate.Array`, `slate.Array` and `array()` in Go, Python and
TypeScript. A `posts` table in the shared test server, and a live round trip
from each of the three clients.

Arrays are now usable from every surface this project ships. That closes F3.

## Why

The previous two commits made the kernel able to hold an array; nothing could
send one. The gap row's evidence was about `ValueType`, and the cost — as the
design note predicted — was mostly outside the kernel.

The element type is the interesting part, and it lands in the `SchemaCheck`
fingerprint for exactly the argument a decimal's scale already carries there.
It addresses no column, which is the test every other excluded property fails;
it is hashed anyway because a client that has it wrong reads the **right**
column and decodes every element as the wrong type, with the wire carrying no
element type to notice by. And it cannot change under a running client, because
it is in the kernel's layout fingerprint too, so changing one is a refused
migration rather than a silent one.

## Alternatives rejected

**Put the element type on the wire, per value.** One field, no fingerprint
change, no four-language restatement. Rejected for the reason the decimal's
scale is not on the wire: it would let a client and the catalog disagree about
what a stored value means, and the disagreement would be *per row* rather than
caught once at the door.

**Refuse nesting only in the kernel.** The kernel already refuses it, so the
server arm looks redundant. It is not: the depth of an `ArrayValue` is chosen
by whoever sends the message, so a server that recursed before refusing would
recurse to a caller-chosen depth — the denial of service the expression
converter already carries an explicit ceiling against. Refusing at depth one
means there is no depth to bound, and the refusal is a `Status` a client can
read rather than a schema error raised from somewhere it did not call.

**An array column on `docs` instead of a new `posts` table.** Adding a column
to a table three clients declare changes that table's fingerprint and every
declaration of it, so the change that is about arrays would have been mostly
about `SchemaCheck`. A new table costs one entry per client and touches nothing
else.

**One array column in the fixture instead of two.** A client with a single
array column can hard-code the element type it decodes and pass every
assertion. Two, of different element types, is the smallest fixture that
cannot.

**A JSON-shaped `array` tag in the testserver oracle carrying bare values.**
`{"array": ["a", "b"]}` would let a client that decoded `["1"]` as strings
agree with one that decoded `[1]` as integers — the confusion the tagging in
that whole function exists to stop, one level down. Each element is tagged in
turn instead, on both sides.

**Joining array element keys with a separator, in the TypeScript `valueKey`.**
This is what the first version did, with a comment asserting no key could
contain the delimiter. False: an element key is `kind:payload` and a string
payload can contain anything, so `["a|string:b"]` and `["a", "b"]` joined to
the same key. Length-prefixed instead, which is unique by construction. See
below — the mutation that reintroduces it is caught.

## Evidence

**Three clients, a real server each, arrays round-tripping with element types
intact.** Python 10 cases, Go 4, TypeScript 7 (4 live, 3 without a server). Each
suite checks the *type* of an element and not only its text, because an `i64`
that came back as a `u64` prints identically and does not compare equal to the
stored value.

**Three mutations over the TypeScript value code, all caught**, through a new
`node` dialect in `scripts/mutate.py`:

```
ok  array keys are joined by a separator instead of length-prefixed -> an array's key cannot collide with a different array
ok  arrays compare by reference instead of element-wise             -> arrays compare element-wise, not by reference
ok  an array element is not decoded, only its kind                  -> an array round-trips through the wire shape, elements and all
```

The first is the collision described above, reintroduced deliberately and
caught. The dialect skips node's per-file wrapper line — node reports
`not ok N - test/foo.test.ts` beside the real case — and
`scripts/test_mutate.py` gains three cases for it, one of which asserts an
*absence*, which needed a new `reject_text` parameter.

**A pre-existing coverage gap, found by extending the wire generator.**
`the_value_generator_reaches_every_variant` in `crates/slate-server/tests/wire.rs`
compared `any_value` against a hand-written list of nine names — and **both
were missing `decimal`**. So from the day decimals were added until now, the
wire round trip never converted one, and the guard written against exactly this
failure reported full coverage, because the list and the generator were
maintained by the same hand. The expected set is now `ValueType::ALL` plus
`"null"`, which nobody maintains.

**The decimal path was then run, and it is correct.** A null result stated
plainly: adding `Decimal` to the generator found no defect. What was missing
was the evidence, not the behaviour.

**The staleness guard fired twice**, once for the Python harness and once for
Go, refusing to test a `slate-serverd` older than `fingerprint.rs`. That is the
check added after a session spent passing tests against a stale binary, doing
its job on a change that would have been exactly that failure again.

Full suites, not one file each: `clients/python` **309 passed**;
`clients/go` `go test ./...` **ok**; `clients/typescript` `npm test` **173
pass, 0 fail**. `go vet ./...` and `gofmt`: clean.
`cargo clippy --workspace --all-targets`, `cargo fmt --all -- --check`: clean.
`scripts/check.sh`: 29 of 29 — after one red step, `python-client-ruff`, on
`RUF022`: adding `Array` to `values.py`'s `__all__` put it out of sorted order.
Caught locally by the check that exists because CI's ruff is newer than this
container's. The Go and Python protobuf stubs were regenerated
with the pinned tooling and are committed.

## What this does not do

**No SQL array literal.** `slate-serverd`'s predicate parser has no syntax for
one, so an array cannot appear in a `WHERE` written as text. It reaches
`pred.rs`'s `#[non_exhaustive]` wildcard and is refused with "this server does
not know how to write a array literal in an expression", which is honest and is
not a feature.

**No `#[derive(Record)]` support.** An array field in a derived record has no
attribute for its element type, so the macro cannot emit an `array_column`
call. Not attempted, not designed.

**No generated client row types.** The codegen reads `--print-schema`, which now
publishes `element_type`, but no generator has a case for it. A table with an
array column will generate something wrong or nothing; I did not check which,
which is itself a gap — the honest statement is that I added the field to the
output and did not follow it downstream.

**The four fingerprint implementations are held together only by the live
suites.** Rust, Go, Python and TypeScript each compute the element type's
contribution separately; nothing compares them directly. What catches a
disagreement is that every client request carries a `SchemaCheck` the server
refuses on mismatch, so the three client suites *are* the cross-language check —
but only for the tables they exercise. `the_canonical_form_is_pinned_against_an
_implementation_in_another_language` pins three values and none of them has an
array or a decimal in it.

**The testserver's JSON oracle still cannot represent a decimal.** Found while
adding the array arm: `Value::Decimal` falls to `{"unrepresentable": "decimal"}`.
Nothing puts one through that oracle today, so it has never mattered — and an
arm added there alone would *create* a disagreement, because the Python tagger
spells a `Units` as `("Units", n)` from its own type name. Closing it means
agreeing on one spelling in two places at once. Left as a comment naming the
gap rather than half-closed.

**The `posts` table is in the test server and not in
`crates/slate-server/tests/common/mod.rs`.** Those two restate the same schema
and the testserver's own comment warns that drift between them is a hazard.
This widens the difference by one table. The Rust tests need no array fixture —
`crates/slate-kernel/tests/arrays.rs` covers the kernel and
`crates/slate-server/tests/wire.rs` covers the conversion — so nothing is
untested by it, but the two files are one table further apart than they were.

**No measurement.** An `ArrayValue` costs a nested message per element on the
wire, against a `repeated float` for a vector. Nothing was timed, and no
statement here is about performance.

**The demo and the docs site show no array.** `examples/explorer` and the
workbench have no array column and no way to enter one. The feature is
reachable from three SDKs and invisible in everything built on top of them.
