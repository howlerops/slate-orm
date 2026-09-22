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

**It reads `crates/` and `docs/`.** Code only, until #283. The page a reader
actually reaches is `docs/performance.md`, and its "a point read costs about 3"
outlived the constant by nine tasks — surviving #277's sweep precisely because
that sweep could not see it. Prose in `docs/` is read as current whether or not
a guard is pointed at it.

Three kinds of sentence there trip these patterns without asserting anything
current: history that names the old figure beside the new one, a withdrawal
that quotes what is being withdrawn, and a measurement of a *different*
quantity — "a point read costs three object-store GETs" is about GETs in a
cacheless build, not about `POINT_READ_COST`. `~~strikethrough~~` handles the
second and is preferred wherever a paragraph also states the live figure,
because it leaves that figure under the guard. `NOT_A_CLAIM` handles the other
two.

WHAT IS CHECKED, AND WHY IT IS NARROW

Three derived figures and one restatement, all as they are actually written:

* **what a point read costs**, against `POINT_READ_COST`;
* **what a scan returns per request**, against `1 / SCAN_ROW_COST`;
* **the crossover** — how many rows a scan must save per row fetched before an
  index pays — against `POINT_READ_COST / SCAN_ROW_COST`;
* **a literal copy of either constant** — `POINT_READ_COST = 3.0` in a comment
  or a printed header — against the constant itself. `stats.rs`'s own `pub
  const` line is the one copy that is not a restatement.

Numbers are matched as digits *and* as the English words this repository
writes them in, because it writes them both ways in the same paragraph, and
through the markdown emphasis it wraps them in.

A run of consecutive comment lines is read as one chunk, because a sentence
in a doc comment wraps and the claim wraps with it. Reading line by line saw
neither half of `a point read costs about` / `**three requests**`.

**Every crate is read, not only `slate-kernel`.** It was only the kernel for
about an hour, on the reasoning that the constants are decided there and the
reasoning lives there. That was wrong by one file and the file mattered:
`slate-slatedb`'s `cost_at_scale` example *printed* `POINT_READ_COST = 3.0` in
its header, "restated so the table below can be read against them" — a
benchmark misreporting the model it exists to measure, in output a reader sees
rather than in a comment they might not. It prints the constants now.

**And widening the tree was not enough, which is the more useful half.** The
same file still held two stale claims after that widening, because neither was
in a form this could see: a bullet whose figure sat past a line break, and a
summary line printing `3 requests` with no "a point read costs" clause beside
it. Widening *what counts as a claim* — the three forms above — found a ninth
stale sentence in `latency.rs`, a file eight rounds of sweeping by hand had
never flagged. A guard aimed at the right tree and the wrong shape reads as
thorough and is not.

`docs/` is still out of scope, and deliberately: `correctness.md` narrates the
history of these numbers at length, and a guard that cannot tell "it costs
three" from "it cost three until #269" would either fire on every paragraph or
need an exemption per paragraph. `site/check/docs.py` is that tree's guard.

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
from typing import NamedTuple

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
STATS = ROOT / "crates/slate-kernel/src/stats.rs"
WHERE = ROOT / "crates"

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

#: Markdown emphasis between the verb and the figure: this repository writes
#: **8,000 rows per request** and **three requests** in bold, and `1` in
#: backticks. Not cosmetic — `**` between "costs about" and "three" is what
#: hid the one stale bullet in `cost_at_scale`'s module doc from the widening
#: in #278 that was aimed at that file, and hid the corrected bullet from
#: being checked at all.
MARKUP = r"[*`_]{0,2}"

