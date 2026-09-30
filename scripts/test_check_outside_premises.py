#!/usr/bin/env python3
"""`check_outside_premises.py`, over trees this file writes.

Against written trees for the reason every guard here gives: run only against
`slate-orm`, each rule passes for as long as the tree stays correct, which is
also what a rule that does nothing does. The real tree is the last case.

Run directly: `python3 scripts/test_check_outside_premises.py`.
"""

from __future__ import annotations

import json
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_outside_premises as guard

#: A ledger entry with one caveat, parameterised by the caveat's text. The
#: five sections are what `caveats.py` needs to find a "What this does not do".
ENTRY = """# {title}

- **Date:** 2026-09-30
- **Author:** a test
- **Touches:** nothing
- **Kind:** process

## What changed

Nothing; this file exists to carry a caveat.

## Why

To be read by a guard.

## Alternatives rejected

None, being a fixture.

## Evidence

n/a

## What this does not do

**{claim}**
"""


def run(
    caveats: dict[str, str],
    verdicts: list[dict[str, object]],
    exempt: dict[tuple[str, str], str] | None = None,
) -> list[str]:
    """`{entry stem: claim}` and the verdict rows, as a tree the guard reads."""
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        (root / "ledger").mkdir()
        for stem, claim in caveats.items():
            (root / "ledger" / f"{stem}.md").write_text(
                ENTRY.format(title=stem, claim=claim), encoding="utf-8"
            )
        (root / "docs").mkdir()
        (root / "docs" / "caveat-status.json").write_text(
            json.dumps({"verdicts": verdicts}), encoding="utf-8"
        )
        return guard.problems(root, {} if exempt is None else exempt)


def verdict(stem: str, claim: str, state: str, **rest: object) -> dict[str, object]:
    import caveats as tracker

    return {"entry": f"{stem}.md", "key": tracker.key(claim), "verdict": state, **rest}


OUTSIDE = "Pages needs Settings -> Pages turned on by hand, which this cannot do."
INSIDE = "The planner does not cost a grouped chain as grouped."
URL = "The published site at https://example.invalid/ is not linked from the README."

