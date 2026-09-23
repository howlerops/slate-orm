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
from collections.abc import Callable
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

#: The demo's README, and the two files whose contents it counts.
#:
#: The endpoint roster above is one half of "does the demo's description match
#: the demo"; this is the other, and it is the half that goes stale *quietly*.
#: An endpoint nobody calls is at least visible in a diff; `sees 9 of 11 books`
#: is a sentence that stays grammatical when somebody adds a twelfth.
README = ROOT / "examples/explorer/README.md"
SEED = ROOT / "examples/explorer/backends/go/seed.go"
CONFIG = ROOT / "examples/explorer/head.toml"

#: One seeded book: `book(id, author, "title", year, ...)`.
BOOK = re.compile(r'^\s*book\(\d+,\s*\d+,\s*"[^"]*",\s*(\d{4}),', re.MULTILINE)
#: The README's claim about what the row policy hides.
#:
#: `\s+` rather than a space in both: the README is wrapped prose and both
#: sentences straddle a line break today. A pattern that assumed one line
#: matched nothing and reported the sentence missing, which is the never-fires
#: branch firing correctly on the check's own bug.
SEES = re.compile(r"sees\s+(\d+)\s+of\s+(\d+)\s+books")
#: The README's claim about which year it hides before.
BEFORE = re.compile(r"hides\s+everything\s+published\s+before\s+(\d{4})")
#: The policy itself.
POLICY = re.compile(r'using = "year >= (\d{4})"')

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


def named(path: Path) -> str:
    """A path as a reader of the repository would name it.

    A fixture's path is under `/tmp` and `relative_to(ROOT)` raises on it, so
    this is not cosmetic — the messages below name their file, and a guard
    whose failure path crashes in the tests is a guard whose failure path is
    untested.
    """
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def readme_counts(
    readme_at: Path = README,
    seed_at: Path = SEED,
    config_at: Path = CONFIG,
) -> list[str]:
    """The README's `sees N of M books` agrees with the seed and the policy.

    Three files have to say the same thing: `seed.go` decides how many books
    there are and what years they carry, `head.toml` decides which of them a
    reader may see, and the README tells a visitor the answer. Adding a book
    published in 1975 changes the answer and none of the other two files, so
    nothing but this notices.

    The numbers are recomputed rather than compared to a constant here: a
    roster of expected counts would be a third place to update.

    The three paths are arguments for the same reason `routes` and `called`
    take theirs, and the reason is a measured one rather than symmetry: with
    them fixed, `scripts/mutate.py` replaced this whole function's body with
    `[]` and **nothing failed**, because the only test was the real tree,
    which agrees. Two survivors from one run — that one, and computing the
    visible count as `year > cutoff`, which no seeded book is published
    exactly on. Both are caught by a fixture and neither by the tree.
    """
    years = [int(year) for year in BOOK.findall(seed_at.read_text(encoding="utf-8"))]
    policy = POLICY.search(config_at.read_text(encoding="utf-8"))
    readme = readme_at.read_text(encoding="utf-8")
    sees = SEES.search(readme)
    before = BEFORE.search(readme)

    problems = []
    # The never-fires halves. Every one of these is a pattern over somebody
    # else's file, and each would match nothing after an ordinary edit —
    # `book(` renamed, the policy rewritten as `year > 1959`, the sentence
    # rephrased — leaving this printing `ok` over three files it did not read.
    if not years:
        problems.append(
            f"no `book(id, author, \"title\", year, ...)` rows in "
            f"{named(seed_at)}, so the counts below checked nothing."
        )
    if policy is None:
        problems.append(
            f'no `using = "year >= NNNN"` in {named(config_at)}, so the '
            "reader's row policy is spelled some other way and this cannot "
            "read it."
        )
    if sees is None:
        problems.append(
            f"no `sees N of M books` in {named(readme_at)}. If the "
            "sentence moved, move this pattern with it; if the claim went "
            "away, delete this check deliberately."
        )
    if before is None:
        problems.append(
            f"no `hides everything published before NNNN` in "
            f"{named(readme_at)}, for the same reason."
        )
    if problems:
        return problems

    assert policy and sees and before  # narrowed by the guards above
    cutoff = int(policy.group(1))
    visible = sum(1 for year in years if year >= cutoff)
    if (int(sees.group(1)), int(sees.group(2))) != (visible, len(years)):
        problems.append(
            f"the README says a reader sees {sees.group(1)} of "
            f"{sees.group(2)} books; the seed has {len(years)} and the policy "
            f"`year >= {cutoff}` admits {visible}.\n"
            "  Whichever moved, the sentence a visitor reads is now wrong."
        )
    if int(before.group(1)) != cutoff:
        problems.append(
            f"the README says the policy hides everything before "
            f"{before.group(1)}; `head.toml` says `year >= {cutoff}`."
        )
    return problems


def main(
    argv: list[str] | None = None,
    prose_check: Callable[[], list[str]] = readme_counts,
) -> int:
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

    # Only against the real tree: `readme_counts` takes its own three paths,
    # so a fixture tests it directly rather than through here.
    #
    # It arrives as an argument because the alternative could not be tested.
    # A test that calls `readme_counts` itself cannot see whether *this*
    # still calls it, and the real three files agree — so deleting this call
    # changed no verdict, and `scripts/mutate.py` reported the survivor.
    # Rebinding the module attribute was the first fix and `ty` refuses it:
    # a module-level `def` is declared as that one function, and no other is
    # assignable to it.
    prose = not argv
    if prose:
        problems.extend(prose_check())

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

    # The README half is named only when it ran. Printing it unconditionally
    # would make a fixture run — which never reads the README — claim it, and
    # a summary that says more than it checked is the same lie as a skip that
    # reports green.
    print(
        f"ok    {len(reached)} of {len(served)} adapter endpoints in the UI, "
        f"{len(NOT_IN_THE_UI)} left out on purpose"
        + (
            "; the README's counts agree with the seed and the policy"
            if prose
            else ""
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