#: "a point read costs about three" / "a point read costs 1" /
#: "POINT_READ_COST says 3 requests".
#:
#: The trailing `(?![,0-9])` refuses a figure that is part of a larger
#: grouped number, because the sentences around here also state measured
#: *totals*: `join.rs` says "four hundred point reads cost 1,221 requests",
#: and without the lookahead that reads as "a point read costs 1". It passed
#: only because 1,221 happens to begin with a 1 — mutate the total to 2,442
#: and the guard accuses two correct files of saying a point read costs 2.
#: A per-read claim and a per-400-rows total are different sentences, and
#: only the first is this pattern's business.
#:
#: The `\b` in front of it is load-bearing for a second reason, found by
#: dropping it: `stamp.rs` says a cacheless build "costs ~3x the object-store
#: requests", and without the boundary that reads as a point read costing 3.
#: A *ratio* is a third kind of sentence. Both guards are needed; neither
#: alone is enough.
PER_READ = re.compile(
    r"(?:point reads? costs?|POINT_READ_COST says) "
    r"(?:about |roughly |~)?" + MARKUP + r"([0-9.]+|[a-z]+)\b(?![,0-9])",
    re.IGNORECASE,
)
#: "a scan returns about eight thousand rows per request", "~8000 rows per
#: object-store request". Derived from `SCAN_ROW_COST` the same way.
PER_REQUEST = re.compile(
    r"(?:about |roughly |~)?" + MARKUP + r"([0-9,]+|[a-z-]+)(?: thousand)? "
    r"rows per (?:object-store )?request",
    re.IGNORECASE,
)
#: A literal copy of a constant: ``POINT_READ_COST = 3.0`` in a comment or a
#: printed string. This is the form that actually went stale — the English
#: clause beside it was what the guard happened to catch, and a restatement
#: with no clause (a benchmark header, a bullet in a module doc) went unread
#: for nine tasks. The declaration in `stats.rs` is the one copy that is not
#: a restatement, so a `pub const` line is skipped.
RESTATED = re.compile(r"\b(POINT_READ_COST|SCAN_ROW_COST)\s*=\s*([0-9._]+)")
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


#: A line that is only a comment, with its marker: `//`, `///`, `//!`.
COMMENT = re.compile(r"^\s*//[!/]?\s?")
#: A strikethrough span, which `ledger/README.md` asks for on a withdrawn
#: figure. Spans lines, because the passages that use it do.
STRUCK = re.compile(r"~~.*?~~", re.DOTALL)


class Chunk(NamedTuple):
    """A run of text to match against, and the lines it came from.

    `first` and `last` differ only for a joined comment run. Reporting the
    range rather than `first` alone matters for a module doc, where every
    violation in eighty lines of `//!` would otherwise be reported at line 1
    — which reads as a guard that cannot find anything.
    """

    first: int
    last: int
    text: str

    def where(self) -> str:
        """`12` for a line, `12-19` for a run."""
        return str(self.first) if self.first == self.last else f"{self.first}-{self.last}"


#: Written above a paragraph in `docs/` to say it is not asserting what the
#: constants are *now*.
#:
#: Three kinds of prose trip these patterns without claiming anything current,
#: and all three are in `docs/` today: history that names the old figure beside
#: the new one ("`n > 8000k` rather than `n > 24000k`"), a withdrawal that
#: quotes the figure being withdrawn, and a measurement of a *different*
#: quantity — "a point read costs three object-store GETs" is about GETs in a
#: cacheless build, not about `POINT_READ_COST`, and the two are only
#: coincidentally both about point reads.
#:
#: `~~strikethrough~~` already covers the second of those and is preferred
#: where it fits, because a struck figure is legible to a reader as withdrawn.
#: It does not fit the other two: a sentence contrasting old with new has to
#: stay readable, and a measurement of GETs is not withdrawn at all.
NOT_A_CLAIM = "<!-- not a cost-model claim -->"

#: A markdown fence. Pasted benchmark output inside one is a record of what a
#: run printed, not prose asserting a current value, so it is skipped — the
#: same reasoning that keeps `check_table_provenance.py` out of fences.
FENCE = re.compile(r"^\s*```")


