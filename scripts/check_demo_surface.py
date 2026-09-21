#!/usr/bin/env python3
"""Every adapter endpoint the demo's UI does not call is one somebody decided about.

The demo's web UI calls 12 of the adapters' 24 endpoints. That is the design —
`ledger/2026-09-21-the-demo-ui-is-a-subset-on-purpose.md` argues it at length:
the UI exists to show the SDK switcher and the identity switcher, and surface
coverage is the conformance runner's job. The problem that entry named and did
not fix is that **nothing made the ratio visible**, so it drifted through six
features and was found by counting on purpose.

It also recorded why it did not write this check:

> A guard is writable (both lists are greppable, as above) and was not
> written, because a guard that fails whenever the adapters gain an endpoint
> would fail on every surface task and be switched off.

That objection is right about a guard that only compares two lists, and it
names its own answer. This is the `EXPECTED_REFUSALS` idiom the repository
already uses in five places: the failure is not "you added an endpoint", it is
"say in one line why the UI does not show it". Adding an endpoint and a line
here is a decision recorded; adding an endpoint and nothing is a decision
nobody made.

The entry counted **11**. It is 12: `/api/restore-unchanged` was added to the
UI afterwards and the sentence in `docs/full-text.md` still said eleven. That
is the drift, one feature later, and it is what this exists to stop.

Run directly: `python3 scripts/check_demo_surface.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: The Python adapter's route table, which is the definition of the surface.
#:
#: One adapter rather than three, because the conformance runner already holds
#: the three to each other: a route in one and not the others fails there, with
#: a better message than this could give. Here it is the *list* that matters.
ADAPTER = ROOT / "examples/explorer/backends/python/adapter/__main__.py"

#: Where the UI lives. Every `/api/...` spelled anywhere under it counts as
#: called, which is generous on purpose — a path in a comment or a dead branch
#: reads as coverage this check will not argue with, and the failure mode of
#: being generous is that it stays quiet, not that it cries wolf.
UI = ROOT / "examples/explorer/web/src"

ROUTE = re.compile(r'^\s*"(/api/[a-z-]+)":', re.MULTILINE)
CALLED = re.compile(r"/api/[a-z-]+")

#: Endpoints the adapters serve and the UI deliberately does not call.
#:
#: The reason is the entry, not a formality: six of these were added by a
#: surface task that stopped at the adapters, and the answer to "should this
#: get a panel?" has been no every time. A seventh should be no for a reason
#: somebody writes down rather than by default.
NOT_IN_THE_UI = {
    "/api/window": (
        "a window function's value is a column like any other, and a panel "
        "showing one would demonstrate the UI rather than the feature"
    ),
    "/api/chain": "an n-way chain renders as a wide table; the plan panel shows the shape",
    "/api/nearest": (
        "a nearest-neighbour search over eight embeddings is a list in an "
        "order nobody can check by eye"
    ),
    "/api/related": "one relationship loaded for many parents; the join panel already shows two tables",
    "/api/page": "keyset pagination needs a cursor to be visible to mean anything",
    "/api/search": (
        "the full-text demo: deliberately no search box. Adding one and not a "
        "window panel would set the precedent that the newest feature gets a "
        "panel, which is how a demo becomes a menu"
    ),
    "/api/conditional-update": "optimistic concurrency needs a second writer to be worth watching",
    "/api/conditional-delete": "the same, for a delete",
    "/api/purge": "erasing retired rows answers with a count and shows nothing",
    "/api/bad-status": "a deliberate check violation, for the conformance corpus's error shapes",
    "/api/typed": "reads two rows through the *generated* decoders; the UI decodes its own",
    "/api/bad-batch": "two rows a batch refuses for different reasons, as data rather than a trailer",
}


def routes(adapter: Path = ADAPTER) -> set[str]:
    return set(ROUTE.findall(adapter.read_text(encoding="utf-8")))


def called(ui: Path = UI) -> set[str]:
    found: set[str] = set()
    for path in sorted(ui.rglob("*")):
        if path.suffix in {".ts", ".tsx"} and path.is_file():
            found |= set(CALLED.findall(path.read_text(encoding="utf-8")))
    return found


def main(argv: list[str] | None = None) -> int:
    # The two subjects are arguments so the tests beside this file can run the
    # real checks over files they wrote. Run only against the tree, a check
    # that had stopped checking would pass for as long as the tree stayed
    # correct, which is the failure mode it exists to prevent one level up.
    argv = sys.argv[1:] if argv is None else argv
    adapter = Path(argv[0]) if argv else ADAPTER
    ui = Path(argv[1]) if len(argv) > 1 else UI

    served = routes(adapter)
    reached = called(ui) & served
    problems: list[str] = []

    # The never-fires halves, and both are spellings. `ROUTES` is a dict
    # literal in one file and `/api/...` is a string the UI happens to write
    # out; either could move and leave this reporting `ok` over nothing, which
    # is the failure `CLAUDE.md` names and every guard here carries a pair for.
    if not served:
        problems.append(
            f"no `/api/...` routes in {adapter}, so this "
            "checked nothing. The route table moved or is no longer a dict of "
            "string keys — a person, not a pass."
        )
    if not reached:
        problems.append(
            f"the UI under {ui} names no adapter endpoint at "
            "all, so this checked nothing. Either the UI stopped calling them "
            "directly or the sources moved."
        )

    for endpoint in sorted(served - reached - set(NOT_IN_THE_UI)):
        problems.append(
            f"`{endpoint}` is served by the adapters and called nowhere in the "
            "UI.\n"
            "  That may well be right — the UI shows 12 of 24 on purpose — but "
            "it is a decision. Give it a panel, or add it to NOT_IN_THE_UI "
            "with one line saying what a panel for it would fail to show."
        )

    for endpoint, reason in sorted(NOT_IN_THE_UI.items()):
        if endpoint not in served:
            problems.append(
                f"NOT_IN_THE_UI names `{endpoint}`, which the adapters no "
                f"longer serve. Delete it; its reason was: {reason}"
            )
        elif endpoint in reached:
            problems.append(
                f"NOT_IN_THE_UI says the UI does not call `{endpoint}`, and it "
                "does. Delete the entry — the decision it records has been "
                "made the other way."
            )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    print(
        f"ok    {len(reached)} of {len(served)} adapter endpoints in the UI, "
        f"{len(NOT_IN_THE_UI)} left out on purpose"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
