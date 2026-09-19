# Every bad field at once

## What changed

A write that fails three checks now reports three. `SchemaError::CheckViolation`
holds `Vec<CheckFailure>` instead of one check's name, the kernel collects
every failing check in declaration order, and the daemon puts each on the wire
as `check.N` / `column.N` / `message.N` beside a `violations` count.

## Why

The second of the validation note's three recommendations, and the one that
changes what a person sees. A form with four bad fields needed four round
trips: fix the first, discover the second, fix it, discover the third. That is
the behaviour per-attribute `errors` was invented to replace, and it is the
reason ActiveRecord validations are remembered for it.

## The cost hedge was wrong, and that is the finding

The note proposed this with a caveat: collect them all, but keep the
first-failure short circuit for callers that do not ask, so the common case
stays cheap. Writing it made the caveat collapse.

**A row that passes already evaluates every check.** That is what passing
means — the loop runs to the end. Short-circuiting only ever saved work on the
*failure* path, which is the rare one, and the saving is a handful of compiled
`Expr` evaluations against a row already in memory.

So there is no success-path cost to weigh, and the opt-in flag the note
imagined would have bought nothing while adding a second code path to keep in
step with the first. The function now says this in a comment, because the next
person to read it will have the same instinct the note did.

## Alternatives rejected

**Keep reporting one and add a count.** "Row violates 3 checks, the first is
`title_length`" is a smaller change and throws away exactly the thing that made
collecting them worth doing.

**Comma-join the names into one metadata value.** Needs a separator that cannot
occur in a name. Identifiers cannot contain a comma today and relying on that
is a constraint nobody wrote down — and it has no room for the per-failure
*message*, which is what a form actually renders. Indexed keys carry all three
fields per failure and cost a few more map entries on a path that is already
failing.

**Drop the unindexed `check` and `column`.** They were added one commit ago, so
removing them would have been churn, and they earn their place: most clients
show one error, and making them learn the indexed form to do the common thing
is a worse API. They are documented as the first failure, not as the only one.

**Sort or deduplicate the failures.** Declaration order is what the schema
author wrote and what a form's field order usually matches. Any other order is
a decision the caller cannot predict; a test pins it.

## Evidence

Six mutations, all caught:

| mutation | caught by |
| --- | --- |
| only the first failure is collected | `every_failing_check_is_reported_at_once` |
| passing checks are collected too | `a_row_that_fails_one_of_three_reports_only_that_one` |
| declaration order is reversed | `every_failing_check_is_reported_at_once` |
| one failure renders as the plural form | `a_single_failure_reads_the_way_it_always_did` |
| the wire reports only the first failure | `every_failing_check_reaches_the_client_…` |
| the violation count is always 1 | the same |

The second mutation is the one a careless implementation would ship: collecting
*every* check rather than every *failing* check compiles, and reports rules the
row satisfied. It needed its own test — a row that fails one of three — because
the three-failure test cannot tell the two apart.

`a_single_failure_reads_the_way_it_always_did` pins the exact string. A list of
one rendering as "row violates 1 checks on table `docs`" would be a gratuitous
break of every log line and every existing assertion, and is the kind of thing
that slips through when the interesting case is the new one.

`cargo clippy --workspace --all-targets`: clean. `cargo test` over the four
changed crates: 88 suites, no failures. The full-workspace run was not
attempted — it has now died at the linker on ENOSPC three times this session,
which is the container, not the code.

## What this does not do

No client surfaces the set. All three SDKs show the status message, which now
summarises every failure, so a person sees them; nothing reads `check.N` into a
structured per-field error yet. That is three small client changes and it is
not here.

The third recommendation — publishing checks so a client can evaluate them
before sending — is untouched. Every validation still costs a round trip; this
change makes one round trip tell the caller everything, which is a different
thing from not needing it.

Foreign keys still fail one at a time, and so does the unique-index check. A
row can be refused for a check, fixed, and then refused again for a duplicate
key. Widening the same treatment to those is a larger change into the write
path's ordering and was not attempted.
