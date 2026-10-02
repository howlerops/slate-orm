#!/usr/bin/env python3
"""Score `read_deliberate.py`'s later-entry signal against a labelled set.

# Why this exists

`ledger/2026-09-30-four-caveats-that-had-a-check-in-them.md` measured the
signal by hand, against the only two rows this repository had ever labelled
false-and-found, and recorded the result as a caveat rather than a number:

  > **One of two is not a validation rate.** The later-entry signal has two
  > labelled rows in the whole repository's history and now scores 1 on them.
  > Nothing here says what it would do on the next false verdict.

That is still true of the *rate* — two rows will not become a rate by being
re-divided — and it is no longer true of the *measurement*. The labelled set
lives in `docs/labelled-verdicts.json`, this scores the signal against every
row in it, and the next false verdict a read finds is a row somebody adds
rather than a measurement somebody re-does by hand. One of two becomes two of
three becomes a number worth reading, and nothing has to remember to re-run
the arithmetic.

# What it asserts, and what it deliberately does not

It asserts **no rate at all**. A signal that must score 2/2 is a signal
nobody will add an honest row to, and this repository's whole reason for
keeping a labelled set is that somebody adds the awkward ones. What it
asserts is that *the recorded score is the score the code produces* — so a
change that makes the signal worse fails here with both numbers printed, and
the only way past it is to write the new number down.

That is the `EXPECTED_REFUSALS` idiom again: a list you are forced to edit is
a list that stays true.

Run directly: `python3 scripts/test_read_deliberate.py`.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import read_deliberate

ROOT = Path(__file__).resolve().parent.parent

#: Where the entry that closed each labelled caveat falls in the signal's
#: ranking, and how many candidates it was ranked among.
#:
#: `(rank, candidates)` per row of `docs/labelled-verdicts.json`, in order.
#: `rank` is `None` when the closing entry is not a candidate at all.
#:
#: # Why a rank and not a hit rate
#:
#: The obvious measurement is "did the signal name the right entry", and it
#: is the wrong one at this sample size. Both labelled rows currently miss,
#: so a hit rate is 0 and *stays* 0 through changes that make the signal
#: much better or much worse — measured: widening the printed cut from three
#: to ten, and removing the skip that stops an audit entry crowding out the
#: real one, both leave a hit rate of 0/2 untouched. A rank moves under
#: both. With two labelled rows, resolution is the only thing there is.
#:
#: # These numbers were 1-of-2 four hours earlier
#:
#: `ledger/2026-09-30-four-caveats-that-had-a-check-in-them.md` measured the
#: signal by hand and found it named the closing entry for one row. This
#: file, the same day against the same two rows and the same code, finds it
#: names neither — and the difference is not a bug in either. It is the
#: ledger growing, which changes the signal with no change to the signal:
#:
#: - Row 1's closing entry is still a candidate, ranked **16th of 19**.
#:   `later_entries` prints `hits[:SHOWN]` with `SHOWN = 3`. A fixed cut
#:   against a candidate list that grows with the ledger drops the answer as
#:   soon as three later entries share two rare words, and ties break
#:   alphabetically, which favours *newer* filenames.
#: - Row 2's closing entry is **not a candidate**. "Rare" means "in fewer
#:   than a sixth of all entries", and at 523 entries that is 87. Words that
#:   were rare in a smaller ledger are ordinary in this one, so the claim is
#:   down to two rare words and stops clearing the threshold.
#:
#: That is the thing a hand measurement cannot show you, and the reason this
#: file exists rather than another paragraph in an entry.
#:
#: **Deliberately not tuned.** Widening the cut, or making the rarity cutoff
#: absolute, would very likely put both answers in view — against a labelled
#: set of two rows chosen after the fact. That is fitting the parameter to
#: the test set and the resulting number would mean nothing. The number to
#: move is the size of `docs/labelled-verdicts.json`.
#:
#: **2026-10-01: row 2's denominator moved 8 → 9, and the rank did not move.**
#: The ninth candidate is
#: `ledger/2026-10-01-a-check-the-derive-could-not-declare.md`, which shares
#: "default", "column" and "assertion" with a claim about a workbench
#: playground's default-column assertion and has nothing whatever to do with
#: it. One entry about a *different* default column joined the shortlist on
#: vocabulary alone, which is the signal's failure mode stated as cheaply as
#: it can be: the answer is still not a candidate and the noise grew by one.
#: Recorded rather than tuned, for the reason above.
#:
#: **2026-10-01, again: 9 → 10, and the rank still did not move.** The tenth
#: is `ledger/2026-10-01-twenty-moments-read-and-two-that-were-not.md`, which
#: shares "default", "assertion" and "column" with the same claim for the same
#: reason — it quotes a verdict about a default column. Two entries in one day
#: joined this shortlist by vocabulary, which makes the point the first note
#: made as a one-off into a rate: **this denominator grows with the ledger and
#: the numerator does not.** A signal whose candidate list grows and whose
#: answer does not appear is one whose precision decays as the repository
#: does, and that is worth more than either number on its own. Still not
#: tuned, and the reason has not changed: the labelled set has two rows.
RANKS: list[tuple[int | None, int]] = [(16, 19), (None, 10)]


def bodies() -> dict[str, str]:
    """Every ledger entry's text, keyed by filename — `read_deliberate`'s input."""
    return {
        one.name: one.read_text(encoding="utf-8")
        for one in sorted((ROOT / "ledger").glob("*.md"))
    }


def check() -> list[tuple[str, bool, str]]:
    """`(what, ok, detail)` per check, in the shape the other guards use."""
    out: list[tuple[str, bool, str]] = []

    path = ROOT / "docs" / "labelled-verdicts.json"
    if not path.is_file():
        out.append(("docs/labelled-verdicts.json exists", False, str(path)))
        return out
    rows = json.loads(path.read_text(encoding="utf-8"))["rows"]

    # The never-fires half. An empty set scores 0 of 0, which is a pass under
    # any arithmetic and means nothing was measured — the shape every guard
    # here carries a check against.
    out.append(("the labelled set has rows", bool(rows), f"{len(rows)} rows"))
    if not rows:
        return out

    text = bodies()
    missing = [
        f"{row['entry']} -> {row['closed_by']}"
        for row in rows
        if row["entry"] not in text or row["closed_by"] not in text
    ]
    out.append(
        ("every labelled row names two entries that exist", not missing,
         "\n      ".join(missing)),
    )

    got: list[tuple[int | None, int]] = []
    detail = []
    for row in rows:
        order = read_deliberate.ranked(row["key"], row["entry"], text)
        rank = order.index(row["closed_by"]) if row["closed_by"] in order else None
        got.append((rank, len(order)))
        shown = order[: read_deliberate.SHOWN]
        detail.append(
            f"{row['entry']}: {row['key'][:44]!r}"
            f"\n           wanted {row['closed_by']}"
            f"\n           rank   {rank} of {len(order)} candidate(s)"
            f"\n           shown  {shown or ['nothing']}"
        )
    out.append(
        (
            f"the signal ranks the closing entry at {RANKS}",
            got == RANKS,
            f"measured {got}, recorded {RANKS}\n      "
            + "\n      ".join(detail)
            + "\n      A rank that improved is worth writing down; one that got"
            " worse is worth\n      a ledger entry. Either way the number"
            " belongs in RANKS, not in nobody's head.",
        )
    )
    # And the cut, separately: how many of these the reader would actually
    # see. Kept apart from the rank so a change to `SHOWN` and a change to
    # the ranking fail as different lines.
    in_view = sum(
        1
        for (rank, _), row in zip(got, rows, strict=True)
        if rank is not None and rank < read_deliberate.SHOWN
    )
    out.append(
        (
            f"{in_view} of {len(rows)} closing entries are inside the printed cut",
            in_view == sum(1 for rank, _ in RANKS if rank is not None and rank < read_deliberate.SHOWN),
            f"SHOWN = {read_deliberate.SHOWN}",
        )
    )
    return out


def main() -> int:
    failed = 0
    for what, ok, detail in check():
        if ok:
            print(f"ok    {what}" + (f"  ({detail})" if detail and "\n" not in detail else ""))
        else:
            failed += 1
            print(f"FAIL  {what}" + (f"\n      {detail}" if detail else ""))
    print()
    print(f"{len(check()) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
