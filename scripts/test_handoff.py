#!/usr/bin/env python3
"""`handoff.py`'s own tests, over a tree and a git history this file builds.

Run against the real repository every case would be "it printed something",
which is what a briefing that had stopped reading the tree also does. So the
derivations that could silently lie are exercised over a miniature: a real
`git init` with real commits and a real tag, a two-entry ledger, and a
tracker with verdicts in it.

The case worth reading is `a claim is cut at its first sentence`. The first
version of this script had no limit and printed **138 KB** — 197 entries and
754 caveats, because a seven-day window over a ledger that takes thirty
entries a day is not a window, it is the whole book.

Run directly: `python3 scripts/test_handoff.py`.
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import handoff

ENTRY = """\
# {title}

- **Date:** {date}
- **Author:** a test
- **Touches:** nothing
- **Kind:** {kind}

## What changed

Something.

## Why

Because.

## Alternatives rejected

None worth naming.

## Evidence

n/a

## What this does not do

**{caveat}** {tail}
"""


def build(root: pathlib.Path, entries: list[tuple[str, str, str, str, str]]) -> None:
    """A miniature repository: a git history, a ledger, and a tracker."""
    (root / "ledger").mkdir()
    (root / "docs").mkdir()
    (root / "Cargo.toml").write_text('[workspace.package]\nversion = "9.9.9"\n')

    verdicts = []
    for date, slug, title, kind, caveat in entries:
        (root / "ledger" / f"{date}-{slug}.md").write_text(
            ENTRY.format(
                title=title,
                date=date,
                kind=kind,
                caveat=caveat,
                tail="The rest of the reasoning, which the briefing should not print.",
            )
        )
        verdicts.append({
            "entry": f"{date}-{slug}.md",
            "key": caveat[:60],
            "verdict": "deliberate",
            "reviewed": date,
            "by": "a reason",
        })
    (root / "docs" / "caveat-status.json").write_text(json.dumps({"verdicts": verdicts}))

    def run(*args: str) -> None:
        subprocess.run(
            ["git", *args], cwd=root, check=True, capture_output=True, text=True
        )

    run("init", "-q", "-b", "main")
    run("config", "user.email", "test@example.invalid")
    run("config", "user.name", "a test")
    run("add", "-A")
    run("commit", "-qm", "first")
    run("tag", "v0.0.9")
    (root / "ledger" / "unreleased.txt").write_text("after the tag\n")
    run("add", "-A")
    run("commit", "-qm", "second")


def collected(root: pathlib.Path, **kwargs) -> dict:
    was = handoff.ROOT
    was_tool, was_frame = handoff.caveat_tool.ROOT, handoff.check_live_frame.STATUS
    handoff.ROOT = root
    handoff.caveat_tool.ROOT = root
    try:
        return handoff.collect(**kwargs)
    finally:
        handoff.ROOT = was
        handoff.caveat_tool.ROOT = was_tool
        handoff.check_live_frame.STATUS = was_frame


TWO = [
    ("2026-02-02", "newer", "The newer thing", "fix", "A newer edge."),
    ("2026-01-01", "older", "The older thing", "process", "An older edge."),
]


def main() -> int:
    failures: list[str] = []
    ran = 0

    def case(name: str, got: object, want: object) -> None:
        # Counted rather than totalled at the bottom: the first version wrote
        # `total = 16` beside seventeen cases, which is the asserted-not-
        # derived hazard this repository keeps meeting, in its own test file.
        nonlocal ran
        ran += 1
        if got != want:
            failures.append(f"FAIL  {name}: {got!r}, wanted {want!r}")
        else:
            print(f"ok    {name}")

    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        build(root, TWO)
        state = collected(root)

        case("the declared version is read", state["declared_version"], "9.9.9")
        case(
            "entries come back newest first",
            [e["file"] for e in state["recent_entries"]],
            ["2026-02-02-newer.md", "2026-01-01-older.md"],
        )
        case("an entry's kind is read", state["recent_entries"][0]["kind"], "fix")
        case(
            "the span is the oldest and newest shown",
            state["spans"],
            "2026-01-01 to 2026-02-02",
        )
        case(
            "each entry's caveat is found, with its verdict",
            sorted((e["claim"], e["verdict"]) for e in state["live_edges"]),
            [("A newer edge.", "deliberate"), ("An older edge.", "deliberate")],
        )
        case("the frame counts the tracker", state["frame"]["deliberate"], 2)
        case("commits since the tag are counted", state["position"]["commits_since_tag"], 1)
        case("the tag is read", state["position"]["last_tag"], "v0.0.9")

        # The bound. Asking for one entry must show one, or the briefing grows
        # with the ledger — which is the defect this file's docstring records.
        trimmed = collected(root, count=1)
        case("the count bounds what is shown", len(trimmed["recent_entries"]), 1)
        case(
            "and bounds the edges with it",
            [e["entry"] for e in trimmed["live_edges"]],
            ["2026-02-02-newer.md"],
        )

        # Rendering must not raise on a real state, and must not leak the
        # reasoning the caveat's first sentence is cut from.
        text = handoff.render(state)
        case("the briefing names an entry", "2026-02-02-newer.md" in text, True)
        case(
            "the reasoning after the first sentence is not printed",
            "which the briefing should not print" in text,
            False,
        )

    # `shorten` on its own, because the fixture's caveats are short by design.
    long = "The first sentence stops here. " + "And then a great deal more. " * 20
    case("a claim is cut at its first sentence", handoff.shorten(long),
         "The first sentence stops here.")
    case(
        "a claim with no sentence break is cut to the limit",
        len(handoff.shorten("x" * 500)),
        handoff.CLAIM_CHARS,
    )
    case("a short claim is left alone", handoff.shorten("Short."), "Short.")

    # The never-fires case: a tree with no ledger at all must not look like a
    # tidy one. An empty briefing and a complete one differ by one word here.
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        (root / "ledger").mkdir()
        (root / "docs").mkdir()
        (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.0.0"\n')
        (root / "docs" / "caveat-status.json").write_text('{"verdicts": []}')
        empty = collected(root)
        text = handoff.render(empty)
        case("an empty ledger says so", "nothing dated recently" in text, True)
        case("and claims no edges", "none recorded" in text, True)

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{ran - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
