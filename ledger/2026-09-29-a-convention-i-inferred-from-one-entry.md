# A convention I inferred from one entry

## What changed

Two corrections and one caveat closed. No new code.

`2026-09-29-a-caveat-closed-by-a-line-through-it.md`, written four hours ago,
said this ledger's convention for a caveat answered later is to strike it **and
leave the original bullet standing underneath**. It is not. Counted:

| struck caveats | with a live twin | without |
| --- | --- | --- |
| 51 | 3 | 48 |

The convention is to strike the caveat and write the closure *inside* the
strike. Keeping the original as well is a thing **three** entries do, one of
which explains why it is doing it. The entry and the `caveats.py` docstring
both now say the measured thing, with the correction left visible rather than
smoothed over.

The caveat *"Nothing stops the pair being created wrongly in the first place"*
is closed by the same count: there is no convention to enforce.

## Why

The morning's rule — a caveat that also stands struck through may not be
recorded `open` — is right, and it found a real stale verdict on its first run.
Its **reason** was wrong. I read one entry, which announces its own practice in
a sentence, and wrote that practice up as what the ledger does.

The cost of a wrong reason attached to a right rule is not the rule; it is the
next rule. That caveat proposed exactly the wrong second one: *an entry that
strikes without keeping the original … is not checked either way*, offered as
something the pre-commit hook should catch. Building it would have failed 48
entries and forced the repository to adopt a convention three entries follow.
The guard would have looked principled and been a mass rewrite.

This is the same shape as two other findings today, which is why it is written
up rather than quietly fixed:

- the generated-declaration guard passed on the word "schema" appearing in 483
  files, and the fix came from a failing test rather than a reading;
- the `4n + 2` was explained from the encoding rules and only became a fact
  when the messages were subtracted.

Three times in one session, the same error: a plausible reading of one example,
written down as a property of the whole. Each time the correction cost one
count. The habit worth extracting is not "read more carefully" — it is **when
a sentence says "the convention is X", count X before publishing it.**

## Alternatives rejected

**Quietly reword the entry.** The repository's convention here is the opposite,
and the entry being corrected argues for it in its own Alternatives section.
The wrong sentence is struck-and-replaced with the count beside it.

**Delete the rule too.** Tempting symmetry — a rule whose stated reason was
wrong looks suspect. But the rule does not depend on the reason: it fires on a
struck caveat that has a live twin recorded `open`, and those three pairs
exist whatever the convention is. It caught one wrong verdict this morning and
would catch the next.

**Build the pair-completeness guard anyway, exempting the 48.** A roster of 48
exemptions for a rule three files satisfy is a rule with the sign flipped. If
anything the majority shape is the one worth enforcing, and it needs no rule:
a strike with the closure inside it is self-describing.

**Count only the entries, not the caveats.** 3 of 51 is caveats; by entry it is
3 entries of however many strike anything. The caveat count is the right
denominator because the rule operates on caveats, and quoting the friendlier
figure would be choosing a number for its shape.

## Evidence

The count, from `caveats.py`'s own `units`, `LEAD` and `key`, so it counts
exactly what the rule counts:

```
struck caveats: 51 total, 3 with a live twin, 48 without
```

Two of the 48 read by hand to check the count means what it says —
`2026-09-19-a-purge-something-can-call.md` (`~~**No scheduler.** …~~ **Closed**
— examples/retention/ is that example`) and
`2026-09-19-the-arm-the-refusal-never-reaches.md` (`- ~~**The hole is still
open.**~~ **Closed the next day** — see …`). Both strike the claim and put the
closure in the strike; neither leaves a live copy.

`python3 scripts/test_caveats.py` — 51 passing, unchanged: the rule is
untouched and only its docstring moved.

`python3 scripts/caveats.py` — 1716 caveats, and the tally is unchanged by the
corrections, which is the check that nothing was reworded past its key.

## What this does not do

**It does not check the majority convention either.** An entry that strikes a
caveat and writes no closure inside the strike would say a claim was answered
and name nothing that answered it. That is the rule worth having, it is the
mirror of the `by`-is-required rule for `closed`, and it is not written — this
entry found the wrong rule and stopped rather than building the right one in
the same breath.

**One count, one day.** 3 of 51 is today's tree. Nothing keeps the docstring's
number honest as entries are added, and a count in prose is exactly the thing
this repository's guards exist to stop going stale.
