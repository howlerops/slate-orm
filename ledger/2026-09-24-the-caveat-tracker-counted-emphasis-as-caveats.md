# The tracker built yesterday read `**12**` in the middle of a sentence as a caveat, and dropped the first caveat of every section. Both directions, found within the hour.

- **Date:** 2026-09-24
- **Author:** Claude Code, working #301 (F7b)
- **Touches:** `scripts/caveats.py`
- **Kind:** fixing the tool before trusting its output

## What changed

`BULLET` required `^\*\*` with `re.MULTILINE`, which matches **any** line
beginning with bold — including emphasis that merely happens to land at a line
break. `2026-09-21-the-demo-ui-is-a-subset-on-purpose.md` contains

    ... it is
    **12** of 24, not 11, because ...

and `12` was extracted as a caveat.

A caveat now has to lead its *paragraph*: a blank line, then `**`. Tightening
that alone broke the other direction — `SECTION`'s `\s*$` eats one of the two
newlines after the heading, so the **first** caveat of every section lost its
paragraph boundary and vanished. 183 of them, silently. The section text is
prefixed with a blank line before matching, which restores the boundary
without loosening the pattern.

## Why

This was caught because tightening the pattern orphaned two verdicts written
an hour earlier, and the tracker reports an orphan rather than quietly
reverting the caveat to untriaged. That reporting was written yesterday for a
different reason — a reworded bullet — and it caught a tool bug instead.

Had the tighten landed without it, the count would have fallen from 762 to 579
and looked like *progress*.

## Alternatives rejected

**Loosen `BULLET` to allow a single newline.** One character, and it
re-admits exactly the class just removed — mid-paragraph emphasis after a line
wrap is preceded by a single newline, which is the phantom's own shape.

**Strip the leading newline inside `SECTION`.** Equivalent, and it puts the
knowledge of how bullets are delimited inside the pattern that finds sections.
The two patterns already have to agree about one blank line; normalising at the
call site is where a reader looking at either one will find the reason.

**Widen `SECTION` to keep both newlines.** `\s*$` is doing real work — the
heading may or may not have trailing spaces — and making it non-greedy to
preserve a newline is a subtler change than adding one where it is needed.

## Evidence

Hand-counted against `2026-09-21-the-demo-ui-is-a-subset-on-purpose.md`: three
lines in that section begin with `**`, two paragraphs begin with `~~`. The
extractor returns **2** — the two real caveats — excluding the struck-through
withdrawals and the mid-sentence `**12**`.

Before: 762, of which one is demonstrably not a caveat. Tightened only: 579,
missing 183 real ones. Fixed: **762 caveats, 48 open, 9 closed, 33 deliberate,
672 untriaged**, and no orphaned verdicts.

The count is the same by coincidence — the entries added since yesterday
account for the difference — and that coincidence is why the hand count is
here rather than the number.

**Fourteen tests**, three of them written for these two bugs: a section's
first caveat is found, emphasis at a line break is not one, and a
struck-through withdrawal is not one. Two mutations, both caught — reverting
either fix fails a named case, in
`ledger/mutations/20260924T141239-scripts-caveats-py.json`.

`scripts/check.sh` at **53 passed, all of them.**

## What this does not do

**The struck-through case is tested by shape, not by history.** The suite
writes `~~**Withdrawn.**~~` and requires it not to count. A real withdrawal in
this ledger is a multi-paragraph passage with the correction beside it, and
whether every such passage is excluded has not been checked against the real
entries — only that the opening `~~` is what decides.

**The 762 is still not audited.** One entry was hand-counted. Whether the
pattern mis-reads some other formatting — a bullet inside a list, a caveat
whose lead is italic rather than bold — is unknown, and the only evidence for
the total is that one file matched.
