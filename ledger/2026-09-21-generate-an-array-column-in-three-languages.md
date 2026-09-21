# The generator emits an array column, and the type tables did not have to change

- **Date:** 2026-09-21
- **Author:** Claude Code (agent session)
- **Touches:** `scripts/codegen.py`, `scripts/test_codegen.py`, `docs/arrays.md`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`scripts/codegen.py` generates an array column for Python, Go and TypeScript
instead of refusing one. The declaration carries the element type — Python's
`element=`, Go's `Element:`, TypeScript's `element:` — because the `SchemaCheck`
fingerprint hashes it and a declaration without it is refused by the server on
the first request. The decoded field is element-typed (`Sequence[str]`,
`[]string`, `string[]`), every element is checked on the way in and wrapped on
the way out, and the error names the position. `UNSUPPORTED` is now empty.

## Why

This was the last thing `docs/arrays.md` recorded as left after the kernel, the
wire, the three clients and the predicate parser had an array. Its own note
said why it was hard: the type tables are keyed by the column's type and an
array's element type is not in `type` — it is a second field on the column, the
way a decimal's scale is, for the reason that note gives one level down
(`ValueType` stays fieldless so it stays `Copy`, `const` and finitely
enumerable).

The resolution is that the tables did not have to express it. One more level of
indirection — `element_of`, `declared`, `field_of`, applied per column rather
than per type — is enough, and the tables are untouched. The refusal was
correct about the obstacle and wrong about its size.

## Alternatives rejected

**A fourth table per language, keyed by `(type, element)`.** Nine element types
times three languages times three tables is eighty-one entries to describe
something with one rule. Rejected on that alone; the per-column function is
three small functions total.

**Making the generated field the raw list of wire values** — `Sequence[PyValue]`,
`slate.Array`, `Value[]` — and leaving the caller to unwrap. Cheap, and it buys
nothing: unwrapping by hand at every call site is exactly what the generator
exists to remove, and the element type would then appear nowhere in the
generated code, so a wrong one could not be caught at all.

**Leaving Python's decoder to trust the list.** A Python array decodes to
*native* elements already — an `Array` of `str`, not of tagged values — so
`Sequence[str]` is honest with no unwrapping, and the element check looks like
work for nothing. It is not: the element type is not on the wire, so a
declaration naming the wrong one produces a list that looks right at the call
site and is refused by the server, a long way from the mistake. Go and
TypeScript must unwrap anyway; Python checking too is what keeps the three
saying the same thing.

**A generated Go helper per element type** rather than an inline loop in the
encoder. One helper would be shared by two columns of different element types
and would need a type parameter for no gain, and `gofmt` is equally happy with
the loop.

**Deleting `refuse_unsupported` now that `UNSUPPORTED` is empty.** Rejected,
but an empty roster means the code that reads it never runs, which is the
"a check that never fires is a check nobody has debugged" shape. So the
mechanism stays and its test puts an entry in, checks the refusal names the
table, the column and the reason, and takes it out again.

## Evidence

`scripts/test_codegen.py` is 28 tests passing, four of them new. The Python
case is **executed**: it compiles the generated module, decodes an
`array<string>` and an `array<i64>`, asserts an empty array stays a value
rather than a null, asserts the encoder's elements keep their tags, and
asserts a wrongly typed element is refused naming `posts.tags[0]`.

The Go and TypeScript output was compiled rather than only matched. A scratch
module against the real client: `go build` clean, `gofmt -l` empty, `go vet`
clean. The generated TypeScript was dropped into the demo's Node adapter and
`tsc --noEmit` accepted it against `@slate-orm/client`.

Thirteen mutations through `scripts/mutate.py`, **all thirteen caught**:

| mutation | caught by |
| --- | --- |
| a declaration drops the element type | the declaration test, and the executed one |
| the decoded field takes the column's type | both array tests |
| Python reads an array with `_field` | the executed test |
| Python encodes an array unwrapped | the executed test |
| the generated module does not import `Array` | the executed test, as a `NameError` |
| an array with no element type is generated | the refusal test |
| an array of arrays is generated | the refusal test |
| Go asserts elements to `slate.Value` | the Go/TS source test |
| Go encodes elements untagged | the same |
| TypeScript reads an array with `field` | the same |
| TypeScript encodes elements untagged | the same |

**Two of those started as survivors, and that is the finding.** The Go and
TypeScript element checks could be removed with nothing failing, because the
tests I had written asserted only the *declaration* line in those two
languages. The Python case was executed and caught everything; the other two
were checked for the one thing they had in common with it. Adding assertions on
the decoder and encoder bodies caught all four.

**A third defect was found by running the generated Python**: the module
referred to `Array` without importing it, so it was a `NameError` at import.
Neither `ruff` nor `ty` sees that — neither is given a file that does not exist
yet — and the pattern-matching tests would not have either. This is the second
time in this file's history that executing the output caught something reading
it could not; the earlier one was a `SyntaxError` from a quoting bug.

## What this does not do

**Nothing executes the generated Go or TypeScript array code.** The suites that
run generated code are the demo's own, and the demo's schema has no array
column, so the two are compiled and read but never called. That is the same
shape as the gap this repository closed as CG1 and G5, and closing it means
putting an array column in `examples/explorer/head.toml`, regenerating the
three committed files and extending the three decoder tests. It is written down
here and in `docs/orm-comparison.md` rather than left implied.

**`UNSUPPORTED` is empty**, so the only thing the generator now refuses is an
array with no element type and an array of arrays — neither of which a catalog
`slate-serverd` printed can contain, since the schema layer refuses both at
build time. Both are kept for the reason the other unreachable refusals here
are kept, and both are tested.

**An element is never nullable**, matching `Row::validate`, which refuses a
null element on every write. Nothing in the generator says so, because nothing
in the catalog can ask for it; if the kernel ever relaxes that, the generated
field type will be wrong and no test here will notice.

**The `element_type` key is read from `--print-schema` and assumed spelled the
way `ValueType::name()` spells it.** That is true and is not checked anywhere:
`declared` would raise `KeyError` on an unexpected spelling rather than the
readable `Unknown` the rest of this file raises.
