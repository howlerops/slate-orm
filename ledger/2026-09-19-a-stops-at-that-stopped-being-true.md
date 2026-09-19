# The comparison note said the clients read only the reason token, which stopped being true twice today

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `docs/orm-comparison.md`
- **Kind:** docs

## What changed

Two lines in the N4 section. Its *Stops at* — "The token. Not the rest of the
details message." — is struck through and replaced with what the clients
actually read now: the check-violation keys of `ErrorInfo.metadata`, on the
lone path and on the batched one.

## Why

Stale documentation is worse than none, because it is read as current. N4 is
marked **built** and its closing line described a boundary that two changes
today moved: the three clients decode `violations`, `check.N`, `column.N` and
`message.N`, and `BatchError` now carries the whole blob so a batched refusal
gets the same list.

Somebody reading that section to find out what a client can do with a failure
would have concluded they must parse the message. That is precisely the
contract V1 exists to avoid inventing.

## Alternatives rejected

**Delete the *Stops at* line.** Shortest, and it loses the record of where the
boundary *was*, which is the reason these sections keep their original plan
text at all. The file's convention is to strike through and say what replaced
it, which is what the entries above N4 do.

**Rewrite the whole section as though it had always included the metadata.**
Tidier to read and dishonest: N4 shipped the token and stopped, deliberately,
with a written argument about keys that vary per variant. That argument was
right and was later narrowed by V1 and V2 making one key family specified. The
sequence is the interesting part.

**Leave it, since the ledger entries record the change.** A ledger is a history
and a design note is a description; somebody looking up "what does a client get
from a failure" reads the second. That is the whole point of the standing rule.

## Evidence

`python3 site/check/docs.py` passes, which is what says the file still holds
together — it does not and cannot say the prose is true. The claim being
corrected is checkable by reading `clients/*/`: three `checkFailuresOf` /
`check_failures_of` / `violationsOf` functions, and a `details` field on
`BatchError` that all three decode. Their tests are in the two entries above.

## What this does not do

**Nothing checks a design note against the code.** This correction was found by
grepping for the words, having just changed the thing they describe. A reader
who did not already know would not have found it, and the next boundary to move
will go stale the same way. A guard is conceivable — a note's claims are prose
and a checker would need them marked — and is not worth building for a file
that is read a few times a year.

**Only N4 was checked.** The sections either side describe features this
session did not touch. Their *Stops at* lines were not re-read against the
code, so this closes one instance and not the class.
