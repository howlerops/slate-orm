# The application declares the rules it used to only document

- **Date:** 2026-10-01
- **Author:** Claude Code (session: finish the backlog, the docs and the examples)
- **Touches:** `examples/helpdesk`
- **Kind:** feature

## What changed

`examples/helpdesk` declares two of the three things it was written to show
it could not declare.

`Ticket::status` carries a `CHECK` — the four words are the schema's now, not
this crate's — with the sentence a form shows and the column it goes beside.
`Comment` carries two foreign keys: into `tickets`, cascading, and into
`agents`, restricting. Both are composite, because both parents are
tenant-scoped and a key into one names `tenant_id` first.

Its README strikes row 1 of the gap table, and a test that asserted the gap
now asserts the opposite.

## Why

`ledger/2026-09-30-an-application-written-against-the-rust-surface.md` built
this crate to find out what `#[derive(Record)]` could not say, and found
three things. Two of them were closed this morning
(`ledger/2026-10-01-a-check-the-derive-could-not-declare.md`,
`ledger/2026-10-01-a-foreign-key-the-derive-could-not-declare.md`) — and the
application went on documenting them as gaps.

That is the stale-documentation failure CLAUDE.md names, in the place it is
most expensive: an example is read as *how you do this*, so a doc comment
saying "the derive cannot declare a `CHECK`, so the constraint lives in the
service method" teaches a reader to write the workaround.

**And the application had the gap, not just the documentation.** `status` was
validated in `Helpdesk::open_ticket` and nowhere else, so a caller reaching
the store directly wrote whatever it liked — which the test suite
demonstrated on purpose. Closing the feature without changing the example
would have left the hole in the one place somebody copies from.

## The test that predicted its own death

`the_same_status_written_past_the_service_is_not_refused` asserted that the
store accepted `"opne"`, with this comment:

> The other half of the finding, demonstrated rather than argued: the store
> takes `"opne"` without complaint, because nothing below this crate knows
> the four words. **If the derive ever grows `check`, this test starts
> failing and that is the signal to delete it.**

It started failing. It was **inverted rather than deleted**, and that is the
decision worth recording: the assertion it can make now — *the rule is below
the service, so going around `open_ticket` is refused too* — is worth more
than the one it made before. The old name said what was absent; the new one
(`..._is_refused_by_the_schema`) says what holds. Deleting it would have
thrown away a test and left the new guarantee untested.

## Alternatives rejected

**Deleting the application-level check in `open_ticket`.** Now redundant: the
schema refuses the row either way. Kept because the two refusals are not the
same experience — `HelpdeskError::NotAStatus` names the four words before any
I/O, and a `CheckViolation` arrives after a round trip. Belt and braces is
the right shape when the braces are free, and the cost of the duplication is
paid by `the_statuses_and_the_check_agree`.

**Deriving `STATUS_PATTERN` from `STATUSES`.** Two spellings of one list is
the duplication this repository usually refuses. It stays because the two are
read by different things — a Rust `contains` and a regular expression the
kernel compiles — and bridging them means either building the pattern at
runtime, so the attribute cannot take it (an attribute needs a `const`), or
parsing the pattern, so a typo in it surfaces as a parse error at the wrong
layer. The test runs every word of `STATUSES` through the check and one word
that is not in it, which is what makes the duplication a cost rather than a
hazard.

**`on_delete = cascade` on both comment edges.** Symmetrical and wrong in
opposite directions. A comment on a deleted ticket is unreachable, so it
should go; a comment by a deleted agent is *what they said*, and erasing it
to make a delete convenient is the wrong trade for an audit trail. The two
words are the whole difference and the test deletes rather than reading them
back, because reading a word back passes whether or not the kernel ever sees
it.

**Leaving the hand-written `TICKETS_TABLE` without the check.** It would have
compiled, passed `the_hand_written_tickets_table_matches_the_derived_one`
(which compares what it compares) and been a constraint no write enforced —
`Record::table()` for `Indexed` is `TICKETS_TABLE`, and that is the table the
store sees. Exactly the silent-empty-index failure
`ledger/2026-09-15-a-new-index-returns-nothing.md` is about, with a rule
instead of an index. Which is why the mutation below is the first one listed.

## Evidence

**Three new tests and one inverted**, in `examples/helpdesk/tests/helpdesk.rs`;
23 passing.

| test | what it pins |
|---|---|
| `the_same_status_written_past_the_service_is_refused_by_the_schema` | the typed `CheckViolation`, its column and its message, and that nothing was written |
| `the_statuses_and_the_check_agree` | all four words pass the check and `"opne"` does not |
| `a_comment_on_a_ticket_that_does_not_exist_is_refused` | `ForeignKeyViolation` naming `comments_ticket` |
| `deleting_a_ticket_takes_its_comments_and_deleting_an_author_does_not` | cascade and restrict, by deleting |

**Five mutations, all caught**
(`ledger/mutations/20261001T161405-examples-helpdesk-src-lib-rs.json`):

| mutation | caught by |
|---|---|
| the hand-written table loses the check, so the writes are unguarded | 2 tests |
| the pattern admits a fifth word | 2 tests |
| the check names `priority`, so a form blames the wrong field | 1 test |
| the ticket edge restricts instead of cascading | 1 test |
| the author edge cascades instead of restricting | 1 test |

The first is the one that matters: it is the shape where the derived table is
correct and the table the writes actually use is not.

**Two things the test run taught, both by refusing.** The restrict probe
first came back `access denied: no role grants delete on table 'agents'` —
correct security, and an `is_err()` assertion would have called it a foreign
key. The caller is the superuser now and the comment says why. Then it came
back `ForeignKeyRestricted` where the test said `ForeignKeyViolation`: two
different refusals, and naming the wrong one would have let an insert-side
violation pass as a delete-side restrict.

**Not measured.** A check is one regex per write on a column already being
encoded; a foreign key is a point read the kernel was already making for the
tenant prefix. Neither was timed and neither is near a budget.

## What this does not do

**The third gap is still open and is the reason `TICKETS_TABLE` exists.**
`#[record(index(...))]` has no `text`, the search box needs an inverted
index, and a `TableDef` is immutable once built — so the whole table is still
restated by hand. Row 2 of the README's table, unchanged.

**Nothing exercises these constraints through a client.** The same caveat
both derive entries carry: the daemon declares its tables in TOML, so no
deployed path reaches a Rust-declared check or key. This is the Rust surface
end to end and stops at the socket.

**The `agents` delete is reachable only by the superuser.** Which makes the
restrict test a statement about the constraint and not about anything an
agent can do — correct for what it tests, and it means the *interaction*
between the grant and the constraint is untested. An agent who could delete
an agent would meet the restrict; nobody can, so nobody does.
