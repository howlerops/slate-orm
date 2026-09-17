# The next six are the edges the first six left

- **Date:** 2026-09-17
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** docs

## What changed

The plan in `docs/orm-comparison.md` and its shorter form on the site now say
that all six of P1–P6 are built, and set out six more. The gap table loses two
rows that are now built and gains one that was always there and had not been
noticed.

Nothing in the code changed. This is the plan catching up with the branch.

## Why

**The plan had gone stale in a way the document itself calls out as the worst
kind.** Its opening paragraph says it "will go stale, and this paragraph is the
instruction for what to do when it has", and four table rows and the whole
roadmap page were describing a repository that stopped existing several commits
ago. The site was the worse of the two: it told a visitor that predicate writes
were "not yet on the wire" and that batched relationship loading was "reachable
only from Rust", both of which had been false for days.

**The new six are not drawn from the table.** They are drawn from what the last
six left behind, and four of them are debts the previous plan created:

1. **`through` and nested loading on the wire.** P4 and P5 built them in Rust
   and stopped there. This is the third time on this branch a capability has
   existed only in Rust — `Related`, predicate writes, and now these. The first
   two were caught a commit later; this one has been open since P4 landed. It
   is first for that reason rather than because it is the biggest.
2. **A ceiling on `returning`**, labelled a hypothesis rather than a finding,
   because it is a reading of the code and not a run. See below.
3. **Keyset paging over a join.** `Query` got a cursor in P3; `JoinQuery` still
   has `offset = 3` and no cursor field. Offset paging over a join is the exact
   thing the cursor exists to replace, and we shipped the replacement for one
   read shape out of two.
4. **The reason token on the lone path**, already written up as an asymmetry
   and not yet fixed.
5. **The demo**, which shows none of the last three features.
6. **A client-side batch measurement**, because the 9.4× is a Rust number and
   the README makes a claim about clients on the strength of it.

Ordering is value over cost, same as the first six, which puts the one real
capability gap first and the two honesty items last.

## The one new gap, and it is a hypothesis

`WriteResponse` is unary, `returning` puts every matched row in it, and nothing
bounds the count. `grep` for `max_encoding_message_size` and
`max_decoding_message_size` across `crates/slate-server/src/` and
`crates/slate-serverd/src/` returns nothing, so tonic's defaults apply: no
encode cap, a 4 MiB decode cap at the client.

If that reading is right, a large enough `delete_where(…, returning = true)`
**commits and then fails to deliver** — the rows are gone, the client refuses
the response, and the caller sees a decode error for a write that succeeded.
That would be a known outcome converted into an unknown one, which is precisely
the case the error taxonomy tells callers not to retry.

**This is written as N2's first step, not as a finding.** Nothing here
reproduces it. The item says to reproduce it at whatever row count crosses
4 MiB and, if it does not reproduce, to withdraw the claim in place and say
what actually happens instead.

## Alternatives rejected

**Leave the six as they are and start N1 directly.** The work would be the
same and the document would still be wrong, which is the thing this repository
treats as worse than having no document. The site page in particular is read by
people who cannot check it against the source.

**Renumber the new items P7–P12.** Continuous numbering implies one plan of
twelve, ordered throughout, which would be a claim that N1 was always ranked
below P6. It was not: it exists because P4 and P5 stopped at the Rust edge.
`N`-prefixed says these were chosen after the first six were done and in light
of them.

**Rewrite each built P-item to describe what it became, in place.** Tidier to
read and it destroys the record. Two of the six turned out to be different
features than they were written as — P3's `RETURNING` shrank to predicate
writes, P5's depth limit was refused on inspection — and a reader who only sees
the outcome cannot tell that the plan was wrong about them, which is the most
useful thing those items have to say. The annotate-above-keep-below shape is
kept, and the preamble now names the three items whose value was the lesson.

**Drop the gap table's built rows silently.** The table's own convention, set
when keyset pagination landed, is to remove a row that was correct when written
and to annotate one that was wrong. Both new removals (batch, `RETURNING`) were
correct when written, so they are removed — but with a note saying so, because
a row vanishing with no trace is indistinguishable from a row nobody ever
wrote.

**Add "five check defects to two code defects" as a claim without counting.**
The preamble now states that ratio. It is counted, not felt: the `protoc` guard
that never ran, `max_batch_operations` with nothing exercising it, the
durability check served by a stale replica, the mutation harness whose filter
missed its own test, and four mutation survivors that were one unasserted
schema claim in three languages.

## Evidence

`python3 site/check/docs.py` passes — every element closes the one it opened on
both pages, every relative link resolves.

Each claim written into the two documents was re-checked against the source
rather than recalled, which is what the audit's opening paragraph asks for:

| claim | checked by |
| --- | --- |
| `RelatedRequest` is one relation, one level | read the message: `Relation relation = 2`, `repeated Value keys = 3` |
| `JoinQuery` has no cursor | read the message: `optional uint64 limit = 2`, `uint64 offset = 3`, no `after` |
| no message-size limits are configured | `grep` for `max_encoding_message_size`/`max_decoding_message_size` in the two server crates → nothing |
| all three clients drop binary trailers | the three comments saying so, in `error.go`, `errors.py`, `errors.ts` |
| Go and TypeScript have no retrying `transact` | `grep` for `Transact`/`transact`: Go has `Begin`/`Commit`/`Rollback` only, TypeScript has neither |
| `slate-serverd` has no per-request logging | `grep` for `tracing::info`/`log::info` in `service.rs` → nothing |

## What this does not do

It does not write N3's design note. That note — what a join's cursor is when
the driving side's key is not unique after a fan-out — is the substance of the
item, and writing it here would be doing the item in the plan.

It does not touch `README.md`, which describes the surface rather than the
plan and is current as far as this change is concerned. The one claim in it
worth revisiting is the batch one, and that is N6's job rather than this
entry's: changing the wording without the measurement would trade one
unsupported sentence for another.

The three "smaller, and owed" items at the end of the new plan are listed
without being ranked against the six. They are things to pick up while already
in the file, and pretending to have costed them would be inventing a number.
