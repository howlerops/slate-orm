# Five passes read 246 `deliberate` verdicts and none could be pooled, because a read against the tree had nowhere to be written down

- **Date:** 2026-09-30
- **Author:** Claude Opus 5
- **Touches:** `scripts/caveats.py`, `scripts/test_caveats.py`, `docs/caveat-status.json`
- **Kind:** process

## What changed

`checked` — the stamp meaning "somebody read this caveat against the tree and
believes it is still true" — is now allowed on a `deliberate` verdict, where it
was refused. `caveats.py --unchecked` lists the `deliberate` caveats nobody has
read, and the summary line carries the count, so it moves on every run.

Today's thirty reads were restamped from `reviewed` to `checked`, which is what
they were. The frame starts at **1008 of 1038**.

## Why

Between 2026-09-28 and 2026-09-30, five entries sampled the `deliberate`
bucket and read 246 verdicts, finding seven false. **None of the five can be
pooled with another.** Two reasons, and the second is the fixable one:

- None of the first four records its seed or its read set, so the overlap
  between any two is unknown. That is unrecoverable and is not what this
  changes.
- A read against the tree could only be stamped `reviewed`, because
  `caveats.py` refused `checked` on a `deliberate` row — and `reviewed` is
  also what the 2026-09-26 reverse sweep put on 316 rows to mean the much
  weaker "somebody re-read the entry's prose". The two are indistinguishable
  after the fact, so even a pass that stamped everything it read left no
  usable trace.

So every pass re-drew from the whole bucket. `2026-09-30-thirty-more-…`
had to publish its pooled rate as "an optimistic bound rather than a count",
and could not say how much of its own thirty was new.

The refusal's reasoning was sound: *"`--unread` reads only `open` and
`narrowed`, so a `deliberate` row wearing a `checked` is a stamp nothing will
ever look at again, pretending to be one that will."* Sound, and backwards.
Something should look at it. Drawing the next sample from `--unchecked`
instead of from the whole bucket makes the passes pool with nobody recording
anything: a row read today leaves the frame, and the union of every pass is
`1038 - len(--unchecked)`.

## Alternatives rejected

**A third date field**, `read`, beside `checked` and `reviewed`. Rejected
because `checked` already means exactly this — read against the tree — and
three date fields with two meanings between them is how the current confusion
started. One meaning per field, and the prohibition goes.

**Recovering the earlier draws** by reproducing their seeds. Not possible;
none is recorded anywhere, and the `reviewed` dates cannot separate a campaign
read from the reverse sweep's bulk stamp. Stated as unrecoverable rather than
attempted.

**Making `--unread` cover `deliberate` too**, rather than adding a second
command. It would drown the list it already has: 146 rows known to be undone,
against 1038 asserted to be fine. Different questions, different cadences —
`--unread` defaults to 30 days, `--unchecked` to "ever", because re-reading
1038 rows on a timer is not a thing anyone will do.

**Leaving it and continuing to sample blind.** The cost is not the wasted
re-reads; it is that no number anybody publishes means anything. Seven false
of 246 is a rate only if the 246 are distinct.

## Evidence

`python3 scripts/test_caveats.py`: 64 passed, 0 failed — six new cases for
`--unchecked`, and the case that tested the old prohibition rewritten to test
the narrower one that survives it (`closed` still refuses `checked`).

Four mutations, all caught, in
`ledger/mutations/20260930T025818-scripts-caveats-py.json`:

| mutation | caught by |
| --- | --- |
| a `reviewed` stamp counts as read | ``reviewed` does not count as read: it is prose against prose` |
| no window silently means thirty days | `no window means ever, so an old read still counts` |
| the frame is drawn over `open`, not `deliberate` | four cases at once |
| `deliberate` goes back to refusing `checked` | `a deliberate verdict carrying \`checked\` is accepted` |

The frame, after restamping today's reads:

```
$ python3 scripts/caveats.py
1923 caveats: 146 open, 117 narrowed, 476 closed, 1038 deliberate (30 read
against the tree), 0 untriaged
$ python3 scripts/caveats.py --unchecked | tail -1
1008 deliberate and not read against the tree, ever
```

## What this does not do

**It does not recover the 216 reads before today.** They are real and they are
not in the count, so `30 of 1038` understates what has been read by up to 216.
Every later pass is poolable; this one starts the ledger from today rather
than pretending to know what came before.

**1008 rows at thirty a session is thirty-four sessions**, which nobody is
going to run. The frame is not a plan to empty it: it is what makes each draw
informative, and the finish line is there so that "we have read some of them"
stops being the answer.

**A `checked` stamp is still a claim about a person's attention.**
`ledger/2026-09-25-the-open-caveats-nobody-re-reads.md` measured what that is
worth on the open backlog — right about six times in seven — and nothing here
makes it worth more. A rubber-stamped row leaves the frame exactly like a read
one.

**Nothing checks that a draw actually came from `--unchecked`.** The next pass
could sample the whole bucket again and stamp what it read, and the count
would move by the same amount while pooling nothing. The convention is written
here and in the docstring; it is not enforced.
