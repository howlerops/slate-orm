#!/usr/bin/env python3
"""The site's factual claims, checked against the repository.

    python3 scripts/check_site_claims.py

`docs.py` checks that the pages hold together and `quickstarts.py` runs their
code. Neither reads the *prose*, and the prose is where the numbers are: "No
`unsafe`", "100,000 real New York yellow-taxi trips", "265-zone lookup",
"Python · Go · TypeScript". Two ledger entries recorded that gap —
`2026-09-16-the-site-looks-like-the-family-it-belongs-to.md` ("the landing
page's claims are not checked by anything") and
`2026-09-14-the-home-page-is-a-workbench.md` ("nothing verifies the two pages
agree").

Every claim here was true when it was written. That is the problem: a claim
nobody checks is true until it is not, and the landing page is the one document
a reader believes before they believe anything else.

# What it can and cannot do

A checkable claim names something this repository holds: a count in a fixture,
a lint in a crate root, a directory that exists. Those are below. A claim about
*character* — "the closest blueprint is FoundationDB's Record Layer, not an
ORM" — is not checkable and pretending otherwise would mean a regex deciding
whether a sentence is still true.

`UNCHECKED` names the claims in that second category, with the reason each is
there. It is the `EXPECTED_REFUSALS` idiom this repository uses elsewhere: a
list you are forced to edit is a list that stays true, and a reader comparing
it against the page can see exactly how much of the page is load-bearing.

# Why `scripts/` rather than `site/check/`

It reads `crates/`, `clients/` and `site/`, so it is not a site check that
happens to look outward — it is a repository check whose *subject* is the
site. The three files in `site/check/` all confine themselves to `site/`.

Needs no browser, no build and no network, so it runs in `scripts/check.sh`
beside the other guards rather than with the browser checks.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


#: Claims on the landing page that nothing here checks, and why not.
#:
#: Each key is a distinctive phrase that must still appear on the page — so a
#: claim that is *rewritten* forces a visit to this list rather than sitting in
#: it describing a sentence nobody wrote any more. That is the failure mode of
#: every roster in this repository, and this one is built to have it caught.
UNCHECKED: dict[str, str] = {
    "The closest blueprint is FoundationDB's Record Layer": (
        "a claim about lineage and intent; no property of the tree settles it"
    ),
    "works end to end, published nowhere": (
        "a claim about where the project is, which only a human knows"
    ),
    "row-level security enforced in the kernel rather": (
        "true, and checked by crates/slate-kernel's RLS suite rather than from here; "
        "a grep for the phrase would prove only that the phrase is present"
    ),
    "A key's encoding <em>is</em> its index.": (
        "a design summary; the keyspace tests are what make it true"
    ),
}


def crate_roots(root: Path) -> list[Path]:
    """Every crate's root module: `src/lib.rs`, or `src/main.rs` for a binary."""
    out = []
    crates = root / "crates"
    if not crates.is_dir():
        return out
    for crate in sorted(crates.iterdir()):
        if not crate.is_dir():
            continue
        for name in ("lib.rs", "main.rs"):
            root = crate / "src" / name
            if root.exists():
                out.append(root)
                break
        else:
            out.append(crate / "src" / "lib.rs")  # missing; reported below
    return out


