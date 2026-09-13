# Grouped joins and ordered groups reach the wire, now that the kernel has them

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `slate-server` — `records.proto`, `convert.rs`, `service.rs`, `session.rs`, `lib.rs`, `tests/multi.rs`; `README.md`
- **Kind:** feature

## What changed

`AggregateQuery` gains `join`, and `sort`/`limit`/`offset` over groups.
`GroupedRead` splits its source into `GroupedSource::Table` and
`GroupedSource::Join`, and both the transactional and replica paths dispatch on
it. Three inputs is refused with the reason. Five wire differentials.

## Why

The protocol said aggregating a join would mean "a second implementation of
grouping living in the head node, over rows it had already streamed" — losing
the projection narrowing that makes `COUNT(*)` read no columns at all, with
nothing to be an oracle against. That was right when it was written and stopped
being right when the kernel grew `group_by_join`: the wire now carries the
request to the kernel and implements nothing. Same for ordering over groups,
which the kernel gained at the same time.

The protocol comment outlasted the reason for it, which is the failure mode
`CLAUDE.md` calls out: a doc that is read as current and is not.

## Alternatives rejected

**A separate `GroupedJoin` RPC.** Rejected because the response is identical —
a stream of `Group` — so a second RPC would duplicate the streaming, the
batching, the `served_by` header and the warnings, to vary one field of the
request.

**Accepting three inputs and grouping the chain in the head node.** Exactly the
thing the original comment refused, and still refused: the kernel does not group
a chain, and writing one here would put a second grouper in the node with
nothing to check it against. Refused with that as the message.

**A precedence rule for a request naming both `input` and `join`.** Whichever
rule was chosen would be a thing to remember and a thing to get wrong. Both set
is a client bug and is reported as one.

**`tables: Vec<TableId>` on the join source.** How it was written first, and
clippy's `indexing may panic` was right about it: "exactly two" was checked in
one place and indexed in three. It is a pair in the type now, so the consumers
cannot be wrong.

## Evidence

930 tests pass across the workspace, fmt and clippy clean. Five wire
differentials: a grouped join agreeing with the kernel, a group key on the
*right* side of the join, ordered-limited-offset groups compared **in order**,
a chain refused with the reason, and both sources named refused.

Three mutations, and all three survived the first version of these tests:

- **Narrowing the joined space to the first input** passed everything, because
  every group key in the tests was on input 0.
  `a_group_key_on_the_right_side_of_the_join_resolves` was written for it.
- **Dropping the sort** and **dropping the offset** both passed, because the
  ordering the test asked for — descending by count — happened to agree with
  the kernel's default of ascending by encoded group key on this fixture. The
  test now sorts *ascending* by count, where the two genuinely disagree, and
  additionally asserts that the requested order differs from the default, so a
  future fixture change that makes them coincide again fails loudly rather than
  quietly weakening the test.

All three are killed now. The mutation harness itself was wrong twice on the
way here — a shell split on `::`, and a `replace` with no assertion that it
matched — both of which reported "survived" for mutations that were never
applied. Fixed by a small script that exits non-zero when its anchor is absent.

## What this does not do

Grouping a chain is not built, in the kernel or here. A grouped join is still
not *costed* as grouped: the join is planned as if its rows were being
returned, so a plan cheaper to group than to stream is not preferred. Both stay
in the README's not-built list.

`aggregate_to_proto_query` still emits the single-table shape only — it exists
for the typed client, and a caller wanting a grouped join builds the message.
Nothing yet exercises `having` over a grouped *join*, only over a grouped
table.
