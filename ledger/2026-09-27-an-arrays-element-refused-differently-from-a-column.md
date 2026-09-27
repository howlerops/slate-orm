# The same unknown type name was a readable refusal for a column and a bare KeyError for an array of it

- **Date:** 2026-09-27
- **Author:** Claude Code, closing an open caveat from
  `ledger/2026-09-21-generate-an-array-column-in-three-languages.md`
- **Touches:** `scripts/codegen.py`, `scripts/test_codegen.py`
- **Kind:** fix

## What changed

`element_spelling(table, column, what)` replaces the five bare
`table[element_of(column)]` subscripts in `scripts/codegen.py`. An element type
no spelling table knows now raises the same `Unknown` a scalar column's unknown
type already raised, naming the column, the element type, and which table is
missing it.

Two tests. `test_an_element_type_no_table_spells_is_refused_by_every_emitter`
drives an `array<u128>` through the Python, TypeScript and Go emitters.
`test_every_element_lookup_refuses_readably_on_its_own` calls the four helpers
directly, because the emitters short-circuit — `declared` refuses before
`field_of` is ever called.

## Why

A column's own type goes through `types.get(...)`, and a missing spelling there
produces "no spelling for `u128`". The element type is a *second field* on the
column — deliberately, because `ValueType` stays fieldless so it can stay
`Copy`, `const` and finitely enumerable — so it is resolved through
`element_of` and then looked up separately. Those lookups were subscripts.

So one catalog, from one server that spells a type these tables do not know,
produced two different failures depending on whether the column was a `u128` or
an `array<u128>`: a sentence naming the column, or `KeyError: 'u128'` from a
line number. The caveat said the spelling "is assumed and is not checked
anywhere"; this is the check, and it is at the only place the assumption is
made rather than at each of the five places it is used.

## Alternatives rejected

**Validate the element name once, inside `element_of`.** The obvious shape, and
it would have been one edit instead of five. Rejected because `element_of` does
not know which language is being generated, so it would have to be handed every
table or hold a canonical list of its own. The first makes the array-of-arrays
refusal depend on its caller; the second is a sixth list that has to agree with
the five — and the comment above `PYTHON_TYPES` argues at length that the tables
are deliberately allowed to diverge, precisely so a type can be added to one
client without inventing spellings for the other two. A canonical list would
have quietly undone that.

**Let the `KeyError` stand and document it.** It does say which name is missing.
It does not say which column, which table, or that the name came from the
server's `ValueType::name()` rather than from anything in this repository — and
the person reading it is looking at generated-code output, not at `codegen.py`.

**Test only through the emitters.** This is what the first draft did, and
`scripts/mutate.py` caught it: reverting `field_of` and the Go encoder's lookups
to bare subscripts *survived*, because `declared` and `go_element` refuse first
and the later lookups are never reached with a bad element. One test through the
emitters plus one per lookup is what it takes for a mutation at each site to
fail something.

## Evidence

- `python3 -m pytest scripts/test_codegen.py -q` — 39 passed (37 before).
- Six mutations via `scripts/mutate.py`, recorded as
  `ledger/mutations/20260927T021612-scripts-codegen-py.json`: five caught,
  each by a named test. The sixth — the `GO_ENCODE` lookup inline in `go_rows`
  — is recorded as an expected survivor with its reason: `go_element` on the
  line above looks the same name up in `GO_FIELDS` and refuses first, so that
  lookup is defensive against the two Go tables disagreeing, which no test can
  reach without making them disagree.
- Before the fix, hand-run against `{"name": "tags", "type": "array",
  "element_type": "u128"}`: `declared`, `field_of`, `python_encode` and
  `go_element` all raised `KeyError: 'u128'`. After: four `Unknown`s, each
  naming `tags` and `u128`.
- `sh scripts/check.sh` — 71 of 71. `codegen.py --check` is one of them, and
  the generated files are byte-identical: this changes how an unspellable
  element fails, not what a spellable one emits.

## What this does not do

**`web_module` was in the emitter list on the first draft and passed an
`array<u128>` through without a word.** That is correct — it emits column
*names* and looks no type up at all — but it took a run to establish, and an
emitter silently accepting an unspellable element is indistinguishable at the
call site from the defect this closes. It is now named in the test's docstring
as deliberately absent.

**Nothing checks that the five tables agree with `ValueType::name()`.** The
refusal fires when a catalog arrives carrying a name the tables lack, which is
one edit and one server release after the divergence. A check comparing the
tables against the server's own list would catch it at the commit that caused
it, and would need `--print-schema` or a parse of `value.rs` — the second
schema this repository has rejected before. The refusal is the cheap half.

**`wants()` still ignores an element type it does not recognise**, silently, the
same way it ignores an unrecognised scalar. That is consistent and it is not
this caveat, but it is the one remaining place where an unknown name produces no
sound: it decides which `slate.values` names to import, so the failure would be
a `NameError` at import of the generated module rather than a wrong decode.
