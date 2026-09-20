# Checking the array note's one load-bearing assumption

- **Date:** 2026-09-20
- **Author:** Claude, doing what the note it follows said to do first
- **Touches:** `docs/arrays.md`
- **Kind:** docs

## What changed

`docs/arrays.md`'s decision 2 — element-wise encoding with a terminator — goes
from reasoned to verified, and the caveat that flagged it as the one
unverified thing is struck through. The design survives unchanged.

## Why

The note written minutes earlier ends with:

> **It does not check the tag space.** Decision 2 depends on a terminator byte
> below every element tag being available in `codes`… which is the one place
> this note could be wrong in a way that changes the design rather than the
> prose.

Leaving that standing would be the mistake this whole session has been about:
a claim reasoned from an adjacent case, written down, and then built on. The
check is one file and five comparisons, so there is no excuse for deferring it.

**It holds, and by a wider margin than the note assumed.** `codes` reserves
`NUL = 0x00`, and the lowest type tag is `NULL = 0x01`. So the terminator is
below every element encoding *by construction* rather than by a byte someone
has to remember to keep free, and `0x24` is the next unused tag for `ARRAY`
after `VECTOR`'s `0x23`.

The part that could have gone wrong is escaping, and it does not need any. An
element's body can contain `0x00` — a string's payload, an integer's bytes —
but the array decoder never scans for its terminator. It decodes elements one
at a time with the same reader, and each element consumes exactly its own
bytes: a string by its own terminator-and-escape, an integer by the length its
tag declares. `0x00` is therefore only ever *examined* at an element boundary,
where it cannot be anything else.

## Alternatives rejected

**Take the note's word for it and start coding.** The note explicitly says not
to. Two of its decisions are irreversible once bytes are on disk.

**Verify by writing the encoder.** The real check, and far more work than the
question needs: the question is whether the tag values permit the scheme, and
that is answerable from the constants plus five comparisons. Writing the
encoder to find out means writing an encoder that might have to be thrown away.

**Check only that a free tag exists.** Half the question, and the easy half.
The ordering property is what the design turns on, and the two cases that
distinguish a correct scheme from a plausible one — `[1, 2] < [2]`, which a
length prefix gets wrong, and `[0] < [0, 1]`, which a terminator not below
every tag gets wrong — are exactly the ones a cursory check skips.

**Assert the ordering in prose.** What decision 2 already did. Running it is
the difference between a hypothesis and a finding, which is this repository's
rule and the reason the previous version of that section was labelled as the
weak one.

## Evidence

Read from `crates/slate-tuple/src/codec.rs`: `NUL = 0x00`, `ESCAPE = 0xFF`,
tags `NULL = 0x01` … `VECTOR = 0x23`, with `0x24` free.

The proposed encoding built by hand against those values and compared
bytewise against the list ordering it must reproduce:

```
lhs          rhs          bytes   lists   agree
[]           [1]            <       <      OK
[1]          [1, 2]         <       <      OK
[1]          [2]            <       <      OK
[1, 2]       [2]            <       <      OK
[0]          [0, 1]         <       <      OK
```

`python3 site/check/docs.py`: the docs site holds together.

## What this does not do

**It is five cases, not a property test.** The real check is an oracle —
generate arrays, encode them, sort the encodings, sort the lists, assert the
permutations match — and that belongs beside the encoder when the encoder
exists. Five hand-picked cases test what somebody thought of, which is exactly
the distinction `CLAUDE.md` draws; what makes them worth running now is that
two of them are the cases that discriminate between the candidate designs.

**It does not check heterogeneous arrays.** Every element above is an `I64`.
Cross-type ordering is a property the codec already has — the tags are
assigned in type order deliberately — so element-wise comparison should inherit
it, but "should" is doing work in that sentence. Decision 1 makes an array
homogeneous through the schema, so the case can only arise from a hostile
encoding, which is the decoder's problem and belongs in `untrusted.rs`.

**It does not check the integer encoding I used.** My stand-in wrote a tiny
integer as its tag alone, which is what the codec does for zero; the real
encoding varies the tag with the byte count. That affects the *bytes*, not the
argument — the terminator is still below every tag and elements are still
self-delimiting — but the table above is my model of the encoding, not the
encoding.

**Still no code.** The note is now safe to build from, which was the point.
