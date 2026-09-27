# Three entries closed four caveats and added thirteen. The open count went up. That is the tracker working, and it is worth saying out loud before somebody reads the number as failure.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `docs/caveat-status.json`
- **Kind:** bookkeeping, and an observation about the shape of the work

## What changed

Thirteen verdicts, for the caveats the three disjunction entries wrote. One
of them closes: **"No `OR` on a join or in `HAVING`"** — half done by the
`HAVING` work, and the half that remains, a join's `WHERE`, now carries its
own `deliberate` verdict with the reason rather than sitting inside a caveat
that reads as entirely open.

`786 caveats: 285 open, 169 closed, 284 deliberate, 0 untriaged.`

## The number went up, and that is correct

Before this session's feature work: 279 open. After closing four: **285**.

Every entry written here ends with *What this does not do*, and this
repository's standards make that section substantial. Three entries about
disjunctions produced thirteen new caveats — no nesting, no client surface,
no measurement, the planner not using one, a join's `WHERE` left out. All
true, all worth knowing, none of them existed before the feature did.

So the count is not a burn-down and should not be read as one. **Closing a
caveat means doing work; doing work means writing an entry; writing an entry
means recording what it stopped short of.** A project that added features
without the count rising would be one that had stopped saying where its
features end.

What the tracker actually measures is the *frontier*: the set of things known
to be undone. A rising frontier alongside a rising `closed` count means the
work is real and the honesty is holding. The number to watch is `untriaged`,
which is the only one that can silently hide the truth, and it is zero.

## Alternatives rejected

**Stop counting the caveats a new entry adds.** Would make the number go
down. It would also make it a lie: the thirteen are exactly as real as the
four, and the only difference is which day they were written.

**Weight them, so a "no measurement" counts less than a missing feature.**
Tempting and unfalsifiable. The verdicts already carry the distinction that
matters — `deliberate` is a decision, `open` is work — and a second axis of
judgement is one more thing to argue about per caveat.

**Say nothing and let the reader work it out.** What the previous three
entries did. A person who sees 279 become 285 across a session described as
"closing the backlog" will reasonably conclude something went wrong, and the
answer should not require reading twelve commits.

## Evidence

`python3 scripts/caveats.py` exits 0: no orphans, no problems, nothing
untriaged. The arithmetic: 279 open before, 4 closed, 13 opened by the three
new entries, minus 1 reclassified — 285. Every one of the thirteen is
`open`, `deliberate` or `moment` by the same rules the 677 were triaged
under.

## What this does not do

**The previous commit bypassed the hook.** `--no-verify`, on a commit
touching only `docs/caveat-status.json`. That is exactly the case
`CLAUDE.md` permits it for and exactly the case it asks for an entry
afterwards, which is this file. The reasoning was nowhere for one commit.

**It does not project.** If every feature entry adds three or four caveats
and closes two, the frontier grows. Whether that converges depends on how
much of the remaining 285 is feature work versus test and measurement work,
and nothing here estimates it.

**No test asserts the arithmetic above.** The counts were read off two runs
of the tracker, by hand, and the 279 comes from a run recorded in a chat
message rather than from anything committed. `caveats.py` reports a state,
not a history; nothing stores yesterday's numbers to diff against.
