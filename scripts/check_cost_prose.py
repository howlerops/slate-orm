#!/usr/bin/env python3
"""No comment states a cost constant's value that the constant does not have.

`crates/slate-kernel/src/stats.rs` holds three numbers the planner decides on,
and the reasoning that justifies every plan choice in this crate is written
in prose *around* them — in module docs, in test comments, in the fixture that
picks a row count. That prose quotes the numbers, and derives further numbers
from them.

**It went stale twice.** #269 re-measured `POINT_READ_COST` from 3.0 to 1.0
after four measurements contradicted the recorded figure. `stats.rs` itself was
corrected in #275, two tasks later. Eight more files still said *"a point read
costs about three"* when #277 looked, and five of them went on to derive a
crossover — *"an index only pays off when it saves scanning some twenty-four
thousand rows"* — that had moved by the same factor of three. A reader deciding
whether to add an index would have got it three times wrong, from a comment
that reads as measured fact.

That is `CLAUDE.md`'s "stale documentation is worse than none" pointed at the
most load-bearing prose in the repository, and it is mechanically checkable:
the numbers are derivable from the constants.

WHAT IS CHECKED, AND WHY IT IS NARROW

Two derived figures, both as they are actually written:

* **what a point read costs**, against `POINT_READ_COST`;
* **the crossover** — how many rows a scan must save per row fetched before an
  index pays — against `POINT_READ_COST / SCAN_ROW_COST`.

Numbers are matched as digits *and* as the English words this repository
writes them in, because it writes them both ways in the same paragraph. Only
`crates/slate-kernel/` is read: that is where the constants are decided and
where the reasoning lives. `docs/performance.md` quotes measurements rather
than constants, and `site/check/docs.py` is its own guard.

A passage that is deliberately historical — a withdrawn figure kept legible
with a strikethrough, which `ledger/README.md` asks for — is not a claim about
today, so a line containing `~~` is skipped. That exemption is the reason the
corrected passages could keep their history instead of losing it.

Run directly: `python3 scripts/check_cost_prose.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STATS = ROOT / "crates/slate-kernel/src/stats.rs"
WHERE = ROOT / "crates/slate-kernel"

#: `pub const POINT_READ_COST: f64 = 1.0;`
CONSTANT = re.compile(
    r"^pub const (POINT_READ_COST|SCAN_ROW_COST): f64 = ([0-9._e-]+);", re.MULTILINE
)

#: How this repository spells small numbers in prose. Both directions matter:
#: `1` and `one` appear in the same paragraph.
WORDS = {
    "one": 1.0, "two": 2.0, "three": 3.0, "four": 4.0, "five": 5.0,
    "six": 6.0, "seven": 7.0, "eight": 8.0, "nine": 9.0, "ten": 10.0,
    "twelve": 12.0, "twenty": 20.0,
}
#: Thousands, as written: `eight thousand`, `twenty-four thousand`.
THOUSANDS = {
    "eight": 8_000.0, "twelve": 12_000.0, "sixteen": 16_000.0,
    "twenty-four": 24_000.0, "forty-eight": 48_000.0,
}

#: "a point read costs about three" / "a point read costs 1"
PER_READ = re.compile(
    r"point reads? costs? (?:about |roughly |~)?([0-9.]+|[a-z]+)\b", re.IGNORECASE
)
#: "saves scanning some twenty-four thousand rows" / "n > 8000k"
CROSSOVER = re.compile(
    r"(?:saves? scanning (?:about |some |something like )?"
    r"([0-9,]+|[a-z-]+) thousand"
    r"|`n > ([0-9]+)k`)",
    re.IGNORECASE,
)


def constants(stats: Path = STATS) -> dict[str, float]:
    """The two constants, read from the source rather than restated here."""
    return {name: float(value.replace("_", "")) for name, value in CONSTANT.findall(
        stats.read_text(encoding="utf-8")
    )}


def number(text: str, thousands: bool) -> float | None:
    """A figure as digits or as this repository writes it in words."""
    plain = text.replace(",", "").strip().lower()
    table = THOUSANDS if thousands else WORDS
    if plain in table:
        return table[plain]
    try:
        value = float(plain)
    except ValueError:
        return None
    return value * 1000 if thousands and value < 1000 else value


def named(path: Path) -> str:
    """A path as a reader of the repository would name it.

    Not cosmetic: a fixture's path is under `/tmp` and `relative_to` raises on
    it, so with this inlined the *failure* path crashed under test while the
    passing path was fine — a guard whose report cannot be exercised. Caught by
    the first run of the tests beside this file, which is the second time this
    week; `check_demo_surface.py` carries the same helper for the same reason.
    """
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def sources(where: Path = WHERE) -> list[Path]:
    """Every Rust file under the kernel, in a stable order."""
    return [path for path in sorted(where.rglob("*.rs")) if path.is_file()]


def check(where: Path = WHERE, stats: Path = STATS) -> tuple[int, list[str]]:
    """Returns how many claims were read, and which disagree."""
    values = constants(stats)
    if len(values) != 2:
        return 0, [
            f"{stats.name} no longer declares both POINT_READ_COST and "
            "SCAN_ROW_COST as `pub const ...: f64 = N;`, so this checked "
            "nothing. Move the pattern with them."
        ]
    read = values["POINT_READ_COST"]
    crossover = values["POINT_READ_COST"] / values["SCAN_ROW_COST"]

    seen = 0
    wrong = []
    for path in sources(where):
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for at, line in enumerate(text.splitlines(), start=1):
            # A withdrawn figure kept legible is history, not a claim.
            if "~~" in line:
                continue
            for match in PER_READ.finditer(line):
                value = number(match.group(1), thousands=False)
                if value is None:
                    continue
                seen += 1
                if abs(value - read) > 0.01:
                    wrong.append(
                        f"{named(path)}:{at} says a point read "
                        f"costs {match.group(1)}; POINT_READ_COST is {read:g}."
                    )
            for match in CROSSOVER.finditer(line):
                written = match.group(1) or match.group(2)
                value = number(written, thousands=match.group(1) is not None)
                if value is None:
                    continue
                seen += 1
                if abs(value - crossover) > 1.0:
                    wrong.append(
                        f"{named(path)}:{at} puts the crossover at "
                        f"{written}; POINT_READ_COST / SCAN_ROW_COST is "
                        f"{crossover:g}."
                    )
    return seen, wrong


def main(where: Path = WHERE, stats: Path = STATS) -> int:
    # Both are arguments so the never-fires guard below can be tested. It
    # could not be: the cases call `check` directly, `main` read the two
    # module constants, and a mutation deleting the guard changed no verdict
    # because this crate always has claims. That is the third guard this week
    # with the judgement in `main` and the tests one level under it.
    seen, wrong = check(where, stats)
    for problem in wrong:
        print(problem, file=sys.stderr)
        print(
            "  The constant moved and the sentence did not. Correct it, or "
            "strike the old figure through with `~~` if it is being kept as "
            "history.",
            file=sys.stderr,
        )
    # The never-fires guard. Both patterns are over English prose, and an
    # ordinary rewording would leave them matching nothing while this printed
    # `ok` over a crate it had read and understood none of.
    if seen == 0:
        print(
            "no sentence anywhere under crates/slate-kernel states what a "
            "point read costs or where the crossover sits, which means this "
            "is looking in the wrong place rather than that the reasoning is "
            "undocumented.",
            file=sys.stderr,
        )
        return 1
    if wrong:
        print(f"\n{len(wrong)} stale of {seen} claims", file=sys.stderr)
        return 1
    print(f"ok    {seen} prose claims about the cost constants, all current")
    return 0


if __name__ == "__main__":
    sys.exit(main())
