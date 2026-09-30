#!/usr/bin/env python3
"""A caveat whose truth lives outside this repository must say how to re-check it.

Every other guard here reads the tree, because almost every claim here is about
the tree. A handful are not: they assert something about GitHub's settings, a
package registry, an upstream project, a toolchain that ships on somebody
else's calendar. Nothing in a diff review touches those, no test covers them,
and `git grep` cannot answer them — so they go stale in a way that is invisible
by construction.

WHAT PROMPTED THIS, WITH THE TIMES

`ledger/2026-09-14-main-and-the-documentation-sweep.md` was committed as
`16a1db6` at **13:49:06** and says Pages "cannot be enabled from here", having
been refused with `Resource not accessible by integration`. The Pages workflow
run on that same commit failed seven seconds later, at 13:49:13. The next run,
**on the same SHA, with no code changed**, succeeded at **14:06:41**: somebody
with admin flipped the setting. The caveat was false seventeen minutes after it
was written and stood for sixteen days, through
`ledger/2026-09-28-the-first-sample-of-the-deliberate-verdicts.md` and the four
sampling passes after it. Its sibling — "`main` is not yet the default branch"
— went the same way.

Neither was a bad decision. Both were true when written. That is the whole
character of this class: it is not falsified by anybody in this repository
doing anything, so nobody here is in a position to notice.

WHAT THIS CHECKS, AND WHAT IT DELIBERATELY DOES NOT

Presence, not truth. A caveat matching `OUTSIDE` whose verdict is still live
must carry a `recheck` in `docs/caveat-status.json`: either a `url` and the
status or text it should answer, or a `manual` sentence saying what a person
must do. Running the recipes is `--run`, which needs the network and is
therefore not part of `scripts/check.sh`.

The obvious alternative was to have this *fetch* every URL and fail on a wrong
answer, with no `recheck` field at all. Rejected twice over. It would put the
network on the path of a check whose whole value is being runnable in a
container with none, and it would make the repository's own guard suite fail
when npm has a bad afternoon — which is how a check gets switched off, and this
one is about things nobody looks at. Separating "there is a recipe" from
"the recipe still answers right" keeps the first free.

The registry is the mechanism and its cost is the same one
`scripts/check_retired_claims.py` states about its own: **a premise nobody's
wording trips cannot be seen.** `OUTSIDE` is a list of surface markers, not an
understanding. It will miss a claim about the outside world phrased without one
of them. What it buys is the next one written in the obvious words.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import caveats  # noqa: E402  - after the path insert, as every guard here does

#: What makes a claim's subject live outside this repository. Named, because a
#: report that says which marker fired is a report a reader can argue with.
#:
#: These are deliberately surface markers over the *claim text*. A classifier
#: would be better and is not available: "is this sentence about something
#: outside the repo" is the undecidable problem this repository declines to
#: approximate elsewhere too, and a bad approximation here would either flood
#: the tracker with `recheck` demands or quietly exempt the next one.
OUTSIDE: dict[str, re.Pattern[str]] = {
    "a URL": re.compile(r"https?://", re.I),
    "GitHub the service": re.compile(
        r"\bgithub\.(?:com|io)\b|\bSettings\s*(?:→|->)|GitHub App|repository setting"
        r"|not accessible by integration|Actions permissions",
        re.I,
    ),
    "a package registry": re.compile(r"crates\.io|\bnpm\b|PyPI|pkg\.go\.dev", re.I),
    "an upstream project": re.compile(r"\bupstream\b|SlateDB'?s own", re.I),
    "a remote ref": re.compile(r"\bthe remote\b|git ls-remote", re.I),
    "a toolchain that ships elsewhere": re.compile(
        r"CI'?s (?:clippy|rustfmt|ruff)|newer than (?:yours|mine|ours)|release calendar", re.I
    ),
}

#: Caveats a marker fires on whose subject is this tree after all, each with
#: the argument for saying so. `npm` is the whole problem: "an npm package" and
#: "on first `npm install`" are about directories here, while "the package is
#: not published anywhere" is about a registry and says `npm` only in passing,
#: in a different clause. Tightening the marker to registry URLs would drop
#: that third one, which is a real member — so the marker stays loose and the
#: misses are argued here one at a time.
#:
#: Keyed the way `caveats.py` keys a verdict, so a reworded caveat orphans its
#: exemption instead of keeping one written for a different sentence.
INSIDE_AFTER_ALL: dict[tuple[str, str], str] = {
    ('2026-09-14-frontend-tests-and-configurable-ports.md',
     '`web/package.json` now depends on `playwright` for a demo, w'):
        'the subject is a devDependency in this repository and the download a reader '
        'pays for on first install. `npm` names the tool, not a registry this claim '
        'asserts anything about',
    ('2026-09-14-the-testserver-joins-the-workspace.md',
     "The demo's three backends (`examples/explorer/backends/`) ar"):
        '"an npm package" here is `examples/explorer/backends/`, a directory in this '
        'tree that no root-level command builds. Nothing about the registry',
}

#: Verdicts that make a caveat's claim a live one. `closed` is answered and
#: `moment` says in its own definition that it described a moment, so neither
#: is a standing assertion about the outside world.
LIVE = frozenset({"deliberate", "open", "narrowed", "untriaged"})

STATUS = Path("docs/caveat-status.json")


def matching(root: Path = ROOT) -> list[tuple[str, str, str, str]]:
    """`(entry, key, marker, claim)` for every caveat whose subject is outside."""
    out = []
    for one in caveats.caveats(root):
        for marker, rx in OUTSIDE.items():
            if rx.search(one["claim"]):
                out.append((one["entry"], caveats.key(one["claim"]), marker, one["claim"]))
                break
    return out


def stale_exemptions(
    rows: list[tuple[str, str, str, str]],
    exempt: dict[tuple[str, str], str] | None = None,
) -> list[str]:
    """Every `INSIDE_AFTER_ALL` entry no marker fires on any more.

    The rot `ledger/2026-09-29-the-skip-list-that-excused-nothing.md` recorded
    against a different guard's skip list, before it could happen here: an
    exemption whose caveat was reworded or removed excuses nothing and reads
    like a rule doing its job. Checked before anything else, because the set
    below was computed with these exclusions applied.
    """
    seen = {(entry, key) for entry, key, _, _ in rows}
    return [
        f"{entry}: `{key}` is exempted in `INSIDE_AFTER_ALL` and no marker fires on it "
        "any more. Either the caveat was reworded, or it is gone. Drop the entry: it "
        "is excusing nothing and it will excuse the wrong thing if the wording returns."
        for entry, key in (INSIDE_AFTER_ALL if exempt is None else exempt)
        if (entry, key) not in seen
    ]


def problems(
    root: Path = ROOT, exempt: dict[tuple[str, str], str] | None = None
) -> list[str]:
    """Every outside-the-tree caveat still live and carrying no way to re-check it."""
    exempt = INSIDE_AFTER_ALL if exempt is None else exempt
    rows = matching(root)
    said: list[str] = stale_exemptions(rows, exempt)
    if said:
        return said
    # The never-fires halves, and there are three of them because there are
    # three ways this rule can end up applying to nothing. They are checked
    # before and after the exemption filter rather than once after it: an
    # all-exempted tree and a tree the markers stopped matching are different
    # failures with different fixes, and one message for both would name the
    # wrong one. Found by `test_check_outside_premises.py`, which had a case
    # for the second and got the first one's message.
    if not rows:
        said.append(
            "no caveat anywhere names a URL, a registry, GitHub's settings or an "
            "upstream project, which was false when this was written: "
            "`ledger/2026-09-14-main-and-the-documentation-sweep.md` names two. "
            "Either the wording moved or `OUTSIDE` stopped matching it."
        )
        return said

    rows = [r for r in rows if (r[0], r[1]) not in exempt]
    if not rows:
        said.append(
            f"every caveat a marker fires on is in `INSIDE_AFTER_ALL` ({len(exempt)} "
            f"{'entry' if len(exempt) == 1 else 'entries'}), so this rule now applies "
            "to nothing. Each exemption is an "
            "argument that one claim is about this tree after all; all of them "
            "together is the rule being switched off one line at a time."
        )
        return said

    stored = json.loads((root / STATUS).read_text(encoding="utf-8"))
    verdict = {(r["entry"], r["key"]): r for r in stored["verdicts"]}

    live = [r for r in rows if verdict.get((r[0], r[1]), {}).get("verdict") in LIVE]
    # And the second half: if every one of them has been closed, this guard is
    # checking an empty set and will stay green through the next one added.
    if not live:
        said.append(
            f"all {len(rows)} outside-the-tree caveats are closed or moments, so this "
            "checked nothing. That is possible and was not true when this was written; "
            "confirm it rather than assuming the rule still bites."
        )
        return said

    for entry, key, marker, _ in live:
        row = verdict[(entry, key)]
        recipe = row.get("recheck")
        if not recipe:
            said.append(
                f"{entry}: `{key}` is about {marker} and is still {row['verdict']}, "
                f"with no `recheck`.\n"
                "  Nothing in this repository can falsify it, so nobody will. Add a "
                '`recheck` to its verdict: `{"url": ..., "expect_status": ...}`, '
                '`{"url": ..., "expect_text": ...}`, or `{"manual": "..."}` saying '
                "what a person has to do."
            )
            continue
        if "manual" in recipe:
            if not str(recipe["manual"]).strip():
                said.append(f"{entry}: `{key}` has an empty `manual` recheck, which says nothing.")
            continue
        if "url" not in recipe:
            said.append(
                f"{entry}: `{key}`'s `recheck` has neither a `url` nor a `manual`: {recipe}"
            )
        elif "expect_status" not in recipe and "expect_text" not in recipe:
            said.append(
                f"{entry}: `{key}`'s `recheck` names {recipe['url']} and nothing to "
                "expect from it. A fetch with no expectation passes whatever comes back."
            )
    return said


def run(root: Path = ROOT, timeout: float = 20.0) -> tuple[list[str], int, int]:
    """Execute every `url` recipe. Returns `(wrong, ran, unreachable)`.

    Unreachable is not wrong. A registry that times out says nothing about the
    claim, and failing on it would make this the check that cries wolf — which
    is how a check about things nobody looks at becomes a check nobody runs.
    An all-unreachable run *is* reported, because then nothing was tested.
    """
    stored = json.loads((root / STATUS).read_text(encoding="utf-8"))
    verdict = {(r["entry"], r["key"]): r for r in stored["verdicts"]}
    wrong: list[str] = []
    ran = unreachable = 0
    for entry, key, _, _ in matching(root):
        if (entry, key) in INSIDE_AFTER_ALL:
            continue
        row = verdict.get((entry, key), {})
        recipe = row.get("recheck") or {}
        if row.get("verdict") not in LIVE or "url" not in recipe:
            continue
        try:
            with urllib.request.urlopen(recipe["url"], timeout=timeout) as answer:
                status, body = answer.status, answer.read(65536).decode("utf-8", "replace")
        except urllib.error.HTTPError as refused:
            status, body = refused.code, ""
        except Exception as raised:  # noqa: BLE001 - any transport failure is "no answer"
            unreachable += 1
            print(f"    {recipe['url']}: unreachable ({raised!r})")
            continue
        ran += 1
        if "expect_status" in recipe and status != recipe["expect_status"]:
            wrong.append(
                f"{entry}: `{key}` expects {recipe['url']} to answer "
                f"{recipe['expect_status']} and it answered {status}. The claim has "
                "probably been overtaken; read it."
            )
        if "expect_text" in recipe and recipe["expect_text"] not in body:
            wrong.append(
                f"{entry}: `{key}` expects {recipe['url']} to carry "
                f"{recipe['expect_text']!r} and it does not. Read the claim."
            )
    return wrong, ran, unreachable


def main() -> int:
    parse = argparse.ArgumentParser(description=__doc__)
    parse.add_argument(
        "--run",
        action="store_true",
        help="also fetch every `url` recipe and compare. Needs the network.",
    )
    argued = parse.parse_args()

    said = problems()
    for one in said:
        print(one)
    failed = bool(said)
    if not said:
        rows = matching()
        print(f"ok    {len(rows)} outside-the-tree caveats, each live one saying how to re-check")

    if argued.run:
        wrong, ran, unreachable = run()
        for one in wrong:
            print(one)
        if ran == 0:
            print(
                "FAIL  every recipe was unreachable, so this run tested nothing. "
                "That is a network problem, not a clean result — say so rather than "
                "reading the exit code as a pass."
            )
            failed = True
        else:
            print(f"ok    {ran} recipes fetched, {unreachable} unreachable, {len(wrong)} wrong")
        failed = failed or bool(wrong)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
