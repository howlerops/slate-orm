# Fourteen *Stops at* lines re-read against the code; two had stopped being true, and one of them was a problem statement for a problem that has been solved

- **Date:** 2026-09-27
- **Author:** Claude Code, closing an open caveat from
  `ledger/2026-09-19-a-stops-at-that-stopped-being-true.md`
- **Touches:** `docs/orm-comparison.md`
- **Kind:** docs

## What changed

Every *Stops at* line in `docs/orm-comparison.md` — fourteen of them, across
P1, P2, P3, N1 (twice), N2, N3 (twice), N4 (twice), N5 (twice) and N6 (twice) —
was read against the wire and the kernel. Two statements were false and are now
struck through with what superseded them, in the form N4 already uses:

**P2** — *"One level, one relationship per request, to start. Nesting is P5."*
`RelatedRequest` carries a `path` of relationships resolved level by level in
one round trip, built by N1. A reader of P2 would have concluded a nested load
takes a round trip per level, which is the thing `path` exists to avoid.

**N3**'s problem statement — *"`JoinQuery` has `offset = 3` and no cursor.
Paging a join is therefore offset paging…"* — in a section marked **built**,
describing the problem that item solved. `JoinQuery` resumes after a primary
key in the first input's table. Somebody reading the section to find out
whether a join can be paged with a cursor would have read the paragraph that
says it cannot.

## Why

The caveat is exact: "Only N4 was checked. The sections either side describe
features this session did not touch. Their *Stops at* lines were not re-read
against the code, so this closes one instance and not the class."

The class turns out to have two more members, and the second is the more
interesting kind. N4's stale line was a *boundary* that had moved. N3's is a
**problem statement** left in the present tense after the problem was solved —
the paragraph is structurally "here is what is wrong today", and the item that
fixed it added its note above without touching it. Nothing about the section's
own shape flags that: the `>` block at the top says what happened and the plain
prose below is the original plan, so the plan reads as current and the
annotation reads as history, which is backwards.

## Alternatives rejected

**Delete both.** Rejected for the reason
`2026-09-19-a-stops-at-that-stopped-being-true.md` already gave and this
follows: the original plan text is the record of where the boundary *was*, and
these sections keep it on purpose. Both are struck through instead.

**Rewrite the plan text so it reads as history.** Prefixing every plan
paragraph with "as planned" or moving it under a heading would fix the reading
problem at the root, across all sixteen items. Rejected as a bigger change than
the caveat asks for, and one that would touch prose in items nobody has
re-read — the same mistake in the other direction. It is written up below as
what remains.

**Build the guard the sibling caveat asks for.** That caveat —
"Nothing checks a design note against the code" — is `deliberate` and stays so:
a note's claims are prose, a checker would need them marked, and the marking is
the work. What this does is the reading the guard would replace, once.

## Evidence

Each line was checked against a specific artefact, not recalled:

| item | *Stops at* | checked against | verdict |
|---|---|---|---|
| P1 | no predicate write across a join, one table | `DeleteWhereRequest.table` is a `string` | holds |
| P2 | one level, one relationship | `RelatedRequest.path` | **stale** |
| P3 | `RETURNING` is not a projection | `returning` is a `bool` | holds |
| N1 | no predicate/ordering/limit per level | `Relation` is `table`, `foreign_key`, `direction` | holds |
| N2 | one number, on the server | `DeleteWhere` returns `WriteResponse`, not a stream | holds |
| N3 | fan-out unbounded | the open caveat in `2026-09-17-a-path-of-relationships-on-the-wire.md` says the same | holds |
| N3 | `JoinQuery` has no cursor | `JoinQuery`'s resume-after field | **stale** |
| N4 | the token only | already superseded, 2026-09-19 | holds |
| N5 | the demo, no new server surface | — | holds |
| N6 | reporting, no CI gate on a timing number | `headbench` asserts exit codes only | holds |

- `python3 site/check/docs.py` and `python3 scripts/check_cited_docs.py` — 651
  citations, all openable.
- `sh scripts/check.sh` — 71 of 71.

## What this does not do

**It is a reading, and readings expire.** Two of fourteen were wrong after
eight days. Nothing here stops the third, and the guard that would is the one
the sibling caveat argues against building. The honest statement is that this
file needs re-reading whenever an item it describes is worked on, and the only
thing enforcing that is somebody remembering.

**The plan prose still reads as present tense.** Both corrections are strike-
throughs bolted onto paragraphs written as "here is what we will do" and read as
"here is what is true". The structural fix — marking plan text as plan — was
rejected above as out of scope, which means the next reader meets the same trap
in the twelve items nobody has had to correct yet.

**Only the *Stops at* lines were read.** Each item has a *Why*, a *Build* and a
*Test* paragraph too, and any of those can go stale the same way — N3's did,
which is how the second finding turned up, and it was found by accident while
reading the line above it. A full re-read of the file is several times this
much work and was not done.