def check(root: Path, unchecked: dict[str, str] | None = None) -> list[tuple[str, bool, str]]:
    """Every claim, as `(what, ok, detail)`, in the order they are reported.

    Returns rather than prints, and takes a root rather than reading the
    repository, so `scripts/test_check_site_claims.py` can write a tree where a
    claim is *false* and see this report it. A guard tested only against a
    clean tree asserts what a guard that does nothing asserts.
    """
    # The roster is a parameter so that `scripts/test_check_site_claims.py` can
    # write a page and a roster that go together. Against the repository it is
    # always `UNCHECKED`, and `main` passes nothing.
    roster = UNCHECKED if unchecked is None else unchecked
    out: list[tuple[str, bool, str]] = []

    def record(what: str, ok: bool, detail: str = "") -> None:
        out.append((what, ok, detail))

    index = root / "site" / "index.html"
    page = index.read_text(encoding="utf-8") if index.exists() else ""
    record("the landing page exists", index.exists(), str(index))
    if not page:
        return out

    # --- "No `unsafe`" -----------------------------------------------------
    #
    # Two halves, and the second is the one that was missing. That there is no
    # `unsafe` block today is a fact about the current tree; that every crate
    # root *forbids* it is what keeps the claim true, and four of twelve crates
    # did not — `slate-sql`, `slate-wasm`, `slate-clickbench`, and nothing
    # stopped the next one.
    record(
        "the page still claims no unsafe, which the rest of this section is about",
        "No <code>unsafe</code>" in page,
    )

    blocks = []
    for source in (root / "crates").rglob("*.rs"):
        if "/target/" in str(source):
            continue
        for number, line in enumerate(source.read_text(encoding="utf-8").splitlines(), 1):
            # `unsafe` opening a block or qualifying an item, not the word in
            # prose: `#![forbid(unsafe_code)]` and "no unsafe anywhere" are
            # both mentions and neither is a use.
            if re.search(r"\bunsafe\s*(\{|fn\b|impl\b|trait\b)", line):
                blocks.append(f"{source.relative_to(root)}:{number}: {line.strip()}")
    record("no crate contains an unsafe block", not blocks, "\n      ".join(blocks))

    unguarded = [
        str(crate_root.relative_to(root))
        for crate_root in crate_roots(root)
        if not crate_root.exists()
        or "#![forbid(unsafe_code)]" not in crate_root.read_text(encoding="utf-8")
    ]
    record(
        "every crate root forbids unsafe, so the claim stays true without anyone checking",
        not unguarded,
        "\n      ".join(unguarded),
    )

    # --- the fixture's numbers ---------------------------------------------
    #
    # Both appear in the prose as round numbers a reader takes literally. Both
    # come from somewhere: the sample size the fixture generator uses, and the
    # zone CSV compiled into the binding.
    generator = root / "site" / "data" / "make-trips.py"
    sample = (
        re.search(r"^SAMPLE = ([\d_]+)", generator.read_text(encoding="utf-8"), re.M)
        if generator.exists()
        else None
    )
    record("the trip generator states its sample size", sample is not None, str(generator))
    if sample:
        trips = int(sample.group(1).replace("_", ""))
        record(
            f"the page's trip count is the fixture's ({trips:,})",
            f"{trips:,}" in page,
            f"make-trips.py samples {trips:,} and index.html does not say so",
        )
        # The sample output block quotes a plan line. Its row count is the same
        # claim in a different notation, and a reader comparing the two would
        # notice a disagreement before this check did — which is why both are
        # checked rather than one.
        record(
            f"the sample plan's row count agrees ({trips})",
            f"rows={trips}" in page,
            "the `Table Scan` line in the hero code block names a different row count",
        )

    csv = root / "crates" / "slate-wasm" / "src" / "taxi_zones.csv"
    if csv.exists():
        count = len(csv.read_text(encoding="utf-8").strip().splitlines()) - 1  # the header
        record(
            f"the page's zone count is the CSV's ({count})",
            f"{count}-zone" in page,
            f"taxi_zones.csv holds {count} zones",
        )
    else:
        record("the zone CSV exists", False, str(csv))

    # --- the three clients --------------------------------------------------
    record(
        "the page names the three clients",
        re.search(r"Python\s*(&middot;|·)\s*Go\s*(&middot;|·)\s*TypeScript", page) is not None,
    )
    missing = [
        name for name in ("python", "go", "typescript") if not (root / "clients" / name).is_dir()
    ]
    record("each named client is a directory", not missing, ", ".join(missing))

    # --- the roster ---------------------------------------------------------
    absent = [phrase for phrase in roster if phrase not in page]
    record(
        "every phrase UNCHECKED names is still on the page",
        not absent,
        "\n      ".join(f"{p!r} — {roster[p]}" for p in absent),
    )
    record("UNCHECKED gives a reason for each", all(roster.values()))
    return out


def main() -> int:
    failed = 0
    for what, ok, detail in check(REPO):
        if ok:
            print(f"ok    {what}")
        else:
            failed += 1
            print(f"FAIL  {what}" + (f"\n      {detail}" if detail else ""))
    print()
    if failed:
        print(f"{failed} failed")
        return 1
    print(f"the landing page's checkable claims hold, and {len(UNCHECKED)} are listed as unchecked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
