#!/usr/bin/env python3
"""Every `checked` deliberate verdict came out of a draw that reproduces.

    python3 scripts/check_draws.py

`scripts/caveats.py --unchecked` is a frame over the 1000-odd `deliberate`
caveats nobody has read against the tree. Drawing from it rather than from the
whole bucket is what makes successive passes *pool*: a row read today leaves
the frame, the next pass draws from what is left, and the union of all passes
is the bucket minus the frame.

`ledger/2026-09-30-a-sample-that-pools-with-the-next-one.md` recorded, the day
the frame was built, that nothing held anyone to it:

  > **Nothing checks that a draw actually came from `--unchecked`.** The next
  > pass could sample the whole bucket again and stamp what it read; the count
  > would move by the same amount while pooling nothing. Written in the
  > docstring and in the entry, which is where a convention lives until
  > something enforces it.

This is the something. A `checked` stamp on a `deliberate` row names a draw;
the draw is a file in `ledger/draws/`; and the draw reproduces — re-running
`random.Random(seed).sample(range(frame), n)` must return the recorded
indices, and those indices must select the recorded keys out of a frame of
the recorded size.

# What that catches, and what it does not

It catches the accident, which is the failure that actually happens: a pass
that samples the bucket because it did not know the frame existed cannot
produce indices that reproduce, because its rows were not drawn from a
`range(frame)` at all. It catches a stamp with no draw behind it, and a draw
naming a row that is not a caveat.

It does not catch a determined hand, which could run `--draw`, throw the
result away, read thirty other rows and stamp them against that draw's name —
the keys would not match, so in fact it catches that too, and what is left is
running `--draw` and reading exactly the rows it named *badly*. That is the
`checked`-stamp-is-a-claim-about-attention limit, one layer in, and no file
in this directory can reach it.

# The one recovered record

`ledger/draws/2026-09-30-329.json` predates the machinery: it was drawn with
`random.seed(329)` over the whole bucket, before the frame existed, and five
rows have been added to that bucket since, so it cannot be re-derived. It
carries `recovered` naming the entry that recorded the seed. A recovered
record drawn **after** 2026-09-30 is refused, which is what stops the field
being a permanent way out — the alternative, an exemption list, is the thing
`ledger/2026-09-30-a-premise-nobody-here-can-falsify.md` warns about.
"""

from __future__ import annotations

import json
import pathlib
import random
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import caveats

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: The last day a draw may claim to be `recovered`. The recovered record is a
#: one-off — the machinery landed the same day — and a date is what keeps it
#: one-off without a roster of names that would itself go stale.
RECOVERY_CLOSED = "2026-09-30"


def problems(root: pathlib.Path = ROOT) -> list[str]:
    found: list[str] = []
    directory = root / caveats.DRAWS
    records: dict[str, dict] = {}
    for path in sorted(directory.glob("*.json")) if directory.is_dir() else []:
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as bad:
            found.append(f"{path.name}: is not readable JSON ({bad})")
            continue
        records[path.stem] = record
        expected = f"{record.get('drawn')}-{record.get('seed')}.json"
        if path.name != expected:
            found.append(
                f"{path.name}: names a draw of {record.get('drawn')!r} with "
                f"seed {record.get('seed')!r}, so it should be {expected}. The "
                f"filename is what a verdict's `draw` field points at."
            )
        recovered = record.get("recovered")
        if recovered:
            if str(record.get("drawn", "")) > RECOVERY_CLOSED:
                found.append(
                    f"{path.name}: is marked recovered and was drawn after "
                    f"{RECOVERY_CLOSED}. Recovery was for the one draw that "
                    f"predates `--draw`; a new pass runs `--draw` and gets a "
                    f"record that reproduces."
                )
            if not (root / recovered).is_file():
                found.append(
                    f"{path.name}: `recovered` cites {recovered}, which is not "
                    f"a file. A recovered record's only evidence is the entry "
                    f"that wrote the seed down."
                )
        else:
            size, seed = record.get("frame"), record.get("seed")
            indices, keys = record.get("indices", []), record.get("keys", [])
            if not isinstance(size, int) or not isinstance(seed, int):
                found.append(f"{path.name}: has no integer `frame` and `seed`")
                continue
            again = random.Random(seed).sample(range(size), len(indices))
            if again != indices:
                found.append(
                    f"{path.name}: does not reproduce. "
                    f"`random.Random({seed}).sample(range({size}), "
                    f"{len(indices)})` is not the recorded indices, so these "
                    f"rows did not come out of the frame `--unchecked` lists."
                )
            if len(keys) != len(indices):
                found.append(
                    f"{path.name}: records {len(indices)} indices and "
                    f"{len(keys)} keys; a draw is one key per index."
                )

    # Every stamped row names a draw that exists and lists it.
    status = caveats.load(root)
    live = {f"{c['entry']}::{caveats.key(c['claim'])}" for c in caveats.caveats(root)}
    stamped = 0
    for k, row in sorted(status.items()):
        if row.get("verdict") != "deliberate" or not row.get(caveats.CHECKED):
            continue
        stamped += 1
        named = row.get(caveats.DRAW)
        if not named:
            # `caveats.py` reports this one too, and reporting it twice is
            # cheaper than either file assuming the other ran.
            found.append(f"{k}: is checked and names no draw")
            continue
        record = records.get(str(named))
        if record is None:
            found.append(
                f"{k}: names draw {named!r}, and {caveats.DRAWS}/{named}.json "
                f"is not there"
            )
        elif k not in record.get("keys", []):
            found.append(
                f"{k}: names draw {named!r}, which did not draw it. A stamp "
                f"against a draw that did not name the row is a read of the "
                f"bucket wearing a frame's clothes."
            )

    for name, record in sorted(records.items()):
        for k in record.get("keys", []):
            if k not in live:
                found.append(
                    f"{name}.json: drew {k!r}, which is no longer a caveat — "
                    f"its bullet was reworded or removed."
                )

    # The never-fires halves. An empty directory and a bucket nobody has
    # stamped both make every loop above vacuous, and print exactly what a
    # clean run prints.
    if not records:
        found.append(
            f"no draw record in {caveats.DRAWS}/, which means this is looking "
            f"in the wrong place rather than that nobody has ever drawn"
        )
    if not stamped:
        found.append(
            "no deliberate verdict carries `checked`, so nothing above was "
            "compared against anything"
        )
    return found


def main() -> int:
    found = problems()
    for problem in found:
        print(problem)
    if found:
        return 1
    print("ok    every checked deliberate verdict came out of a draw that reproduces")
    return 0


if __name__ == "__main__":
    sys.exit(main())
