# An absence two clients documented and neither demonstrated

## What changed

Go and TypeScript each gain one test: an input's own computed value, read off
the unmatched side of a left outer join, is `nil` / `undefined` rather than an
empty collection.

- `clients/go/slate/scalar_test.go` —
  `TestAnInputsComputedValueIsNilOnAnUnmatchedOuterSide`
- `clients/typescript/test/scalar.test.ts` — *"an input's own computed value is
  undefined on an unmatched outer side"*

Both doc comments now name the test that holds them up, because both said this
before anything ran it. The TypeScript fixture gains a third author with no
books, inside the test rather than in the shared `library()`, because every
author in that fixture has a book and a left join over it has nothing
unmatched — which is exactly why the case had never been exercised.

This closes the client half of
`ledger/2026-09-15-the-values-that-arrived-and-vanished.md`, and with it the
caveat `ledger/2026-09-27-an-expression-over-a-row-that-is-half-missing.md`
raised pointing at it.

## Why

`InputComputed` and `inputComputed` were written to fix a real defect — an
input-level computed value arrived on the wire and the decoder dropped it — and
the entry that added them said plainly that the unmatched case was "reasoned
about rather than demonstrated". Two weeks later a kernel-side entry hit the
same gap from the other end and recorded it again rather than closing it, which
is the point at which a caveat stops being a note and becomes a backlog item.

The property is not obvious and the two wrong answers are both plausible. A
decoder can return an empty collection ("this input computed nothing") where it
should return absence ("this input produced no row to compute over"), and every
assertion about lengths passes either way. Both clients decide it in exactly one
place — `joinedRowFromProto` in Go, `joinedRowFromWire` in TypeScript, each
extracted so the streaming and paged readers cannot disagree — so a single wrong
line is wrong in two readers at once, and that is the line these tests pin.

## Alternatives rejected

**Put the case in the shared `library()` fixture rather than in the test.** An
orphan author in the fixture would let the test read as the others do. Rejected
because four other tests in that file assert exact row counts against it, so the
change would edit five tests to demonstrate one thing, and the next reader would
have no way to tell which of them the extra author was for. The insert sits
beside the assertion that needs it.

**Assert only on the unmatched row.** Shorter, and a decoder returning nil for
*every* row would pass it — which is how a test for an absence talks itself into
passing. Both tests check the matched rows' computed values against the stored
column they were computed from, in the same loop, so "nil everywhere" fails.

**`len(...) == 0` in Go and a falsy check in TypeScript.** Both read more
naturally and both accept the wrong answer: an empty non-nil slice and `[]` are
the two mistakes this exists to catch. `== nil` and `=== undefined` are what the
doc comments promise, so they are what is asserted.

**Add a Python case too.** Python has no per-input accessor and needs none —
each input is a `Row` carrying its own `computed_values`, and an unmatched input
is `None`, which its existing outer-join tests already cover. A Python test here
would be testing `None is None`.

**A full outer join, to get unmatched rows on both sides at once.** A left join
produces the row the caveat is about with one fewer moving part. The full-outer
shape is a separate gap, already recorded by the entry this closes half of.

## Evidence

Both suites, whole, against a debug `slate-serverd` built from this tree:

- `cd clients/go && go test ./...` — `ok github.com/howlerops/slate-orm/clients/go/slate 8.282s`
- `cd clients/typescript && npm test` — `# tests 195 / # pass 195 / # fail 0`

The TypeScript test failed first, on `0 !== 1` for the unmatched count — the
fixture has no orphan author — which is the honest way to find out that a
fixture cannot exhibit the case a test is written for.

**Two mutations, one per client, both caught.** Each turns the absence into an
empty collection, which is the specific wrong answer:

| mutation | caught by |
| --- | --- |
| `perInput = append(perInput, []Value{})` for an unmatched Go input | `TestAnInputsComputedValueIsNilOnAnUnmatchedOuterSide` |
| `row ? computedFromWire(row) : []` in TypeScript | `an input's own computed value is undefined on an unmatched outer side` |

Records: `ledger/mutations/20260927T034341-clients-go-slate-client-go.json` and
`ledger/mutations/20260927T034453-clients-typescript-src-client-ts.json`.

Neither mutation is an equivalent one: `[]Value{}` is non-nil and `[]` is not
`undefined`, and the `go` dialect's cache refusal plus `-count=1` mean the Go
run judged freshly compiled code rather than replaying `ok (cached)`.

## What this does not do

**Only the streaming reader is exercised.** Both clients share the decode
between the streaming and the keyset-paged reader — that sharing is why the
comment on each says "a second copy is how the two come to disagree" — so a
mutation in the shared function is caught, but a future divergence in
`PageJoin` / `pageJoin` alone would not be. The alternative, running each
assertion twice through both readers, doubles the test for a property that is
currently a single function; it becomes worth doing the day either reader stops
calling it, which is a change a reader would have to make deliberately.

**The unmatched side is produced by a left join only.** A right outer with the
orphan book, and a full outer with unmatched rows on both sides at once, are
not run from any client. The kernel covers right and full outer joins and the
wire carries the join type unchanged, so the risk is in the client decoders,
which cannot see which join produced a missing input — they see an absent
`row` either way. That is an argument, not a demonstration.

**Nothing stops the next accessor being added without this case.** These are two
hand-written tests against two hand-written accessors. The guard shape that
would generalise — a list of every kind of value a request can produce, with a
per-client test generated from it — is what
`ledger/2026-09-15-the-values-that-arrived-and-vanished.md` proposed and is
still not built; this change does not build it, and a third client accessor
would arrive with the same gap.
