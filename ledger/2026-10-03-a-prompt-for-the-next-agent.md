# A prompt for the next agent, holding only what does not move

- **Date:** 2026-10-03
- **Author:** Claude Code (session: what's next, after the release)
- **Touches:** `docs/agent-handoff.md`, `CLAUDE.md`
- **Kind:** docs

## What changed

`docs/agent-handoff.md` is a prompt to paste as the first message to a fresh
agent. Its first instruction is to run `python3 scripts/handoff.py`.

## Why

`scripts/handoff.py` answers *where the tree is*. It deliberately does not
answer *how to work here* or *what to pick up next* — the second is a
person's call and the script says so. So handing the work over still took a
human writing the same three paragraphs each time, and the parts most worth
saying are the ones easiest to forget: that a green `check.sh` is necessary
and not sufficient, that four things are refused rather than forgotten, and
that three things only the owner can do.

**The split is the design, and it is the lesson this repository keeps
relearning.** A handoff document that states the state is wrong the first
time anybody commits. So this file holds only what does not move — standing
rules, pointers, the two scoped tasks and their constraints — and its opening
line tells the agent to trust the script over the text:

> Do not trust any state described below over what that prints — this text is
> standing rules, the script is the truth.

## Alternatives rejected

**Generating the prompt from the tree too.** The appealing answer, and it
cannot produce the useful half. "Write the design note first, in the style of
`views.md`" and "do not publish to npm, the names are unclaimed" are
judgements, not facts about the tree; a generator would either omit them or
invent them. The honest division is: facts derived, judgements written and
dated.

**Leaving it in chat, as the previous handoffs were.** It works once. A
prompt that lives in a conversation is gone when the conversation is, which
is exactly the situation somebody handing off is in.

**Listing every open caveat rather than two tasks.** There are 1,376
`deliberate` verdicts; dumping them is the 138 KB mistake
`ledger/2026-10-03-a-briefing-that-reads-the-tree.md` already made once. Two
tasks with their constraints is a starting point; the script's live edges are
the breadth.

**Omitting the DO NOT list.** Shorter, and it is the half that saves the most
time — an agent that re-does the cross-compile, or helpfully publishes to
npm, costs more than one that does nothing. Each entry names where its
argument lives rather than restating it.

## Evidence

**Both citation guards pass** — 1,255 doc citations and 151 file paths, every
one resolving, including the four `ledger/` and `docs/` paths this file
points at. That is the only mechanical check that applies: the prompt's value
is its judgement, and nothing can test that.

**Not tested: whether the prompt actually works on a fresh agent.** It has
not been handed to one. The claim it makes about itself — that an agent
following it starts usefully — is untested, and the first real handoff is the
test.

**No mutation testing.** Prose. The code it points at is mutated in the two
entries beside this one.

## What this does not do

**The two tasks and four refusals will rot.** They are the hand-maintained
half, named as such in the file itself, and keeping them true is part of
whatever change lands T3 or T4. Nothing enforces that — no guard can tell a
stale judgement from a current one.

**It does not brief a human.** It is written to be pasted at an agent; a
person arriving cold still wants `CLAUDE.md` and the script.

**It assumes the next agent is a Claude Code session in this container.**
The disk notes, `scripts/reclaim.py` and the `gh api` line are specific to
that. An agent somewhere else gets advice that is mostly right and partly
about a machine it is not on.
