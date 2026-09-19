# The arm the refusal never reaches

## What changed

A comment block in `crates/slate-server/tests/transaction_counts.rs` naming the
one `Tally::applied` case with no test — the plain delete arm's "failed having
applied nothing" — and the two routes that do not reach it. The open-hole
paragraph in `ledger/2026-09-19-the-count-a-refusal-did-not-write.md`, written
one commit earlier, is corrected to match: it guessed at *why* the state was
unreachable, and one of the two reasons turned out to be a different thing than
I said.

No behaviour changes.

## Why

The previous entry said I "could not reach that state over the wire without a
policy or fencing setup this file has no harness for". That was a guess dressed
as a finding — I had not tried a policy. When I did, the result was more
interesting than the guess: a principal with no grant on `docs` *is* refused,
but the refusal never reaches the session task, so a test written against it
would pass under a deliberately broken tally. It would have looked like coverage
of the one arm that has none.

That is the failure shape this repository keeps producing — a check that never
fires, a decoder that compiles and never runs, a workflow that triggered on
nothing — and the version of it that survives review is a *test* that runs and
pins nothing. A hole named in prose is honest. A hole covered by a test that
cannot fail is worse than the hole, because the next reader stops looking.

Putting it beside the tests rather than only in a ledger is the point: the
reader who asks "why is there no plain-delete case here" is reading that file,
not this one.

## Alternatives rejected

**Add the unauthorized-delete test anyway.** It passes, it reads as coverage of
the gap, and mutating the arm back to the unconditional `add` leaves it green.
Cost: exactly the false assurance above, for thirty lines.

**Add a foreign key to the shared `docs`/`books` fixture** so a restrict
violation gives the plain delete a real mid-loop failure. This would genuinely
close the hole. Rejected on blast radius: `common::docs()` is the `TableDef`
that most of `slate-server`'s twenty-four test binaries build rows against, and
a new constraint on it changes what every one of them is allowed to write. The
value is one branch of one arm, whose sibling branch two lines away is tested.
If a future change needs an FK in that fixture for its own reasons, this becomes
free and should be taken then.

**Leave the previous entry as it was.** It is one commit old and nobody has read
it. But it states a reason that is not the reason, and "stale documentation is
worse than none, because it is read as current" does not have a grace period.

**Say nothing in the test file and only fix the ledger.** Cheaper, and it leaves
the absence looking like an oversight to anyone reading the tests — which is how
absences get filled with the bad test above.

## Evidence

**The unauthorized delete is refused before the session sees it.** A principal
`u64:1` holding role `nobody` (no grant on `docs`; only `app` has one) sending a
`Delete` inside an open transaction gets `PermissionDenied`, and the transaction
still commits. Observer contents before and after the attempt are identical:
`[("insert", "docs", 1)]` from the setup insert, both times.

**And that is not the tally being right.** Mutating all three `delete` call
sites back to the unconditional `add` — the spelling that records a zero on a
refusal — left the case green. An `eprintln!` at the top of the plain delete arm
printed nothing during the run, confirming the arm is not entered rather than
entered-and-counted-correctly.

**The absent-key route is closed by design, not by accident.** `transaction
.delete` answers `Ok(false)` for a key that is not there, with a comment saying
why: a caller must not be able to tell an absent row from one the policy hides.
The loop does not break, so `outcome` stays `Ok`.

**`crates/slate-server/tests/common/mod.rs` declares no foreign keys at all** —
`grep -n "foreign_key\|references\|OnDelete\|Restrict"` returns nothing — which
is what rules out the third route without a fixture change.

**Suites:** `cargo test -p slate-server --test transaction_counts` — 13 ok.
`sh scripts/check.sh` — 19 of 19.

## What this does not do

- **The hole is still open.** This commit documents it accurately; it does not
  close it. The plain delete arm's failure branch has no test and will not have
  one until something else gives that fixture a foreign key.
- **No other arm was re-examined for the same problem.** I checked the one I had
  named. The other six go through the same `applied` call and are pinned by the
  four tests in the previous commit, but "pinned by a test that can fail" was
  only verified there by mutation for the arms those tests name.
- **Nothing was measured.** There is no code change to measure.
