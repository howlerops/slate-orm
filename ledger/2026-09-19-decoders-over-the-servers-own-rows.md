# The generated decoders run over rows a real server sent, and something calls them

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** the three demo adapters, `examples/explorer/conformance/conformance.py`
- **Kind:** fix

## What changed

`/api/typed` in all three demo adapters: read `books` 10 and `shipments` 600,
decode each with the *generated* row type, and render named fields off the
decoded value. Then a conformance case comparing the three.

## Why

Two things, and the second is the one I did not expect to find.

The recorded gap, from the entry that first executed the decoders: "Nothing
tests the decoders against rows that came from the *server*. These build
`[]slate.Value` by hand, so a disagreement between what the server sends and
what the decoder expects — a value arriving as `Int` where the schema says
`Uint`, say — would not be caught here." Every suite agreed with its own idea
of the wire.

The second: **nothing called a generated decoder outside a test.** Grepping the
three adapters for `ScanBooks`, `decodeBooks` and `Books.from_row` finds
nothing but the generated files and their own tests. They were generated,
compiled, vetted, typechecked, and — since the previous entry — executed
against fixtures, and no code path used one. That is the same shape as the
decoders shipping unrun in the first place, one level out: a decoder whose
contract with the server is a hypothesis, tested against a transcription of
that hypothesis.

## Alternatives rejected

**A Go/TypeScript test that starts a head node itself.** The clients' own
suites already do this, so the machinery exists. It would put the demo's
generated files under test from the client repositories, which is backwards —
they are generated from the *demo's* catalog — and it would test each language
alone. The conformance runner compares the three, which is the stronger claim
and the reason the corpus exists.

**Decode every seeded row of every table.** More coverage, and the answer
becomes a page of JSON whose diff is unreadable when it fails. Two rows chosen
to cover the interesting shapes — u64, string, i64, decimal, vector, a nullable
column and an enumerated one — is what a failure can be read from.

**Include `rating`.** It is the one column left out, and deliberately: a
double's decimal spelling is the thing three languages will not agree on
without a shared formatter, the corpus pins float rendering elsewhere, and a
disagreement here would be about `%f` rather than about decoding. Saying so in
the handler rather than leaving a reader to notice the omission.

**Make an existing handler use the decoders instead of adding a route.** The
tempting version, because then the decoders would be on a path the whole corpus
exercises. It also rewrites handlers whose answers are pinned by cases that
have nothing to do with this, and a rendering difference would then be
attributed to the wrong change. A route of its own is one case and one thing to
read.

**Integers as JSON numbers.** TypeScript decodes them as `bigint`, so they
would have to be narrowed to `Number` and the three would agree right up to
2^53. Strings, like the demo's other handlers.

## Evidence

98 conformance cases, the three SDKs agreeing on all of them, with the new case
among them. Then the probe that says the case is load bearing — ordinal 3
changed to 5 in the TypeScript generated decoder's `year`, which is exactly the
mistake the whole generator exists to prevent and which typechecks perfectly:

    two rows through the generated decoders (app): the adapters disagree
        go      {"book": {… "year": "1968" …}, …}
        node    {"book": {… "year": "-36754200" …}, …}
        python  {"book": {… "year": "1968" …}, …}

One case, naming the field and printing the neighbour's value. Restored, and
the run is back to 98 agreeing. The value it read is `published`, an `i64` at
ordinal 5 — a column of the *same kind*, so the decoder's own type assertion
could not see it and nothing but a comparison against a second implementation
could.

`sh scripts/check.sh` is 19 passed, which includes `tsc --noEmit` over the node
adapter and `go vet` over the Go one.

## What this does not do

**Nothing yet decodes a row with a value of the wrong *kind*.** The case proves
the decoders agree with the server on the rows the server actually sends. The
failure it is built to catch — an `Int` where the schema says `Uint` — is one
the server does not currently produce, so this is a *regression* guard rather
than a demonstration that the guard fires. Making the server send the wrong
kind is not reachable through any of its surfaces, which is the good news and
also why this evidence is weaker than a caught mutation.

**Three of five decoders per language are still only run against fixtures.**
`authors`, `sales` and `editions` have unit cases from the previous entry and
no live row. The two chosen cover every value shape the demo has between them,
so the three that are left add a column list rather than a kind — a hypothesis,
labelled as one.

**The Python client has no `ForeignKey` type and its checks are plain dicts**,
so `Books.from_row` is the only generated Python artefact this exercises. Its
generated declaration is covered by the fingerprint check, as before.

**The demo UI still does not use the decoders.** It reads tagged values off the
adapters' JSON, which is the adapters' shape and not a client's. Making the
browser app use the TypeScript row types would be a fourth consumer and a
larger change than this.
