# The review's own scope sentence is why it missed findings 9 and 10

- **Date:** 2026-09-20
- **Author:** Claude, reading the review's header after editing its body all day
- **Touches:** `docs/security-review.md`
- **Kind:** docs

## What changed

Three corrections to the review's front matter: a duplicated row in the
demonstrations table, two missing rows, and a scope sentence that findings 9
and 10 falsify.

## Why

The document has been edited a dozen times today and the parts nobody re-reads
had drifted. `crates/slate-server/tests/security_probe.rs` appeared twice — as
"finding 2 over gRPC, finding 6" and again as "findings 8 and 9" — because each
addition appended a row rather than amending one. Finding 10's demonstration
was missing, and so was the lease decoder's.

The third is the one worth an entry. The review opens with:

> Everything below is reachable by an **ordinary authenticated caller** — a
> principal with a tenant and an `app`-shaped role — unless it says otherwise.

That was a true and useful framing for findings 1 to 8. It is false for both
findings found since, and **the way it is false explains why they were
missed**:

- **Finding 9 needs no authentication at all.** It is the one RPC of nineteen
  that never looked at the caller. A reviewer holding "what can an
  authenticated caller reach" in mind reads the handler list for what they do
  with the context, and the one that takes `_request` does not appear in that
  reading at all.
- **Finding 10 is not reachable over the wire in either direction.** It is a
  client library printing its own user's credential into a log. There is no
  caller, no request and no server; the whole vocabulary of the review does not
  reach it.

So the sentence now says which findings it covers, and says plainly that a
review scoped that way would not have found the other two — and did not.

## Alternatives rejected

**Widen the sentence to cover everything.** "Everything below is reachable by
somebody" says nothing. The original framing is precise and load-bearing for
the eight findings it describes: it tells a reader that no finding needs a
superuser, which is the first question anybody asks. Keeping it and naming its
limit is worth more than replacing it with something vaguer.

**Renumber 9 and 10, or move them into a separate document**, since they are a
different shape from 1–8. Rejected because the numbering is cited from commit
messages, ledger entries and test comments written today, and because the
sharpest thing about them is precisely that they sit in the same list as the
findings a narrower frame did catch.

**Say nothing about why they were missed.** The review's own best paragraph is
the one listing what it did not examine, on exactly this principle: what a
review did not look at is more useful than another account of what it did. A
frame that excluded two real findings is the same kind of information.

## Evidence

The duplicated row is visible in the table's own diff; there are now seven
rows, one per demonstration file, with no file listed twice.

`python3 site/check/docs.py`: every relative link resolves.
`python3 scripts/check_cited_tests.py`: 9 documents, every cited test name
resolves — which matters here because the table now cites two more files.

No code changed, so there is nothing to mutation-test. What stands behind the
new rows is the tests they name, each of which was mutation-tested in the
commit that added it.

## What this does not do

**It does not re-run the review under a wider frame.** Naming a blind spot is
not searching it. Two classes are now visibly outside what this document
covered: things reachable *without* authentication, and things that are not
reachable over the wire at all — client libraries, CLI tools, the seed command,
the migration runner. Finding 9 was the first of the former and finding 10 the
first of the latter; neither class has been swept.

**The "probed and clean" section was not re-read against the same question.**
Its claims were made under the old frame too, so an unauthenticated path
through any of them would have been out of scope there as well. `rls_probe.rs`
and the wire-ordinal work are about what an authenticated caller can do, and
rule 5 of `check_handlers.py` now covers the "no authentication at all" case
for RPCs — but nothing has re-examined the clean list itself.

**Three of today's edits to this file were corrections of earlier edits to this
file.** The review has been rewritten enough times that its coherence is now
maintained by two link checkers and a reader, and the reader is the part that
failed here.
