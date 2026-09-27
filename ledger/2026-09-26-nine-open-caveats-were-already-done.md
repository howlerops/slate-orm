# The `open` list audited for work that is already shipped: nine moved, and four of them were closed by work done later the same day in the same session

- **Date:** 2026-09-26
- **Author:** Claude, working from a request to audit what actually needs doing
- **Kind:** process
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`, `ledger/`

## What changed

Every one of the 199 `open` caveats was checked against the tree for work that
has **already been done**. Six were, and three more were half done:

| caveat | what already exists |
|---|---|
| `the-helper-that-can-be-used-now`: *Go and TypeScript never restore against a real server* | the conformance runner's `/api/restore` and `/api/restore-unchanged` cases, run against all three adapters against a live head node |
| `the-helper-that-can-be-used-now`: *The demo does not show it* | `panels.tsx:653` — a restore panel with a `restore-run` control and a `restore-summary` readout |
| `a-verdict-is-not-a-reading`: *The reverse direction was not swept* | `2026-09-26-the-reverse-sweep-found-six.md`, four hours later |
| `the-verdict-sweep-finished…`: *The reverse sweep is still not done* | the same entry — two caveats, two files, one job |
| `the-reverse-sweep-found-six`: *The 195 `closed` verdicts have never been re-read* | `the-closed-verdicts-audited.md`, then `check_closed_caveats.py` |
| `the-closed-verdicts-audited`: *Nothing re-reads a `closed` verdict when the thing that closed it is reverted* | `check_closed_caveats.py`, which does exactly that on every run |

And to `narrowed`: *Nothing checks a `by`* (the path half is checked), *Nothing
tests the daemon's `max_concurrent_requests` or `request_timeout`* (the
configuration half is, over the real binary), *It still reads Rust only* (the
Markdown half was widened).

`open` falls 199 → 190, `closed` rises 197 → 203, each new closure with a
witness row so `check_closed_caveats.py` re-checks it.

## Why

Because a caveat that says a thing is missing when the thing is shipped is not
a harmless stale record — it is an instruction to build it again. The tracker
is the answer to "what is left to do", and every wrong entry in it is a
duplicated afternoon.

**Four of the six were closed by work done later the same day, in this
session.** That is the finding. The reading pass on 2026-09-26 stamped every
open caveat as read; the two sweeps after it sorted `open` from `deliberate`;
none of them asked *has this been done since*, because each ran once over a
list that was still growing underneath it. An entry writes its caveats at the
moment it is committed, and the next entry an hour later can close one without
anything noticing — the tracker keys a verdict to a claim, not a claim to a
subject, so two entries describing one job are two rows that never meet.

The restore pair is the same shape at a five-day remove:
`the-helper-that-can-be-used-now.md` recorded two gaps on 2026-09-20 and both
were filled *that day* by `three-sdks-agree-about-a-restore.md` and
`the-undo-a-visitor-can-see.md` — whose own equivalent caveats **were** closed,
against those very entries. One of each pair was found and the other was not,
because the pass that found them read entries and not subjects.

## Alternatives rejected

**Re-read the open list the way the earlier passes did.** Three passes had
already read all 199 asking "is this still true?" and this one found nine they
missed, so a fourth reading of the same kind was the wrong instrument. What
worked was asking a different question — *does the thing this says is missing
exist in the tree?* — and answering it with `git grep` rather than with
judgement. Half the nine came out of two greps.

**Cluster the open caveats against the closed ones by word overlap.** Built and
run: a Jaccard score over each open caveat's paragraph against all 197 closed
ones, everything above 0.20 printed. Nine pairs, of which **one** was a real
duplicate and the rest shared vocabulary and nothing else — "nothing", "still",
"clients", "guard" are the house style, not a subject. It is in the session and
not in the repository, because a detector with an eight-in-nine false-positive
rate is one nobody runs twice.

**Add a `subject` field so two caveats about one job collide.** It is the fix
for the mechanism rather than the instance, and it is the wrong shape: a
subject taxonomy over 900 caveats is a second hand-maintained roster, invented
by one reader, that goes stale exactly like the first. The cheaper thing that
works is what this entry does — re-run the *has this been done* question after
a session that shipped things, which is a habit rather than a field.

**Leave the three half-done ones `open`.** They read as more missing than is
missing, which is the case `narrowed` was added for, and the daemon-limits one
in particular would send somebody to write configuration tests that already
exist beside the ones they would write.

## Evidence

**199 open caveats checked.** Nine moved: six `open` → `closed`, three `open` →
`narrowed`. Each closure verified by reading the tree, not by inference:

```
examples/explorer/conformance/conformance.py:654
    ("a retired row can be restored", "/api/restore", {}, "app"),
examples/explorer/web/src/panels.tsx:682
    data-test="restore-run"
crates/slate-serverd/tests/refusals.rs:504
    fn a_zero_concurrency_limit_is_refused_because_it_would_serve_nobody()
scripts/check_cost_prose.py:126
    README_GLOBS = ("README.md", "clients/*/README.md", "examples/*/README.md")
```

**The yield, and what it says.** 9 of 199 is 4.5%, against 6 in 322 (1.9%) for
the `deliberate` sweep and 1 in 195 (0.5%) for the `closed` audit. The `open`
list is the *least* accurate of the three, which is the opposite of what the
tracker's design assumes — `open` is the class that gets re-read most often and
it is the one that rots fastest, because it is the only class the rest of the
work can invalidate without touching it.

**Four of the six closures are same-session.** The two restore caveats are five
days old; the four tracker ones were written between two and six hours before
the work that closed them.

`python3 scripts/check_closed_caveats.py`: 187 of 203 witnessed, 16 exempt.
`python3 scripts/caveats.py`: no problem, no orphan; `--unread 30` reports 0.
`sh scripts/check.sh`: 65 passed, all of them.

**No mutation run.** The diff is nine verdicts, six witness rows and this entry;
`scripts/test_check_closed_caveats.py` covers the roster mechanism and is
untouched.

## What this does not do

**Nothing stops the next same-session duplicate.** The mechanism is that a
caveat is keyed to its own words and not to the job it describes, so an entry
can close another entry's caveat silently. This audit is a pass, not a guard,
and the alternatives section says why the guard shapes considered were worse
than the habit. The habit — re-ask *has this been done* after a session that
shipped something — is written here and nowhere a tool reads.

**The 190 that remain were checked for "already done", not re-read for truth.**
A caveat that was never true, or has become false for a reason other than the
work being finished, is not what this pass looked for. That reading happened on
2026-09-26 and its own limits are recorded in
`ledger/2026-09-26-the-backlog-is-read.md`.

**The word-overlap detector is measured and discarded.** One real pair in nine
at a 0.20 threshold; a lower threshold floods and a higher one finds nothing.
Whether a better similarity measure exists over this corpus is a question this
did not ask — it asked whether the obvious one works, and it does not.

**Six greps decided six closures.** Each was read in context before the verdict
moved, but a grep that hit for an unrelated reason would look exactly the same
at the moment of deciding, and the witness rows now defending those closures
were chosen from those same greps.
