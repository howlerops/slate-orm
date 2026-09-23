# "A surviving mutation is a missing test or redundant code" is a false dichotomy, and I spent part of today inside the third option.

- **Date:** 2026-09-23
- **Author:** Claude Code, working #296 (F6p)
- **Touches:** `scripts/mutate.py`, `scripts/test_mutate.py`, `CLAUDE.md`
- **Kind:** a message that was wrong in the direction that manufactures findings

## What changed

`mutate.py` told the author two things could cause a survivor. There are three,
and the one it left out is the one to rule out first:

```text
!!   a case: SURVIVED (1 suites ran).
     Three causes, and the third is the one to rule out first: the mutation
     may not be a change.
     This script checks that `old` occurs exactly once, never that `new`
     behaves differently —
     `&x.clone()` and `if true { x } else { x }` both survive everything and
     mean nothing.
     Otherwise it is a missing test or redundant code: write the test, or
     record why it cannot be caught with `expect_survivor`.
```

`CLAUDE.md`'s mutation-testing paragraph now says the same, since that is where
the discipline is stated.

## Why

Three of my own mutations today were equivalent code. `&input.data.clone()`
matches the same pattern `&input.data` does; `if true { x } else { x }` is `x`.
Each survived, correctly and meaninglessly.

The first one I nearly wrote up as a finding — an untested cross-tenant guard
in `#[derive(Record)]` — and stopped only because the claim seemed too
alarming to state without re-reading the file. It was wrong twice over: the
mutation was a no-op *and* the guard has had a `compile_fail` doctest since
#149.

That is the shape of the problem. A survivor is supposed to mean "write a
test", so the tool's own instruction pushes toward adding a test that defends
nothing, or toward recording a defect that does not exist. The message was
confident in exactly the wrong direction, and a confident wrong message beats
a reader's doubt more often than it should.

## Alternatives rejected

**Detect equivalent mutations.** This is the fix that would actually help and
it is not available: deciding whether two programs behave identically is
undecidable, and the cheap approximations are worse than nothing here.
Comparing compiled output would call every `#[inline]` difference a change and
miss every no-op the optimiser folds. What the tool *can* do honestly is say it
does not check, which is what it now does.

**Refuse a replacement that is textually similar to the original.** Tempting
and wrong in both directions: `!=` for `==` is one character and always a real
mutation, while a fifty-character equivalent expression is not. Similarity is
not the property.

**Leave it and rely on the author.** The author here had read the script's
docstring, written two entries about its failure modes the same day, and still
walked into it three times. A trap that catches the person who just finished
documenting the tool is not one to leave to attention.

**Say it only in `CLAUDE.md`.** The message is read at the moment of the
mistake; the document is read before starting. Both, and the message first.

## Evidence

Two mutations, **all caught**, in
[`ledger/mutations/20260923T140426-scripts-mutate-py.json`](mutations/20260923T140426-scripts-mutate-py.json):
dropping the equivalent-mutation cause from the message, and silencing the
survivor line entirely. The existing case that asserted the old wording now
asserts all three causes, so the sentence cannot be quietly shortened back.

The citation above was wrong when first written — I typed a timestamp the run
had not produced, exactly as in #292 — and `check_mutation_claims.py`, built
this morning for this, refused the entry. Second time that guard has caught its
own author in a day.

`sh scripts/check.sh`: **48 passed, all of them.**

## What this does not do

**It does not stop an equivalent mutation being written**, only reframes what
its survival means. The next person still has to look at their own `new` string
and ask whether it is different, and nothing checks that they did.

**It is a message, not a mechanism**, which is the weakest kind of guard this
repository has — it works only on a reader who reads it. The guards added this
week that *refuse* a bad state are strictly better, and no such guard is
available here for the reason in Alternatives.

**The three equivalent mutations are mine and recent.** Whether earlier
sessions wrote any is unknown: the records begin on 2026-09-22 and hold the
`old`/`new` text, so from here forward the question is at least answerable by
reading them, which it was not before.
