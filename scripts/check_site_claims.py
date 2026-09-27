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


#: The *fixture's* trip count, as the pages state it.
#:
#: Narrower than "a number near the word trips", and it has to be: this
#: repository legitimately states more than one trip count.
#: `docs/performance.md` reports loading **2,964,619** — the whole of January
#: 2024, which is the point of that measurement — and a guard demanding every
#: trip count equal the workbench's would have demanded that one be wrong. The
#: first draft did exactly that, and the failure is the reason this pattern
#: names the fixture rather than the noun.
#:
#: So a match needs "New York" beside it, or the phrase "N-trip sample" that
#: `storage.html` uses. Four digits without a comma are excluded separately:
#: `2024 yellow-trip data` is a year, and that was the other false positive.
TRIP_COUNT = re.compile(
    r"(\d{1,3}(?:,\d{3})+|\d{5,})(?:[^<]{0,40}?New York[^<]{0,30}?\btrips?\b"
    r"|-trip sample)",
    re.I,
)
ZONE_COUNT = re.compile(r"(\d{2,4})[- ]zones?\b", re.I)


def prose(text: str) -> str:
    """The page with its code blocks removed, and its whitespace collapsed.

    Sample output is not a claim. The landing page's hero block prints
    `132  4,837` above a line reading `Table Scan on trips`, and with
    whitespace collapsed that is a number beside the word — which the first
    draft reported as the page claiming 4,837 trips.

    Collapsing whitespace is not optional either: the landing page wraps
    "100,000 real New York / yellow-taxi trips" across a line, so a
    line-bounded pattern misses the one page this check started with.
    """
    without_code = re.sub(r"<pre\b.*?</pre>|<code\b.*?</code>|```.*?```", " ", text, flags=re.S)
    return re.sub(r"\s+", " ", without_code)


def claim_pages(root: Path) -> list[Path]:
    """Every page that makes claims about the project, to a reader.

    The site, the README and the design notes. Not `ledger/`, which is dated
    and append-only: an entry stating what was true in September is a record,
    not a claim, and correcting it would destroy the thing it is for.
    """
    pages = sorted((root / "site").rglob("*.html"))
    readme = root / "README.md"
    if readme.exists():
        pages.append(readme)
    pages.extend(sorted((root / "docs").glob("*.md")))
    return pages


def refused_keywords(root: Path) -> list[str]:
    """The keywords `sql.rs` documents as refused by name.

    Read out of the sentence that lists them rather than out of the parser's
    `match`, because the parser refuses them in three places and a regex over
    Rust control flow is a parser for Rust. The sentence is tied to the code by
    `sql::grammar::the_refused_keywords_are_all_named`, which is the link that
    makes reading prose here sound.
    """
    source = root / "crates" / "slate-sql" / "src" / "sql.rs"
    if not source.exists():
        return []
    match = re.search(r"//! (`UNION`[^\n]*(?:\n//! [^\n]*)*?) are refused by", source.read_text(encoding="utf-8"))
    if not match:
        return []
    return re.findall(r"`([A-Z ]+)`", match.group(1))


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

    # --- the same two numbers, wherever else they are stated ---------------
    #
    # The first version of this read `site/index.html` and nothing else, which
    # its own entry recorded as a caveat: the fixture's size is claimed on the
    # workbench page, on two docs pages, in `README.md` and in
    # `docs/performance.md`, and a page nobody checks is exactly where a stale
    # number lives longest. Whitespace is collapsed first, because the landing
    # page wraps "100,000 real New York / yellow-taxi trips" across a line and
    # a line-bounded pattern misses the one page this started with.
    wrong, seen = [], 0
    for page_path in claim_pages(root):
        body = prose(page_path.read_text(encoding="utf-8"))
        if sample:
            for stated in TRIP_COUNT.findall(body):
                seen += 1
                if int(stated.replace(",", "")) != trips:
                    wrong.append(f"{page_path.relative_to(root)}: {stated} trips")
    record(
        "every page that states a trip count states the fixture's",
        not wrong,
        "\n      ".join(wrong),
    )
    # A pattern that matches nothing satisfies "every match agrees" perfectly,
    # and that is how a narrowed pattern stops checking anything: this one was
    # narrowed twice while it was being written. The floor is the *landing
    # page and one more*, because one match is what the check above it already
    # covers and this widening exists to reach past that page.
    record(
        f"the fixture's size is found on more than one page ({seen} statements)",
        seen > 1,
        "the trip-count pattern has stopped matching the pages it was written for",
    )

    csv = root / "crates" / "slate-wasm" / "src" / "taxi_zones.csv"
    if csv.exists():
        count = len(csv.read_text(encoding="utf-8").strip().splitlines()) - 1  # the header
        record(
            f"the page's zone count is the CSV's ({count})",
            f"{count}-zone" in page,
            f"taxi_zones.csv holds {count} zones",
        )
        wrong, seen = [], 0
        for page_path in claim_pages(root):
            body = prose(page_path.read_text(encoding="utf-8"))
            for stated in ZONE_COUNT.findall(body):
                seen += 1
                if int(stated) != count:
                    wrong.append(f"{page_path.relative_to(root)}: {stated} zones")
        record(
            "every page that states a zone count states the CSV's",
            not wrong,
            "\n      ".join(wrong),
        )
        record(
            f"the zone count is found at all ({seen} statements)",
            seen > 0,
            "the zone-count pattern has stopped matching",
        )
    else:
        record("the zone CSV exists", False, str(csv))

    # --- what the parser refuses, and what the docs say it refuses ----------
    #
    # A refusal is a feature here: `ledger/2026-09-16-a-subquery-is-two-reads-
    # not-an-operator.md` argues that the point of refusing by name is that the
    # reader is told, and a refusal the *documentation* does not mention is one
    # the reader meets as a surprise.
    #
    # The names come from `sql.rs`'s module documentation rather than from a
    # list here, and that documentation is tied to the parser by
    # `sql::grammar::the_refused_keywords_are_all_named`. Parser → module docs
    # → site, each link checked, and no list in the middle for anyone to keep
    # in step by hand.
    refused = refused_keywords(root)
    record("the parser's module docs name some refusals", bool(refused), str(refused))
    if refused:
        docs = " ".join(
            prose(page_path.read_text(encoding="utf-8"))
            for page_path in claim_pages(root)
        )
        unsaid = [name for name in refused if name not in docs]
        record(
            "every keyword the parser refuses by name is named in the docs",
            not unsaid,
            ", ".join(unsaid),
        )

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
    print(
        f"the site's checkable claims hold across {len(claim_pages(REPO))} pages, "
        f"and {len(UNCHECKED)} are listed as unchecked"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
