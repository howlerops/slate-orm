# A caveat withdrawn: the previous spelling was accepted all along

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `clients/go/slate/schema_test.go`,
  `clients/typescript/test/schema.test.ts`, and three ledger entries
- **Kind:** repair

## What changed

`2026-09-14-schema-checks-in-go-and-typescript.md` recorded, under "What this
does not do":

> Neither client accepts a renamed column's previous spelling, which the server
> does accept. A client declaring the old name is refused where the Python
> client would be served.

Both halves are wrong. Struck through, with the correction beside it, and two
tests that demonstrate the truth.

Three other caveats had gone stale and now say so: the demo backends nothing
built (`./run.sh --conformance` builds all three), the frontend's hard-coded
ports (it reads them from the environment), and `scripts/build_testserver.sh`
left in place (it is gone).

## Why

Stale documentation is worse than none because it is read as current, and a
recorded gap is read as a to-do list. Somebody would have set out to add
previous-name support to two clients that need none.

## What was actually true

A client declares the spelling *it* uses and hashes that. The server does not
compare against one fingerprint: `fingerprint::accepted` enumerates every
spelling the catalog would accept — a product over each column's renames,
truncated past `MAX_SPELLINGS` — and checks membership. So the previous name
passes, the current name passes, and a name the table never had is refused.

`TableDef` having nowhere to record a rename is therefore not a gap; it is why
there is nothing to record. And the Python client models renames no more than
the other two, so the claim that it "would be served" where they are not was
comparing two identical things and finding one better.

## How the mistake happened, which is the part worth keeping

The reasoning ran entirely on the client's side: *the client's type has no
field for a previous name, therefore the client cannot express one, therefore
it is refused.* Each step follows. The conclusion is false because the premise
of the last step — that expressing it is the client's job — was never checked
against the ten lines of server code that decide.

It is the same shape as the "grouped join is not costed as grouped" hypothesis
withdrawn two days ago: a plausible chain about one side of an interface,
written up as a gap without reading the other side. Both were caught by writing
the test that the claim implies should fail.

## Alternatives rejected

**Deleting the caveat.** It would leave no record that the reasoning was made
and why it was wrong, which is the only durable value here — the fact itself is
two lines of server code anyone can read. Struck through and answered instead.

**Correcting it in prose only.** "A finding must be demonstrated, not argued",
and the claim being withdrawn was itself an argument with no test. Replacing one
argument with the opposite argument would repeat the mistake in the other
direction.

**One test, in one client.** The caveat named both, and the two clients compute
their fingerprints independently — Go in `schema.go`, TypeScript in
`schema.ts`, each with its own FNV port. A single test would leave the other
client's behaviour still asserted rather than shown.

## Evidence

`TestARenamedColumnIsAcceptedUnderItsPreviousName` (Go) and `a renamed column is
accepted under its previous name` (TypeScript). Each declares a table whose
`kind` column carries `previous_names = ["category"]`, inserts and deletes
under *both* spellings, and then — the control that makes it a test rather than
a demonstration — declares `genre`, a name the table never had, and requires
`invalid-request`.

Without that control the test would pass against a server that had stopped
checking fingerprints at all, which is the failure mode a "this is accepted"
test is most prone to.

Go: full suite green. TypeScript: 52 pass, up from 51.

## What this does not do

Neither test covers a column with *several* previous names, or the
`MAX_SPELLINGS` truncation that stops the accepted set exploding on a table with
many renames. Both are exercised in `crates/slate-server/src/fingerprint.rs`,
where the enumeration lives; a client cannot tell them apart from here, since it
sends one hash either way.

The Python client has no equivalent test. Its fingerprint is the reference the
other two were ported from and is pinned against them, so the property follows —
but "follows" is not "shown", and this entry has just spent five paragraphs on
what that distinction costs.
