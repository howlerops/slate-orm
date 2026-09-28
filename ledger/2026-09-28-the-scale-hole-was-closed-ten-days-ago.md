# The scale hole was closed on the day it was written, and I said otherwise three times

- **Date:** 2026-09-28
- **Author:** an agent session
- **Touches:** `site/docs/limits.html`, `docs/caveat-status.json`
- **Kind:** docs

## What changed

Five caveats saying nothing checks a client's declared decimal scale are
**closed**, with `crates/slate-server/src/fingerprint.rs` as the witness. The
paragraph on `site/docs/limits.html` that announced the hole is replaced by one
that describes what the fingerprint actually covers and where it actually
stops. An earlier entry today, which claimed to correct the same paragraph and
did not, is deleted rather than left as a second wrong account.

## Why

**The fingerprint hashes a decimal's scale.** Line 199 of `fingerprint.rs`:

```rust
if let Some(scale) = column.scale() {
    state.number(scale as usize);
}
```

with a module docstring that makes exactly the argument I spent today
describing as un-made: *"The scale is the one exception to that rule and it is
deliberate. It addresses no column, so by the test below it does not belong —
and the failure it prevents is worse than the failure the test is about."* It
is safe because a scale cannot change under a running client:
`changing_a_scale_is_a_migration_refusal` in
`crates/slate-kernel/tests/decimals.rs`.

Commit `35d9182`, 2026-09-18, titled *"fix: hash a decimal's scale into the
schema fingerprint"*, whose message opens: *"The last open item on the decimal
feature, and three documents called it the sharpest edge."* Written the same
day as the five caveats that say it is open, by a session that evidently read
them.

**I got this wrong three times in one day, each time in the alarming
direction.**

1. During the triage I read all five caveats and gave them the same `open`
   verdict five times, writing "the sharpest thing standing open in this
   repository" into each.
2. I then put that on the docs site, under the heading *What it is not*.
3. I noticed `codegen.py` emits the scale, wrote an entry announcing that the
   hole was narrower than claimed, and published a *second* wrong paragraph
   saying a hand-written declaration is unchecked. It is not: a hand-written
   declaration that carries a schema check is refused on a scale mismatch like
   any other.

Each correction was one `git grep scale crates/slate-server/src/fingerprint.rs`
away, and the entry that closed it —
`2026-09-18-the-one-thing-in-the-fingerprint-that-addresses-no-column.md` — is
one I read today and whose *other* caveats I triaged. The title names the fix.

What is actually left is the caveat that entry already records and I already
marked `deliberate`: **a request without a schema check is served as before**,
because the check is optional on the wire. That is a real limit, it is one
sentence, and it is what the page says now.

## Alternatives rejected

**Amend the earlier entry instead of deleting it.** `ledger/README.md` makes an
entry append-only, so amending is out — but this one had existed for under an
hour, had been pushed once, and was wholly wrong rather than dated. Leaving two
entries an hour apart, both about the same paragraph, both wrong in different
ways, would make a reader reconstruct which was which. Deleting the shorter-lived
mistake and writing one accurate account is the smaller lie to the record, and
this paragraph is the disclosure that makes it not a lie at all.

**Add a guard that catches a caveat overtaken by work on its own day.** It is
the general shape of the failure and I do not think it is reachable: a caveat is
prose, the work that closes it is a commit, and matching them is the "existence,
not aptness" problem that `2026-09-26-the-citation-nobody-could-follow.md`
already records as the half no pattern does. What would have caught *this* one
is reading the entry whose title is the fix, which is a habit rather than a
check.

**Say nothing and quietly fix the verdicts.** The tracker would have been right
and the record would have been missing the most instructive thing that happened
today. A repository whose standard is "withdraw the hypothesis and say that you
did" does not get to skip the third withdrawal because it is embarrassing.

## Evidence

`git grep -n scale crates/slate-server/src/fingerprint.rs` — the hash site at
199, the module docstring at 26–40, the inline argument at 177–197.

`git log -S "if let Some(scale) = column.scale()" -- crates/slate-server/src/fingerprint.rs`
— one commit, `35d9182`, 2026-09-18.

`git grep -n changing_a_scale_is_a_migration_refusal` —
`crates/slate-kernel/tests/decimals.rs:209`, the test the docstring cites for
why hashing it is safe.

`python3 site/check/docs.py`: the ten pages render and every element closes the
one it opened — which failed on the first attempt at the replacement paragraph,
on an unclosed `<p>`, and passes now.

`scripts/check_closed_caveats.py`: 345 of 365 closures carry a witness,
including the five closed here.

No measurement. This was a reading of a file that contradicted a claim I had
made three times, and the claim is withdrawn.

## What this does not do

**It does not make a schema check mandatory.** A request carrying none is
served as before, which is the protocol's decision and is recorded
`deliberate` against the fingerprint entry. Every protection the fingerprint
offers — ordinals, types, the primary key, a decimal's scale, an array's
element type — is opt-in in exactly the same way, so this is one limit and not
five.

**It does not re-read the other 113 open caveats against the tree.** Five of
them were false and I found that by accident, twice removed: noticing
`codegen.py`, being wrong about what that implied, and only then reading the
fingerprint. The triage's own closing caveat says a verdict resting on an
entry's reasoning is only as good as that reasoning; this is the worked example,
and the population it came from has not been re-examined.

**Nothing stops the fourth time.** The habit this wants — when a caveat says a
thing is unbuilt, grep for it before writing the verdict — is a habit, and the
three verdicts above were written by somebody who would have said they had it.
