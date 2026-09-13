# The last two security findings: one inherent, one an ordering bug

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `crates/slate-kernel/src/{security,pool}.rs`,
  `crates/slate-server/src/service.rs`, three test files, `docs/security-review.md`
- **Kind:** security

## What changed

**Finding 8 is fixed.** The four handlers that fingerprint-check now authorise
first, so a caller with no grant cannot confirm a table's shape.

**Finding 4 is closed as inherent**, with `security.rs`'s claim narrowed to what
is actually true — and with a correction to the finding itself.

## Why

These were the last two open findings from the adversarial review, and both had
been sitting on "someone should look at this properly" rather than on a
decision. Finding 8 is a genuine defect with a small fix. Finding 4 is not
fixable, and leaving it open implied it might be — which is worse than closing
it honestly, because an open finding reads as work outstanding rather than as a
property of the design.

The review also recorded finding 4's scope from reasoning rather than from a
test, and that turned out to be wrong in a way worth catching.

## Finding 4, and where the review was wrong

The review's own assessment was that the insert oracle is inherent, and that is
right: a unique key is shared by everyone who can write the table, so the only
ways to withhold "is this key taken" are to overwrite the hidden row or to
accept a write that cannot be stored. Postgres has the same oracle.

But the finding also said the invariant held for "`update`, `upsert` and
`delete`", and **`upsert` does not hold it**. An upsert onto a free key succeeds
and onto a key held by an invisible row is refused; that is the same bit. This
is worth more than a footnote because a careful caller reaching for an upsert
*specifically to avoid* the insert oracle would be picking it for a property it
does not have.

I also checked the worse possibility the finding did not raise: whether an
upsert *overwrites* the hidden row. It does not — the `USING` check runs against
the existing row first. That is now pinned by a test rather than assumed, and it
was worth the ten minutes: a silent overwrite would have been a far more serious
defect than the oracle.

`security.rs` now states per-path what is disclosed, and names the two things
that bound it: same-tenant only, and only where the attacker can name the key
(a UUID or sequence key leaves nothing to probe).

## Finding 8

`fingerprint::check` ran before anything authorised the caller, so a role with
no grant on `users` could send a guessed `(name, type, key)` layout and learn
from the answer whether the guess was right. Now `authorized_table` resolves and
authorises in one step, ahead of the fingerprint.

## Alternatives rejected

**Changing the insert path to hide the bit.** Either overwrites another
principal's row or accepts a write that will not land. Both are worse than the
oracle.

**Reporting `NOT_FOUND` for a table the caller has no grant on.** Would close
the existence half of finding 8. Rejected: it hides a table from someone who
cannot use it anyway, and makes every ordinary misconfiguration — the common
case by a wide margin — indistinguishable from a typo. Postgres makes the same
call. Stated in `authorized_table` so the next reader knows it was a decision.

**Exposing `security()` on `ReplicaPool`.** `snapshot_from`'s doc comment argues
at length against handing out the components, because a caller then assembles
its own view. `ReplicaPool::authorize` gives the *answer* instead of the
ingredient, which respects that argument and adds one method rather than two.

**Trusting the planner's check alone.** It is the check that protects the rows
and it still runs. But it runs *after* everything the handler does first, and
the disclosure was in that gap.

## Evidence

935 workspace tests pass; fmt and clippy clean. Five mutations, all killed —
after two rounds:

- Restoring the old order (fingerprint before authorise) fails
  `a_caller_with_no_grant_cannot_confirm_a_tables_shape`.
- **Authorising the wrong action initially survived all four handlers.** The
  fixture's `app` role holds `EVERYTHING` on `users`, so checking `Explain`
  where `Delete` was meant denied nobody. The fixture gained four single-action
  roles (`reader_only`, `inserter_only`, `updater_only`, `deleter_only`) and
  each handler is now exercised by a role holding exactly its one action. All
  four wrong-action mutations now fail.

That second one is the same shape as two earlier misses this week: a test whose
assertion is satisfied by the bug. A blanket grant cannot detect a wrong
action, exactly as an ordering that matches the default cannot detect a dropped
sort.

## What this does not do

The insert/upsert oracle is still there — it is inherent, and closing the
finding means the documentation now matches, not that the behaviour changed. A
deployment whose row existence is a secret and whose primary keys are
attacker-chosen has to design around it; `security.rs` says so.

Table existence remains disclosed to an authenticated caller. Deliberate, and
recorded as such rather than left open.

Only the four fingerprint-checking handlers authorise early. `query`, `join`,
`aggregate` and `explain` still rely on the planner's check alone — correct,
because they do no shape-confirming work before reaching it, but it does mean
the early check is not a uniform policy and a future handler could reintroduce
the gap.
