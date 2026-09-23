#!/usr/bin/env python3
"""`check_retired_claims.py`, over trees this file writes.

Against a written tree rather than against the repository, for the reason the
other guards here give: a check tested only by running it over `slate-orm` can
only assert that today's tree is clean, which is also what a check that does
nothing asserts.

The wrapped case is the one that matters. `the_claim_is_found_when_it_wraps`
writes the comment that prompted this guard, split across lines exactly as it
was written in `loader_cliff.rs`. A line-by-line implementation passes every
other case here and fails that one.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_retired_claims as guard

PHRASE = "the runner that does reach it cannot be built"

#: Every case's verdict, appended by `case` and by the never-fires blocks,
#: so the summary counts what ran rather than a number kept in step by hand.
RESULTS: list[bool] = []


def tree(root: Path, files: dict[str, str]) -> None:
    for name, body in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")


def registry(entries: list[dict[str, str]]) -> str:
    return json.dumps({"note": "fixture", "retired": entries}, indent=2)


def one(phrase: str = PHRASE) -> list[dict[str, str]]:
    return [
        {
            "phrase": phrase,
            "retired_by": "ledger/e.md",
            "why": "fixture",
            "instead": "fixture",
        }
    ]


def case(name: str, files: dict[str, str], problems: int) -> bool:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        tree(root, files)
        _, _, got = guard.check(root)
        if len(got) != problems:
            print(f"FAIL  {name}: wanted {problems} problems, got {len(got)}")
            for problem in got:
                print(f"        {problem}")
            RESULTS.append(False)
            return False
        print(f"ok    {name}")
        RESULTS.append(True)
        return True


def main() -> int:
    base = {
        "scripts/retired_claims.json": registry(one()),
        "ledger/e.md": "the entry that withdrew it\n",
    }
    ok = True

    ok &= case(
        "a clean tree passes",
        {**base, "crates/a/src/lib.rs": "/// The suite runs in release.\n"},
        0,
    )

    ok &= case(
        "the claim is found on one line",
        {**base, "crates/a/src/lib.rs": f"/// Note: {PHRASE} here.\n"},
        1,
    )

    # The case the guard exists for. Written as it actually appeared.
    ok &= case(
        "the claim is found when it wraps",
        {
            **base,
            "crates/a/examples/loader_cliff.rs": (
                "/// reach — and on this container the runner that *does* "
                "reach it cannot be\n"
                "/// built, because nine debug example binaries exhaust the "
                "disk.\n"
            ),
        },
        1,
    )

    ok &= case(
        "a ledger entry keeps the old wording",
        {**base, "ledger/2026-01-01-old.md": f"It said {PHRASE}, wrongly.\n"},
        0,
    )

    ok &= case(
        "NOT_A_CLAIM exempts a quotation",
        {
            **base,
            "docs/x.md": f"We used to say {PHRASE}.\n\nNOT_A_CLAIM — quoted.\n",
        },
        0,
    )

    ok &= case(
        "strikethrough exempts a withdrawal",
        {**base, "docs/x.md": f"~~{PHRASE}~~ — it runs in release.\n"},
        0,
    )

    ok &= case(
        "the registry itself is not scanned",
        {**base, "docs/x.md": "clean\n"},
        0,
    )

    ok &= case(
        "a phrase under three words is refused",
        {**base, "scripts/retired_claims.json": registry(one("eight binaries"))},
        1,
    )

    ok &= case(
        "a retirement naming no entry is refused",
        {
            **base,
            "scripts/retired_claims.json": registry(
                [{"phrase": PHRASE, "retired_by": "ledger/gone.md", "why": ""}]
            ),
        },
        1,
    )

    ok &= case(
        "a vendored tree is skipped",
        {**base, "clients/typescript/node_modules/p/i.ts": f"// {PHRASE}\n"},
        0,
    )

    # The never-fires guards, which are the reason the other checks here have
    # one: both of these tree shapes report zero problems and look clean.
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        tree(root, {"scripts/retired_claims.json": registry([]),
                    "docs/x.md": "prose the guard can scan\n"})
        if guard.main(root) == 0:
            print("FAIL  an empty registry passed, which it must not")
            ok = False
            RESULTS.append(False)
        else:
            print("ok    an empty registry is refused")
            RESULTS.append(True)

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        tree(root, base)
        if guard.main(root) == 0:
            print("FAIL  a tree with no scannable file passed, which it must not")
            ok = False
            RESULTS.append(False)
        else:
            print("ok    a tree with nothing to scan is refused")
            RESULTS.append(True)

    # `N passed, M failed`, which is what `mutate.py`'s `python` dialect
    # reads. Written as "all passed" first, which that dialect cannot see: it
    # reported "no test results at all" and scored every mutation unrunnable.
    # #261 found the same thing in two older runners; a new one repeats it
    # unless the format is copied deliberately.
    print(f"\n{sum(RESULTS)} passed, {len(RESULTS) - sum(RESULTS)} failed")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