#: name, the caveats, the verdicts, the exemptions, the text the report must
#: carry ("" means clean).
CASES: list[tuple[str, dict[str, str], list[dict[str, object]], dict, str]] = [
    (
        "a live outside-the-tree caveat with no recheck is reported",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "deliberate")],
        {},
        "with no `recheck`",
    ),
    (
        "one carrying a recheck is not",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "deliberate", recheck={"manual": "ask somebody with admin"})],
        {},
        "",
    ),
    (
        # The distinction the whole guard rests on: a claim about this tree is
        # falsified by ordinary work here, so it needs no outside recipe.
        "a caveat about this tree is not asked for one",
        {"a": OUTSIDE, "b": INSIDE},
        [
            verdict("a", OUTSIDE, "deliberate", recheck={"manual": "ask"}),
            verdict("b", INSIDE, "deliberate"),
        ],
        {},
        "",
    ),
    (
        # `closed` is answered and `moment` says it described a moment, so
        # neither is a standing assertion anybody should re-fetch.
        "a closed one is not asked for a recheck",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "closed")],
        {},
        "all 1 outside-the-tree caveats are closed",
    ),
    (
        "a moment is not asked for one either",
        {"a": OUTSIDE, "b": URL},
        [
            verdict("a", OUTSIDE, "moment"),
            verdict("b", URL, "deliberate", recheck={"url": "https://x.invalid/", "expect_status": 200}),
        ],
        {},
        "",
    ),
    (
        "a recheck naming a url and expecting nothing of it is reported",
        {"a": URL},
        [verdict("a", URL, "open", recheck={"url": "https://x.invalid/"})],
        {},
        "nothing to expect from it",
    ),
    (
        "a recheck with neither a url nor a manual is reported",
        {"a": URL},
        [verdict("a", URL, "open", recheck={"note": "somebody should look"})],
        {},
        "neither a `url`, a `tree` nor a `manual`",
    ),
    (
        "an empty manual is reported, because it says nothing",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "deliberate", recheck={"manual": "  "})],
        {},
        "empty `manual`",
    ),
    (
        # Beside a match that is *not* exempted, so this case is about the
        # exemption working rather than about the rule emptying out, which is
        # the case below.
        "an exempted caveat is not asked for a recheck",
        {"a": OUTSIDE, "b": URL},
        [
            verdict("a", OUTSIDE, "deliberate"),
            verdict("b", URL, "deliberate", recheck={"manual": "look"}),
        ],
        {("a.md", guard.caveats.key(OUTSIDE)): "the subject is docs/caveat-status.json, here"},
        "",
    ),
    (
        # The other half the entry that added `INSIDE_AFTER_ALL` recorded as
        # missing: the rot rule catches an exemption whose caveat moved, and
        # nothing caught one that was wrong when written. Every exemption makes
        # the same argument — the subject is a path here — so it has to name
        # one, and the path has to exist.
        "an exemption whose reason names no path here is reported",
        {"a": OUTSIDE, "b": URL},
        [
            verdict("a", OUTSIDE, "deliberate"),
            verdict("b", URL, "deliberate", recheck={"manual": "look"}),
        ],
        {("a.md", guard.caveats.key(OUTSIDE)): "it is really about this repository"},
        "names no path here that exists",
    ),
    (
        "an exemption naming a path that is not there is reported too",
        {"a": OUTSIDE, "b": URL},
        [
            verdict("a", OUTSIDE, "deliberate"),
            verdict("b", URL, "deliberate", recheck={"manual": "look"}),
        ],
        {("a.md", guard.caveats.key(OUTSIDE)): "the subject is scripts/not-there.py"},
        "names no path here that exists",
    ),
    (
        # The third kind of recipe. Two of the three `manual` recipes in this
        # repository named evidence that is *in* it, so counting a needle in a
        # file re-checks them on every run with no network.
        "a tree recipe whose count is right passes",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "deliberate", recheck={
            "tree": "docs/caveat-status.json", "expect_text": "verdicts", "count": 2})],
        {},
        "",
    ),
    (
        "a tree recipe whose count is wrong is reported",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "deliberate", recheck={
            "tree": "docs/caveat-status.json", "expect_text": "verdicts", "count": 7})],
        {},
        "carries it 2",
    ),
    (
        "a tree recipe naming a file that is not there is reported",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "deliberate", recheck={
            "tree": "scripts/absent.py", "expect_text": "x", "count": 1})],
        {},
        "which is not a file",
    ),
    (
        "a tree recipe with no expect_text is reported",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "deliberate", recheck={"tree": "docs/caveat-status.json"})],
        {},
        "needs a `tree` path and an `expect_text`",
    ),
    (
        # The rot in `ledger/2026-09-29-the-skip-list-that-excused-nothing.md`,
        # guarded here before it could happen: an exemption whose caveat was
        # reworded excuses nothing and reads like a rule doing its job.
        "an exemption no marker fires on any more is reported",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "deliberate", recheck={"manual": "ask"})],
        {("gone.md", "a claim nobody wrote"): "was exempted once"},
        "no marker fires on it any more",
    ),
    (
        # The third never-fires half, and the one an exemption list grows into:
        # each entry is an argument about one claim, and the whole set is the
        # rule switched off a line at a time.
        "a tree where every match is exempted is reported, and says so",
        {"a": OUTSIDE},
        [verdict("a", OUTSIDE, "deliberate")],
        {("a.md", guard.caveats.key(OUTSIDE)): "exempted; it is docs/caveat-status.json"},
        "applies to nothing",
    ),
    (
        # The never-fires half. With no outside-the-tree caveat anywhere the
        # loop reports nothing, which is indistinguishable from clean.
        "a tree whose caveats are all about itself is reported, not passed over",
        {"b": INSIDE},
        [verdict("b", INSIDE, "deliberate")],
        {},
        "no caveat anywhere names a URL",
    ),
]


def main() -> int:
    failed = 0
    ran = 0
    asked: list[tuple[str, list[str]]] = []
    for name, entries, verdicts, exempt, wanted in CASES:
        try:
            found = run(entries, verdicts, exempt)
        except Exception as raised:  # noqa: BLE001 - a crash is this case failing
            found = [f"raised {raised!r} instead of reporting a problem"]
        asked.append((wanted, found))
        ok = (not found) if not wanted else any(wanted in one for one in found)
        failed += not ok
        ran += 1
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected {wanted!r}, got {found}")

    # Both never-fires halves, held to firing the way
    # `test_check_toolchain_pins.py` holds its own: the roster comes from a
    # tree where they must fire, and each must be some case's subject above.
    # A constant listing them would rot the way the thing it checks rots.
    halves = (
        run({"b": INSIDE}, [verdict("b", INSIDE, "deliberate")])
        + run({"a": OUTSIDE}, [verdict("a", OUTSIDE, "closed")])
        + run(
            {"a": OUTSIDE},
            [verdict("a", OUTSIDE, "deliberate")],
            {("a.md", guard.caveats.key(OUTSIDE)): "the only one; docs/caveat-status.json"},
        )
    )
    failed += not halves
    ran += 1
    print(f"{'ok  ' if halves else 'FAIL'}  the never-fires halves fire on a tree that has no rule to apply")
    for text in halves:
        ok = any(want and want in text and text in got for want, got in asked)
        failed += not ok
        ran += 1
        print(f"{'ok  ' if ok else 'FAIL'}  a case above is about {text.split(',')[0]!r}")

    # The real tree, last, so a failure here reads as "the tree drifted".
    found = guard.problems()
    ok = not found
    failed += not ok
    ran += 1
    print(f"{'ok  ' if ok else 'FAIL'}  every outside-the-tree caveat here says how to re-check it")
    for one in found:
        print(f"        {one}")

    # Counted, not summed. See `scripts/test_check_examples_roster.py`.
    print(f"\n{ran - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
