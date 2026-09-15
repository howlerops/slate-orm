# Timestamps, enums and JSON, none of which is a new value type

- **Date:** 2026-09-15
- **Author:** Claude Code
- **Touches:** `slate-orm` (`field.rs`, new `json.rs`, `lib.rs`, `Cargo.toml`),
  `slate-derive` (`#[derive(Enum)]`), `.github/workflows/ci.yml`, `README.md`,
  `site/docs.html`
- **Kind:** feature

## What changed

Three Rust types now map onto the record layer without widening its value
model, plus the re-exports that make one of them useful:

- **`Timestamp`** — an `i64` of seconds since the epoch. Nothing on disk
  changes; the column is `ValueType::I64`, the same bytes an `i64` field writes.
- **`#[derive(Enum)]`** — a fieldless enum stored as its variant name in a
  `Str` column, with `#[record(rename = "...")]` per variant.
- **`Json<T>`** — a serialized document in a `Str`, behind a non-default `json`
  feature, serialized at construction rather than at write time.
- **`Scalar`, `CalendarPart`, `CalendarUnit`, `TimeUnit`, `Metric`** are now
  re-exported from `slate-orm`. They were not, which meant a caller of the ORM
  could store a timestamp and had to reach past the crate into `slate-kernel`
  to ask a calendar question about it — against `slate-orm`'s own stated
  purpose, which its module docs spell out as "re-export what a caller needs so
  they are not forced to depend on four crates".

CI gained two lines: `cargo clippy -p slate-orm --features json --all-targets`
and `cargo test -p slate-orm --features json`.

## Why

The request was "type mapping: timestamps, enum, JSON", and the interesting
part is that none of the three wants a tenth value type.

The value model is closed because each of its nine types is one the tuple codec
can order, and the ordering is what makes an index an index. A date type would
need an ordering, and it would be the integer ordering of its seconds — so it
would be a second, unproven copy of one this repository has already fuzzed. The
same argument settles JSON in the other direction: a document has *no* useful
total order, so it cannot be a key, cannot be indexed, and cannot answer a range
predicate. A `Json` value type would exist to be excluded from every path that
makes the layer worth using.

`Units` is the precedent and the contrast. A decimal *did* get a value type,
because its ordering and its arithmetic genuinely differ from an integer's — a
decimal column sums exactly and a float column does not. An instant's do not.
The rule that falls out: a new value type is for a new *ordering*, and
everything else is a `Field` impl.

## Alternatives rejected

**Storing an enum as an ordinal.** Compact, and it sorts by declaration order,
which is what a severity column wants. Rejected on the failure modes rather than
the sizes: with an ordinal, *reordering* the variants silently reinterprets
every stored row, and reordering is the thing people do by accident —
alphabetising, inserting a variant in the middle, nothing in the diff to suggest
it matters. With a name, *renaming* does the same damage, but a rename is a
deliberate act and the compiler drags you through every use site while you do
it. The name fails on the rarer, louder action. Both costs are written into the
derive's own docs, including the one that bites: an index over the column sorts
`"critical" < "info" < "warning"`, so an ordered enum should be an integer with
a hand-written `Field` and this derive is the wrong tool.

**Milliseconds, or a scale on the timestamp column the way a decimal has one.**
Every calendar function in the kernel reads seconds, so a millisecond column
would be answered with a year around 55000 and no error. A scale would fix that
and would mean threading it through `CalendarPart`, `DateTrunc`, the zone
lookup and the three clients. Not built; the README says so, and `Timestamp`
names its unit in the type so the trap is at least visible.

**`chrono` or `time` interop.** A dependency in the record layer for two `i64`
conversions a caller can write. `from_unix_seconds` and `.seconds()` are the
whole surface.

**A lazily-serializing `Json<T>`.** The obvious shape, and unimplementable
honestly: `Field::to_value` returns a `Value`, not a `Result`, and the workspace
forbids `panic`/`unwrap`/`expect` — so a lazy version would have to write a
wrong value when serialization failed. Serializing in `Json::new` moves the
failure to where a caller can still do something about it and makes the write
infallible, at the cost of keeping the string beside the document.

**Sorting map keys inside `Json` to make every encoding stable.** It would fix
`HashMap` and silently change the encoding of every other type, buying
consistency for one container at the cost of surprise everywhere else.
`BTreeMap` already has the property; the test asserts that, so the
recommendation is demonstrated rather than asserted.

**Making `json` a default feature.** It would be covered by
`cargo test --workspace`, which is the real reason to want it. Rejected because
every consumer of the ORM would then pay for `serde_json` to get a convenience
over `String`; the two explicit CI lines cost less. Named explicitly rather than
`--all-features`, which would also switch on `slate-slatedb`'s storage features
and change what the other jobs test.

## Evidence

`crates/slate-orm/tests/types.rs` (8) and `tests/json.rs` (8), plus five doc
tests and five `compile_fail` doctests for the derive's refusals.

**Mutation testing**, 7 mutations:

| mutation | outcome |
|---|---|
| `Timestamp::VALUE_TYPE` I64 → U64 | caught, 3 tests |
| the derive ignores `#[record(rename)]` | caught, 2 tests |
| an unknown variant name reads as the first variant | caught |
| the duplicate-stored-name check deleted | caught, by the `compile_fail` doctest written for it |
| a parse failure reported as the wrong `FieldError` | caught |
| **`Json::from_value` drops the encoded text** | **SURVIVED** |
| `Json` accepts text that is not the document | (first attempt was a no-op; redone above) |

The survivor was a real defect and the entry worth keeping. Blanking `encoded`
in `from_value` left every test green, because every one of them built with
`Json::new` and none wrote back what it had read — so a `Json` column would have
stored an **empty string** on the second write of any row, a total, silent data
loss on exactly the read-modify-write path an ORM exists for.
`a_row_read_and_written_back_stores_the_same_text` covers it and the mutation is
caught now.

One test was wrong before it was right, and the comment records it.
`json_hash_maps_do_not_have_a_stable_encoding` first asserted that sixteen
`HashMap`s built in one function all encode the *same* way, on the belief that
`RandomState`'s seed is per process. It is per map — each takes a fresh seed
from a thread-local counter — so the sixteen produced several different strings
and the test failed. The claim in the docs is stronger than the one first
written, and it is stronger because the hazard was run rather than reasoned
about.

`Option<Timestamp>` is covered too, with `None` and a *negative* instant in the
same table: a mapping that folded a null and "one second before the epoch" into
each other would pass a test that used only positive instants.

Checks: `cargo test -p slate-orm --features json` (all binaries), the doc tests
above, `cargo clippy -p slate-orm --features json --all-targets` clean, and
`cargo clippy --workspace --all-targets` clean.

## What this does not do

**Nothing crosses the wire.** All three are `Field` impls, which live in the
Rust ORM: a `Timestamp` reaches a client as the integer it is, an enum as its
string, and a `Json<T>` as its text — which is arguably fine for all three and
is not the same as the clients having the types. No client change was made.

**Nothing inside a `Json<T>`** — no path expression, no index on a field, no
partial update — and that is the design rather than a gap.

**No measurement.** An enum name is longer than an ordinal in every row and
index entry and nothing here says by how much on a real table; the claim is
about the mechanism, and `docs/performance.md` already has the per-row footprint
work that would be the place to put a number if one is ever taken.
