# The scale-hole correction reached the five caveats it was sourced from and not the two roll-ups quoting them

- **Date:** 2026-09-29
- **Author:** Claude Code
- **Touches:** `docs/caveat-status.json`, `scripts/test_caveats.py`
- **Kind:** docs

## What changed

Two `open` verdicts are **`narrowed`**, each with a residual. Both say that a
client's declared decimal scale is unchecked against the server's, and both are
half false: `crates/slate-server/src/fingerprint.rs` has hashed a decimal's
scale since commit `35d9182` on 2026-09-18.

`scripts/test_caveats.py` gains a case that runs `caveats.report()` against the
**real** `docs/caveat-status.json`, which is what makes the change above
mutation-testable at all.

## Why

`ledger/2026-09-28-the-scale-hole-was-closed-ten-days-ago.md` found the fix and
closed the five caveats that recorded the hole. It did not touch two others
that quote the same claim second-hand:

- `2026-09-28-the-invisible-backlog-is-triaged.md`: *"It does not fix any of
  the 118. … The largest is a client's declared decimal scale that nothing
  checks against the server's, which three entries record independently and
  which renders money a hundred times wrong in silence."*
- `2026-09-28-the-invisible-backlog-part-three.md`: *"The two grouped findings
  are named, not fixed. The scale hole and the response-head latency are each
  one piece of work now instead of six sentences, and neither has been
  started."*

Both are triage roll-ups. Neither is one of the five, because neither is the
entry the claim came from — they *restate* it while summarising a backlog. So
a correction that walks from the fix to the caveats about the fix finds five of
seven, and the two it misses are the two most likely to be read: a reader
asking "what is the largest thing still open?" lands on the roll-up, not on the
source entry.

Two things make this worth an entry rather than a silent verdict edit.

**It survived two passes aimed at exactly this.** The 2026-09-28 census read
the 99 open caveats from earlier entries; these two are from that day, so it
excluded them. `ledger/2026-09-29-the-days-own-caveats-read-and-a-phrase-that-
did-not-survive-counting.md` then read all 33 of the day's own, found eight,
and did not find these — because both bullets read as true if you check only
their *first* sentence. "It does not fix any of the 118" is true of that entry
forever; the false part is the sentence after it.

**It names the shape.** A caveat whose claim is sourced from another entry goes
stale when the source is corrected, and nothing connects the two. The
`retired_claims` machinery exists for exactly this class in prose — it excludes
`ledger/` deliberately — and the tracker has no equivalent.

The second change is smaller and was forced by the first. `scripts/mutate.py`
could not score a change to `docs/caveat-status.json`, because no suite read
the real file: `scripts/test_caveats.py` builds fixture trees, and
`scripts/caveats.py` prints a summary rather than a `N passed, M failed` line
any dialect recognises. Breaking a verdict in the real tracker left every suite
green. One case fixes that, in the shape `test_check_closed_caveats.py` already
uses: the real tree, last, so a failure reads as "the tracker drifted".

## Alternatives rejected

**Closing both outright.** Both name two things and one of the two is still
true: the response-head latency in one case, the other 117 of the 118 in the
other. `narrowed` exists so that a half-answered claim stops reading as a whole
one without erasing the half that holds.

**Leaving both `open` and fixing only the `by`.** Tempting, because the bullets'
*first* sentences are true, and a verdict's `by` is reasoning rather than claim.
Rejected because the falsehood is inside the caveat, not only inside the
verdict: the roll-up's own words say the scale is unchecked and that money
renders a hundred times wrong. A reader of `caveats.py --open` sees the bullet,
not the `by`.

**A guard that flags a caveat quoting another entry's claim.** This is the
mechanical version of the finding and it does not work, for the reason
`ledger/2026-09-28-the-census-finished-and-three-more-were-already-answered.md`
measured for the neighbouring idea: quoting is not marked, so detecting it
means matching phrases across 200 entries, and the two instances here share no
distinctive phrase with the five they quote — one says "declared decimal scale",
the five say "scale". A phrase list tuned until it catches these two is a list
fitted to two examples. The honest mechanism is the one already here: when a
correction closes a caveat, grep the *claim* rather than walking the entry.

**Adding the real-tree case as a separate change.** It exists only because this
change needed it, and splitting it would leave an entry claiming a mutation
test it could not have run.

## Evidence

- **The fix predates both bullets by eight and ten days.**
  `crates/slate-server/src/fingerprint.rs:177`, comment *"A decimal's scale, and
  only a decimal's"*, from commit `35d9182` (2026-09-18, *"fix: hash a decimal's
  scale into the schema fingerprint"*). `README.md:1344` already records it as
  done, with the same reasoning — so the repository's own front page and its
  caveat tracker disagreed.
- **Nothing else carries the claim.** A search of every `by`, `residual` and
  `key` in `docs/caveat-status.json` for `declared decimal scale|decimal scale a
  client|scale nothing checks|scale that nothing checks|hundred times wrong`
  returns exactly these two rows. Outside the ledger the only match is
  `README.md:1349`, inside the `[x]` bullet that records the fix.
- **Mutation, against the real tracker**, `scripts/mutate.py`, record
  `ledger/mutations/20260929T132922-docs-caveat-status-json.json`: three cases,
  three caught, each by the named new case `the real tracker: every verdict is
  well formed`:
  - emptying the first residual → caught (a `narrowed` verdict with no residual
    says what closed and not what is left);
  - emptying the second → caught;
  - altering one caveat's `key` by a character → caught, as an **orphan**,
    which is the other half of the case and would otherwise have been
    unexercised.
  Each was a change: the first two replace a sentence with `""`, the third
  changes the text the verdict is keyed by. An earlier attempt at the same
  three is recorded beside it as
  `ledger/mutations/20260929T132855-docs-caveat-status-json.json`, outcome
  `interrupted`: it deleted whole `"residual"` lines, which `mutate.py` refused
  because the anchor did not occur — the residual is the object's last key, so
  removing the line would have left a dangling comma and scored invalid JSON
  rather than a missing residual.
- **The case is load-bearing and was not before.** Run against the tree as it
  stood this morning, all three mutations above leave `scripts/test_caveats.py`
  at `51 passed, 0 failed` — there was nothing to catch them.
- `python3 scripts/caveats.py`: 1760 caveats, **121 open** (was 123), 96
  narrowed, 439 closed, 964 deliberate, 0 untriaged.
- `python3 scripts/test_caveats.py`: 52 passed, 0 failed.

## What this does not do

**It does not find the next one.** Two roll-ups quoted this claim; whether any
other entry quotes a claim that has since been corrected is unexamined, and the
alternatives above argue no cheap rule finds them. The count that would decide
whether a rule is worth building is "how many caveats restate a claim sourced
elsewhere", and nothing measures it.

**The new test case checks the tracker's form, not its truth.**
`report()` verifies that every verdict is well formed, keyed to a caveat that
exists, and carries what its kind requires. A verdict that is well formed and
wrong — which is precisely what these two were for ten days — passes it. That
is the same limit `check_closed_caveats.py`'s docstring states about witnesses
and is restated rather than solved.

**One of the two mutations is weaker than it looks.** Emptying a residual and
emptying the other residual are the same mutation applied twice; they
demonstrate the rule fires, not that it fires for two independent reasons. The
orphan case is the one that adds coverage, and it was added after noticing
that.