def prose(text: str) -> list[Chunk]:
    """A markdown file as blank-line-separated paragraphs.

    Markdown wraps a sentence across lines exactly as a doc comment does, so
    the claim has to be matched against the joined paragraph rather than each
    line — the same reason `paragraphs` joins comment runs.
    """
    chunks: list[Chunk] = []
    run: list[str] = []
    start = 0
    fenced = False
    lines = text.splitlines()
    for at, line in enumerate([*lines, ""], start=1):
        if FENCE.match(line):
            fenced = not fenced
            line = ""
        if fenced:
            continue
        if line.strip():
            if not run:
                start = at
            run.append(line)
            continue
        if run:
            chunks.append(Chunk(start, at - 1, " ".join(run)))
            run = []
    return chunks


def paragraphs(text: str) -> list[Chunk]:
    """The file as `(first line, text)` chunks a claim can be matched against.

    A run of consecutive comment lines is joined into one chunk, because a
    sentence in a doc comment wraps and the claim wraps with it. Reading line
    by line — which this did until #279 — cannot see
    `a point read costs about\n//!   **three requests**`, and that is not a
    contrived split: it is what `rustfmt` and a 100-column margin produce, and
    it is how the one stale claim in `cost_at_scale`'s module doc survived the
    widening that was aimed at exactly that file.

    Every other line is its own chunk. A claim split across a Rust string
    literal's backslash-continuation is therefore still unseen; nothing splits
    one today, and joining arbitrary adjacent code lines invents sentences
    that were never written.
    """
    chunks: list[Chunk] = []
    run: list[str] = []
    start = 0
    last = 0
    # The `None` sentinel closes a run that reaches the end of the file. A
    # second copy of the flush after the loop is the obvious way to write
    # this and was how it was written: every fixture here ends on a comment,
    # so only that copy ran for a joined run, and a mutation to the in-loop
    # copy survived with all tests green. One flush cannot drift from itself.
    lines: list[tuple[int, str | None]] = [
        (at, line) for at, line in enumerate(text.splitlines(), start=1)
    ]
    for at, line in [*lines, (len(lines) + 1, None)]:
        marker = COMMENT.match(line) if line is not None else None
        if marker and line is not None and line.strip().startswith("//"):
            if not run:
                start = at
            run.append(line[marker.end():])
            last = at
            continue
        if run:
            chunks.append(Chunk(start, last, " ".join(run)))
            run = []
        if line is not None:
            chunks.append(Chunk(at, at, line))
    return chunks


def live(text: str) -> str | None:
    """`text` with withdrawn figures removed, or None if it is all history.

    A struck-through figure is a record of what was believed, not a claim
    about today. Removing the spans rather than skipping the whole chunk is
    what lets one paragraph carry both — `stats.rs` strikes two sentences
    through and then states the current figure in the third.
    """
    stripped = STRUCK.sub(" ", text)
    # An unbalanced `~~` means the span is open across a boundary this cannot
    # see. Skipping is the conservative read: a missed claim, never a false
    # accusation against a passage that is marked as history.
    if "~~" in stripped:
        return None
    # Whitespace is collapsed because a joined comment run carries the second
    # line's indent into the middle of the sentence — `costs` and `about` end
    # up three spaces apart, and every pattern here is written with the one
    # space a reader sees. This cost the first two cases of the wrapped-claim
    # test, which is the cheapest place to have found it.
    return re.sub(r"\s+", " ", stripped)


def sources(where: Path = WHERE, docs: Path | None = DOCS) -> list[Path]:
    """Every Rust file under `where` and every doc under `docs`, in order.

    `where` is every crate, not the kernel, since #278 — the sentence that
    said "under the kernel" outlived that change by two tasks, in the file
    whose whole job is catching sentences that outlive a change.

    `docs` arrived with #283. Excluding it was costing something real: the
    prose a reader actually reaches said "a point read costs about 3" for
    nine tasks after the constant became 1.0, and nothing could see it
    because the guard read only code.
    """
    found = [path for path in sorted(where.rglob("*.rs")) if path.is_file()]
    if docs is not None and docs.is_dir():
        found += [path for path in sorted(docs.glob("*.md")) if path.is_file()]
    return found


