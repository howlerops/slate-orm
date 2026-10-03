#!/usr/bin/env python3
"""The state a new session needs, derived from the tree rather than written down.

    python3 scripts/handoff.py            # the briefing
    python3 scripts/handoff.py --json     # the same, for a machine

# Why this is a script and not a document

A handoff document is wrong within a day. This repository has had the lesson
several times and in both directions: `CLAUDE.md` said *seventeen* CI jobs
while there were twenty-three, `docs/releasing.md` said nothing had been
published two weeks after a release, and `scripts/check_retired_claims.py`
exists because a correction reached one copy of a claim and not another.

So this derives everything it can and **points at** the rest. Every number
below is computed when you run it:

- the git position — branch, head, how far from `main`, what is uncommitted;
- the caveat frame, from `scripts/caveats.py`'s own `report()`, so this and
  `caveats.py` cannot disagree about what is counted;
- what is still `narrowed`, read out of `check_live_frame.STILL_NARROWED`,
  which is the roster a person is forced to edit;
- what has shipped — the newest tag, the declared version, and whether there
  is unreleased work between them;
- the newest ledger entries, with their kind;
- **and the live edges**: every caveat from the last few days' entries, which
  is where this repository writes down what it deliberately did not do.

That last one is the part that would otherwise be a hand-maintained TODO.
It is not one: `## What this does not do` is a section every entry must have,
`docs/caveat-status.json` carries a verdict and an argument for each, and
`scripts/check_live_frame.py` refuses to let one sit undecided. The backlog
is already written; this just reads the recent end of it.

# What it deliberately does not do

**It does not reach the network.** No CI conclusion, no registry, no GitHub.
Those need a token and a connection, and a briefing that cannot be produced
offline is one that fails exactly when a container has no network. The
commands to ask are printed instead.

**It does not say what to do next.** It says where the work stopped and what
was decided about each stopping point. Choosing among those is a person's
job, and a script that ranked them would be inventing a priority nobody set.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import caveats as caveat_tool
import check_live_frame

ROOT = Path(__file__).resolve().parent.parent

#: The trunk. Everything merges here and `pages.yml` deploys from it.
TRUNK = "main"

#: How many of the newest entries to brief on.
#:
#: A **count**, after a window of seven days was tried and produced 197
#: entries, 754 caveats and 138 KB of output. That number is the finding: this
#: ledger takes roughly thirty entries a day when anybody is working in it, so
#: any fixed span is either empty or a book. A count is bounded by
#: construction, and the span it covers is printed so the reader can see how
#: fast the thing is moving.
RECENT_ENTRIES = 12

#: How much of a caveat to show. Claims run to 509 characters here; the first
#: sentence is what tells you whether to open the entry.
CLAIM_CHARS = 140

#: Where a reader goes next, by what they are trying to do. Printed rather
#: than summarised, because each of these is long and already written.
POINTERS = {
    "the standards this code is held to": "CLAUDE.md",
    "how to write a ledger entry": "ledger/README.md",
    "what this does not have, and why": "docs/orm-comparison.md",
    "how a release is cut, and the switches that are off": "docs/releasing.md",
    "what every check does": "sh scripts/check.sh --list",
}


def git(*args: str) -> str:
    """A git command's stdout, or `""` if git refuses.

    Empty rather than raising: a briefing that dies because it is run outside
    a work tree is worse than one that says it could not tell.
    """
    result = subprocess.run(
        ["git", *args], cwd=ROOT, capture_output=True, text=True, check=False
    )
    return result.stdout.strip() if result.returncode == 0 else ""


def position() -> dict[str, object]:
    """Where the tree is, relative to the trunk and to the last release."""
    branch = git("rev-parse", "--abbrev-ref", "HEAD")
    head = git("rev-parse", "--short", "HEAD")
    dirty = [line for line in git("status", "--porcelain").splitlines() if line]

    ahead = behind = None
    counts = git("rev-list", "--left-right", "--count", f"{TRUNK}...HEAD")
    if counts:
        parts = counts.split()
        if len(parts) == 2:
            behind, ahead = int(parts[0]), int(parts[1])

    # `--tags` so a lightweight tag counts; `--abbrev=0` for the name alone.
    tag = git("describe", "--tags", "--abbrev=0")
    since_tag = git("rev-list", "--count", f"{tag}..HEAD") if tag else ""

    return {
        "branch": branch,
        "head": head,
        "uncommitted": len(dirty),
        "ahead_of_trunk": ahead,
        "behind_trunk": behind,
        "last_tag": tag,
        "commits_since_tag": int(since_tag) if since_tag.isdigit() else None,
    }


def declared_version() -> str:
    """The one number, from the root manifest.

    Read with a regex rather than `tomllib` for the reason
    `scripts/check_versions.py` gives: the line a reader checking by eye would
    look for is the thing worth reading.
    """
    manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    found = re.search(r"^version = \"([^\"]+)\"", manifest, re.M)
    return found.group(1) if found else "<unreadable>"


def frame() -> dict[str, int]:
    counts, _, _ = caveat_tool.report(ROOT)
    return counts


def narrowed() -> list[tuple[str, str]]:
    """Every still-narrowed caveat and what closes it, from the roster."""
    return [(f"{entry}: {claim}", why) for (entry, claim), why in
            sorted(check_live_frame.STILL_NARROWED.items())]


def recent_entries(count: int) -> list[dict[str, str]]:
    """The newest `count` ledger entries, by the date in their filename.

    The filename and not the git log: an entry written today for work done
    last week is dated for the work, and the sort this repository reads by is
    the one in `ledger/`.
    """
    dated = re.compile(r"^(\d{4}-\d{2}-\d{2})-")
    entries = []
    for path in sorted((ROOT / "ledger").glob("*.md"), reverse=True):
        stamp = dated.match(path.name)
        if not stamp:
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        title = text.splitlines()[0].lstrip("# ").strip() if text else path.stem
        kind = re.search(r"^- \*\*Kind:\*\* (.+)$", text, re.M)
        entries.append({
            "date": stamp.group(1),
            "file": path.name,
            "title": title,
            "kind": kind.group(1).strip() if kind else "?",
        })
    return entries[:count]


def shorten(claim: str, limit: int = CLAIM_CHARS) -> str:
    """A claim's first sentence, or its first `limit` characters.

    Cut at a sentence boundary when there is one inside the limit, because a
    caveat's first sentence is almost always the claim and the rest is its
    reasoning — truncating mid-word loses the former to keep half of the
    latter.
    """
    claim = " ".join(claim.split())
    if len(claim) <= limit:
        return claim
    stop = claim.find(". ")
    if 0 < stop < limit:
        return claim[: stop + 1]
    return claim[: limit - 1].rstrip() + "\u2026"


def live_edges(entries: list[dict[str, str]]) -> list[dict[str, str]]:
    """Each recent entry's caveats, with the verdict recorded against them.

    This is the closest thing to a worklist that exists here, and it is
    derived: `## What this does not do` is mandatory, and every bullet in it
    carries a verdict in `docs/caveat-status.json`.
    """
    recent = {e["file"] for e in entries}
    status = caveat_tool.load(ROOT)
    out = []
    for c in caveat_tool.caveats(ROOT):
        if c["entry"] not in recent:
            continue
        verdict = status.get(f"{c['entry']}::{caveat_tool.key(c['claim'])}", {})
        out.append({
            "entry": c["entry"],
            "claim": shorten(c["claim"]),
            "verdict": verdict.get("verdict", "untriaged"),
        })
    return out


def collect(count: int = RECENT_ENTRIES) -> dict[str, object]:
    entries = recent_entries(count)
    return {
        "spans": f"{entries[-1]['date']} to {entries[0]['date']}" if entries else "",
        "position": position(),
        "declared_version": declared_version(),
        "frame": frame(),
        "narrowed": [{"caveat": c, "closes_on": w} for c, w in narrowed()],
        "recent_entries": entries,
        "live_edges": live_edges(entries),
        "pointers": POINTERS,
    }


def render(state: dict) -> str:
    p = state["position"]
    lines = ["# slate-orm, where it stands", ""]

    lines.append("## Position")
    lines.append("")
    lines.append(f"- branch `{p['branch']}` at `{p['head']}`")
    if p["ahead_of_trunk"] is not None:
        lines.append(
            f"- {p['ahead_of_trunk']} ahead of `{TRUNK}`, {p['behind_trunk']} behind"
        )
    lines.append(
        f"- {p['uncommitted']} uncommitted path(s)"
        + ("" if p["uncommitted"] else " — tree is clean")
    )
    if p["last_tag"]:
        unreleased = p["commits_since_tag"]
        lines.append(
            f"- last tag `{p['last_tag']}`, tree declares `{state['declared_version']}`"
            + (f", {unreleased} commit(s) since" if unreleased else ", nothing since")
        )
    lines.append("")

    counts = state["frame"]
    live = counts.get("open", 0) + counts.get("narrowed", 0)
    lines.append("## The caveat frame")
    lines.append("")
    lines.append(
        "  ".join(f"{name} {counts[name]}" for name in sorted(counts))
        + f"   (live: {live})"
    )
    lines.append("")
    if state["narrowed"]:
        lines.append("Still narrowed, and what closes each:")
        lines.append("")
        for row in state["narrowed"]:
            lines.append(f"- **{row['caveat']}**")
            lines.append(f"  {row['closes_on']}")
        lines.append("")

    lines.append(f"## The newest {len(state['recent_entries'])} entries"
                 + (f" ({state['spans']})" if state["spans"] else ""))
    lines.append("")
    for e in state["recent_entries"]:
        lines.append(f"- `{e['date']}` ({e['kind']}) {e['title']}")
        lines.append(f"  `ledger/{e['file']}`")
    if not state["recent_entries"]:
        lines.append("- nothing dated recently")
    lines.append("")

    edges = state["live_edges"]
    lines.append("## Where that work stopped")
    lines.append("")
    lines.append(
        "Every bullet below is a `## What this does not do` caveat from an entry "
        "above, with the verdict recorded against it. A `deliberate` one is a "
        "boundary somebody argued for; an `untriaged` one is work this script "
        "found before the ledger did."
    )
    lines.append("")
    for edge in edges:
        lines.append(f"- [{edge['verdict']}] {edge['claim']}")
        lines.append(f"  `ledger/{edge['entry']}`")
    if not edges:
        lines.append("- none recorded")
    lines.append("")

    lines.append("## Before you push")
    lines.append("")
    lines.append("```sh")
    lines.append("sh scripts/check.sh                 # every static check CI runs")
    lines.append("python3 scripts/reclaim.py          # disk is tight; run it first")
    lines.append("```")
    lines.append("")
    lines.append(
        "Every change outside `ledger/` needs an entry in the same commit; the "
        "pre-commit hook refuses otherwise."
    )
    lines.append("")

    lines.append("## Not derivable here")
    lines.append("")
    lines.append(
        "This script never reaches the network, so it cannot tell you how CI "
        "ended or what a registry holds. Ask:"
    )
    lines.append("")
    lines.append("```sh")
    lines.append(
        "gh api 'repos/howlerops/slate-orm/actions/workflows/ci.yml/runs"
        f"?branch={p['branch']}&per_page=1' \\"
    )
    lines.append(
        "  --jq '.workflow_runs[0] | \"\\(.run_number) \\(.head_sha[0:7]): "
        "\\(.status) \\(.conclusion)\"'"
    )
    lines.append("```")
    lines.append("")

    lines.append("## Where the rest is written")
    lines.append("")
    for what, where in state["pointers"].items():
        lines.append(f"- {what} — `{where}`")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--json", action="store_true", help="emit the state as JSON")
    parser.add_argument(
        "--entries",
        type=int,
        default=RECENT_ENTRIES,
        help=f"how many of the newest ledger entries to brief on (default {RECENT_ENTRIES})",
    )
    args = parser.parse_args(argv)

    state = collect(args.entries)
    if args.json:
        print(json.dumps(state, indent=2, sort_keys=True))
    else:
        print(render(state), end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
