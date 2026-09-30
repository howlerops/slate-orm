#!/usr/bin/env python3
"""`check_draws.py`, over ledgers this file writes.

Against written trees rather than against `slate-orm`, for the reason every
other guard test here gives: a check run only over this repository asserts
that today's tree is clean, which is also what a check that does nothing
asserts.

The cases that matter are the two the guard exists for — a stamp with no draw,
and a draw whose indices did not come out of `range(frame)` — and the two
never-fires halves, because an empty `ledger/draws/` and an unstamped bucket
both make every loop in the guard vacuous and print what a clean run prints.

Run directly: `python3 scripts/test_check_draws.py`.
"""

from __future__ import annotations

import json
import random
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_draws as guard

RESULTS: list[bool] = []

#: One ledger entry with one caveat per claim, so `caveats()` finds them.
ENTRY = """# t

## What this does not do

{bullets}
"""


def tree(claims: list[str], verdicts: list[dict], draws: dict[str, dict]) -> Path:
    root = Path(tempfile.mkdtemp())
    (root / "ledger").mkdir()
    (root / "docs").mkdir()
    (root / "ledger" / "2026-01-01-e.md").write_text(
        ENTRY.format(bullets="\n\n".join(f"**{c}** why." for c in claims)),
        encoding="utf-8",
    )
    (root / "docs" / "caveat-status.json").write_text(
        json.dumps({"verdicts": verdicts}), encoding="utf-8"
    )
    if draws:
        (root / "ledger" / "draws").mkdir()
        for name, record in draws.items():
            (root / "ledger" / "draws" / f"{name}.json").write_text(
                json.dumps(record), encoding="utf-8"
            )
    return root


def row(claim: str, **extra: str) -> dict:
    base = {
        "entry": "2026-01-01-e.md",
        "key": claim[:60],
        "verdict": "deliberate",
        "by": "a reason",
    }
    base.update(extra)
    return base


def case(name: str, root: Path, wanted: list[str]) -> None:
    found = guard.problems(root)
    unseen = [w for w in wanted if not any(w in f for f in found)]
    extra = [f for f in found if not any(w in f for w in wanted)]
    ok = not unseen and not extra
    print(f"{'ok  ' if ok else 'FAIL'}  {name}")
    if not ok:
        for u in unseen:
            print(f"        expected and did not get: {u}")
        for e in extra:
            print(f"        got and did not expect:   {e}")
    RESULTS.append(ok)


def drawn(keys: list[str], size: int, seed: int) -> dict:
    """A record that reproduces, over a frame of `size` holding `keys` first."""
    indices = random.Random(seed).sample(range(size), len(keys))
    return {
        "drawn": "2026-01-02",
        "seed": seed,
        "frame": size,
        "digest": "x",
        "indices": indices,
        "keys": keys,
    }


def main() -> int:
    claims = [f"Claim number {n}." for n in range(6)]
    keyed = [f"2026-01-01-e.md::{c[:60]}" for c in claims]

    # The clean case. One row stamped, one draw that reproduces and names it.
    record = drawn([keyed[0]], size=6, seed=7)
    case(
        "a stamp naming a draw that reproduces and lists it is clean",
        tree(
            claims,
            [row(claims[0], checked="2026-01-02", draw="2026-01-02-7")]
            + [row(c) for c in claims[1:]],
            {"2026-01-02-7": record},
        ),
        [],
    )

    # The failure the guard exists for: a stamp with no draw behind it.
    case(
        "a checked row naming no draw is refused",
        tree(
            claims,
            [row(claims[0], checked="2026-01-02")] + [row(c) for c in claims[1:]],
            {"2026-01-02-7": record},
        ),
        ["is checked and names no draw"],
    )

    # And the other shape of the same failure: a pass that sampled the whole
    # bucket and wrote a record to match. Its indices do not reproduce.
    faked = dict(record, indices=[0], keys=[keyed[0]])
    if faked["indices"] == record["indices"]:  # pragma: no cover - seed-dependent
        faked["indices"] = [1]
    case(
        "a record whose indices did not come out of the frame is refused",
        tree(
            claims,
            [row(claims[0], checked="2026-01-02", draw="2026-01-02-7")]
            + [row(c) for c in claims[1:]],
            {"2026-01-02-7": faked},
        ),
        ["does not reproduce"],
    )

    # A stamp against a draw that did not name the row.
    other = drawn([keyed[1]], size=6, seed=7)
    case(
        "a stamp against a draw that did not draw the row is refused",
        tree(
            claims,
            [row(claims[0], checked="2026-01-02", draw="2026-01-02-7")]
            + [row(c) for c in claims[1:]],
            {"2026-01-02-7": other},
        ),
        ["which did not draw it"],
    )

    # A draw naming a row that is no longer a caveat.
    gone = drawn(["2026-01-01-e.md::Claim number 99."], size=6, seed=7)
    case(
        "a draw naming a key that is not a caveat is refused",
        tree(
            claims,
            [row(c) for c in claims],
            {"2026-01-02-7": gone},
        ),
        ["is no longer a caveat", "no deliberate verdict carries `checked`"],
    )

    # The recovery hatch: allowed on the day it was written, refused after,
    # and refused when its citation does not resolve.
    recovered = {
        "drawn": "2026-09-30",
        "seed": 1,
        "frame": 6,
        "keys": [keyed[0]],
        "recovered": "ledger/2026-01-01-e.md",
    }
    case(
        "a recovered record citing a real entry is allowed",
        tree(
            claims,
            [row(claims[0], checked="2026-09-30", draw="2026-09-30-1")]
            + [row(c) for c in claims[1:]],
            {"2026-09-30-1": recovered},
        ),
        [],
    )
    case(
        "a recovered record drawn after the hatch closed is refused",
        tree(
            claims,
            [row(claims[0], checked="2026-10-01", draw="2026-10-01-1")]
            + [row(c) for c in claims[1:]],
            {"2026-10-01-1": dict(recovered, drawn="2026-10-01")},
        ),
        ["marked recovered and was drawn after"],
    )
    case(
        "a recovered record citing nothing that exists is refused",
        tree(
            claims,
            [row(claims[0], checked="2026-09-30", draw="2026-09-30-1")]
            + [row(c) for c in claims[1:]],
            {"2026-09-30-1": dict(recovered, recovered="ledger/not-there.md")},
        ),
        ["which is not a file"],
    )

    # A filename that disagrees with its own contents, which is how a `draw`
    # field ends up pointing at a record nobody edits.
    case(
        "a record whose filename does not match its seed is refused",
        tree(
            claims,
            [row(claims[0], checked="2026-01-02", draw="2026-01-02-8")]
            + [row(c) for c in claims[1:]],
            {"2026-01-02-8": record},
        ),
        ["so it should be 2026-01-02-7.json"],
    )

    # --- the never-fires halves ------------------------------------------
    case(
        "an empty draws directory is reported, not read as clean",
        tree(claims, [row(c) for c in claims], {}),
        ["no draw record in", "no deliberate verdict carries `checked`"],
    )
    case(
        "a bucket nobody has stamped is reported, not read as clean",
        tree(claims, [row(c) for c in claims], {"2026-01-02-7": record}),
        ["no deliberate verdict carries `checked`"],
    )

    # And the guard over the real tree, which must be clean.
    case("slate-orm itself is clean", guard.ROOT, [])

    failed = RESULTS.count(False)
    print(f"\n{len(RESULTS) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