def check(
    where: Path = WHERE, stats: Path = STATS, docs: Path | None = DOCS
) -> tuple[int, list[str]]:
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

    per_request = 1.0 / values["SCAN_ROW_COST"]

    seen = 0
    wrong = []
    for path in sources(where, docs):
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        markdown = path.suffix == ".md"
        chunks = prose(text) if markdown else paragraphs(text)
        excused = False
        for chunk in chunks:
            if markdown:
                # A marker covers the paragraph it is in, and — when it is a
                # paragraph of its own, which is how it reads best above a
                # long passage — the next one.
                if NOT_A_CLAIM in chunk.text:
                    excused = chunk.text.strip() == NOT_A_CLAIM
                    continue
                if excused:
                    excused = False
                    continue
            current = live(chunk.text)
            if current is None:
                continue
            for match in PER_READ.finditer(current):
                value = number(match.group(1), thousands=False)
                if value is None:
                    continue
                seen += 1
                if abs(value - read) > 0.01:
                    wrong.append(
                        f"{named(path)}:{chunk.where()} says a point read "
                        f"costs {match.group(1)}; POINT_READ_COST is {read:g}."
                    )
            for match in PER_REQUEST.finditer(current):
                written = match.group(1)
                value = number(written, thousands=" thousand" in match.group(0))
                if value is None:
                    continue
                seen += 1
                if abs(value - per_request) > 1.0:
                    wrong.append(
                        f"{named(path)}:{chunk.where()} says a scan returns {written} "
                        f"rows per request; 1 / SCAN_ROW_COST is "
                        f"{per_request:g}."
                    )
            for match in CROSSOVER.finditer(current):
                written = match.group(1) or match.group(2)
                value = number(written, thousands=match.group(1) is not None)
                if value is None:
                    continue
                seen += 1
                if abs(value - crossover) > 1.0:
                    wrong.append(
                        f"{named(path)}:{chunk.where()} puts the crossover at "
                        f"{written}; POINT_READ_COST / SCAN_ROW_COST is "
                        f"{crossover:g}."
                    )
            # A restatement of the constant itself, which is the form the
            # English patterns above cannot see and the one that went stale.
            # `stats.rs`'s own declarations are excluded by RESTATED itself:
            # a declaration reads `NAME: f64 = 1.0`, and the pattern wants
            # `NAME = 1.0`, so the type annotation is what separates the one
            # authoritative copy from every restatement of it. There was an
            # explicit `if "pub const" in chunk: continue` here for an hour;
            # a mutation deleting it changed no verdict, because it had never
            # excluded anything. A dead safety check is worse than none.
            for match in RESTATED.finditer(current):
                name, written = match.group(1), match.group(2)
                value = number(written, thousands=False)
                if value is None:
                    continue
                seen += 1
                if abs(value - values[name]) > 1e-9:
                    wrong.append(
                        f"{named(path)}:{chunk.where()} restates {name} as {written}; "
                        f"it is {values[name]:g}. Print it from "
                        f"`slate_kernel::stats` rather than copying it."
                    )
    return seen, wrong


def main(
    where: Path = WHERE, stats: Path = STATS, docs: Path | None = DOCS
) -> int:
    # All three are arguments so the never-fires guard below can be tested. It
    # could not be: the cases call `check` directly, `main` read the two
    # module constants, and a mutation deleting the guard changed no verdict
    # because this crate always has claims. That is the third guard this week
    # with the judgement in `main` and the tests one level under it.
    #
    # `docs` joined them for a sharper reason: #283 gave it a real default, and
    # the never-fires case — an empty tree, which must fail — began passing,
    # because `main` was reading the repository's own five doc claims over the
    # fixture's zero. A parameter that a test cannot override is a parameter
    # the test is not really exercising.
    seen, wrong = check(where, stats, docs)
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
