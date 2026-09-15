#!/usr/bin/env python3
"""Generate `src/zones.rs`: a transition table for a curated set of timezones.

Named zones were refused for two rounds, with a reason that was true as far as
it went — "there is no timezone database here, so `America/New_York` could only
be honoured by guessing at daylight saving". Pulling in the whole IANA database
is what that was rejecting: megabytes, a parser, and a release cadence, for a
binding whose point is that it fits in a browser tab.

A *table* is not a database. One zone's transitions over 1900-2100 are a few
hundred pairs of integers, which is a few kilobytes — so the zones anyone asks
for can be exact rather than refused, and the rest stay refused by name.

Run: python3 crates/slate-kernel/scripts/generate_zones.py

The output is committed and its freshness is asserted by
`tests/zones_generated.rs`, which runs this script with `--check`. A generated
file nobody regenerates drifts, and the drift is invisible.

`--check` compares the *transitions*, not the bytes. The protobuf stub tests
byte-compare because their input — the `.proto` — is in the repository, so a
regeneration is reproducible and a formatting difference means the generator
changed. This script's input is the system's `tzdata`, which is not: a byte
compare would also fail on a comment, on the version string in the header, and
on a `cargo fmt` from a newer toolchain. Comparing the pairs fails on exactly
one thing, which is the thing worth failing on — the committed table no longer
says what `tzdata` says.

That still goes red when `tzdata` revises a rule, and that red is correct: the
table really is stale and the fix really is to regenerate. It is not the
`grpcio-tools` failure, where the *generator* moved and the committed output was
never wrong.

## What this is honest about

`tzdata` is versioned, and transitions after the current year are *predictions*
— the rules as they stand, projected. The United States moved its DST
boundaries in 2007 and could again. So the header records which `tzdata` this
was generated from, and the table is a snapshot rather than an oracle. A query
about 2087 is answered from today's rules, which is the best anyone can do and
worth saying out loud.
"""

from __future__ import annotations

import argparse
import datetime
import pathlib
import re
import subprocess
import sys
import zoneinfo

#: The zones this ships, and why each one is here.
#:
#: Curated rather than complete: every zone is a few kilobytes, and the point of
#: choosing is that the list covers the *shapes* a timezone can have rather than
#: the places it can name.
ZONES = [
    ("UTC", "the identity, so a caller can name it explicitly"),
    ("America/New_York", "northern DST, and the zone the taxi sample is in"),
    ("America/Chicago", "a second US zone, to catch a table indexed by accident"),
    ("America/Denver", "and a third"),
    ("America/Los_Angeles", "and a fourth"),
    ("America/Phoenix", "US, and no DST at all — Arizona opted out"),
    ("Europe/London", "northern DST on different dates from the US"),
    ("Europe/Paris", "and central Europe, an hour off London"),
    ("Europe/Berlin", "the same rules as Paris, different history"),
    ("Asia/Tokyo", "no DST, and a whole-hour offset"),
    ("Asia/Kolkata", "a *half*-hour offset, which an hours-only table gets wrong"),
    ("Asia/Kathmandu", "a quarter-hour offset, which a half-hour table gets wrong"),
    ("Australia/Sydney", "southern DST: summer in January, so the sign flips"),
    ("Pacific/Auckland", "southern DST again, and a different transition month"),
]

#: 1900-01-01 to 2100-01-01. Before 1900 the tables are dominated by local mean
#: time to the second, which is true history and not what anyone querying a
#: timestamp column wants; after 2100 they are pure projection.
FROM_YEAR = 1900
TO_YEAR = 2100

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE.parent / "src" / "zones.rs"


def require_tzdata() -> None:
    """Fail clearly when the system has no timezone database.

    Without this the first `ZoneInfo` call raises deep inside `transitions`,
    and in a container with no `tzdata` package that traceback reads like a
    bug in this script. It is not, and the fix is one `apt install`.
    """
    try:
        zoneinfo.ZoneInfo("America/New_York")
    except Exception as error:  # noqa: BLE001 - any failure here means the same thing
        raise SystemExit(
            f"no IANA timezone database on this system ({error}). Install one "
            f"(`apt-get install tzdata`, or `pip install tzdata`) — this script "
            f"reads the zones from it and cannot invent them."
        ) from error


def offset_at(zone: zoneinfo.ZoneInfo, instant: int) -> int:
    """The zone's offset from UTC, in seconds, at an epoch second."""
    when = datetime.datetime.fromtimestamp(instant, datetime.timezone.utc)
    delta = when.astimezone(zone).utcoffset()
    assert delta is not None
    return int(delta.total_seconds())


def transitions(name: str) -> list[tuple[int, int]]:
    """Every offset change in the window, as `(instant, offset)` pairs.

    Found by walking a day at a time and refining each change to the second by
    bisection. Crude, and right: `zoneinfo` exposes no transition list, and
    reading the TZif binary would be a second parser to be wrong in.

    The first pair is the window's start, so a lookup before any transition
    still finds an offset rather than falling off the front.
    """
    zone = zoneinfo.ZoneInfo(name)
    start = int(
        datetime.datetime(FROM_YEAR, 1, 1, tzinfo=datetime.timezone.utc).timestamp()
    )
    end = int(
        datetime.datetime(TO_YEAR, 1, 1, tzinfo=datetime.timezone.utc).timestamp()
    )

    out = [(start, offset_at(zone, start))]
    day = 86_400
    previous = out[0][1]
    at = start + day
    while at < end:
        current = offset_at(zone, at)
        if current != previous:
            # Somewhere in (at - day, at]. Bisect to the second.
            low, high = at - day, at
            while low + 1 < high:
                middle = (low + high) // 2
                if offset_at(zone, middle) == previous:
                    low = middle
                else:
                    high = middle
            out.append((high, current))
            previous = current
        at += day
    return out


def tzdata_version() -> str:
    """Which `tzdata` this came from, as far as the system will say.

    `zoneinfo` does not report it, so this reads the file Debian and friends
    ship it in and falls back to a plain statement of ignorance — which is
    better in a generated header than a confident wrong version.
    """
    for candidate in ("/usr/share/zoneinfo/+VERSION", "/usr/share/zoneinfo/tzdata.zi"):
        path = pathlib.Path(candidate)
        if not path.exists():
            continue
        text = path.read_text(errors="replace")
        if candidate.endswith("+VERSION"):
            return text.strip()
        for line in text.splitlines():
            if line.startswith("# version"):
                return line.split()[-1]
    return "unknown (the system does not record one)"


def build() -> tuple[str, dict[str, list[tuple[int, int]]], str]:
    """The file's text, the tables it contains, and the `tzdata` version.

    Returned together rather than written straight out, so `--check` compares
    against the same tables a write would have produced and there is no second
    code path to be wrong in.
    """
    require_tzdata()
    version = tzdata_version()
    tables = {name: transitions(name) for name, _ in ZONES}
    lines = [
        "//! Timezone transition tables, generated. Do not edit.",
        "//!",
        "//! Generated by `scripts/generate_zones.py` from the system's IANA",
        f"//! `tzdata`, version `{version}`, over {FROM_YEAR}-{TO_YEAR}.",
        "//!",
        "//! # This is a snapshot, not an oracle",
        "//!",
        "//! Transitions after the year this was generated in are *predictions*:",
        "//! the rules as they stood, projected forward. The United States moved",
        "//! its DST boundaries in 2007 and could again, so a query about 2087 is",
        "//! answered from today's rules. That is the best anyone can do and it is",
        "//! worth saying rather than implying otherwise.",
        "//!",
        "//! # Why a table rather than a database",
        "//!",
        "//! Named zones were refused twice, on the grounds that honouring them",
        "//! needs the IANA database — megabytes, a parser and a release cadence,",
        "//! in a binding whose point is that it fits in a browser tab. That is a",
        "//! fair objection to a *database* and not to a *table*: what a lookup",
        "//! needs is a sorted list of instants and offsets, which for the zones",
        "//! below is a few kilobytes. Zones outside the list are still refused by",
        "//! name, and the refusal now says which ones are here.",
        "",
        "/// The `tzdata` version these tables were generated from.",
        f'pub const TZDATA_VERSION: &str = "{version}";',
        "",
        "/// The first and last year the tables cover. Outside it, the nearest",
        "/// transition's offset applies — which is right at the start (the zone",
        "/// had that offset before the window) and a projection at the end.",
        f"pub const COVERS: (i64, i64) = ({FROM_YEAR}, {TO_YEAR});",
        "",
        "/// One zone: its IANA name and its transitions, ascending by instant.",
        "///",
        "/// Each entry is the UTC instant a new offset takes effect, and that",
        "/// offset in seconds east of UTC. The first entry is the window's start,",
        "/// so a lookup before any real transition still finds an offset.",
        "#[derive(Debug)]",
        "pub struct Zone {",
        "    /// The IANA name, exactly as a caller writes it.",
        "    pub name: &'static str,",
        "    /// `(instant, offset_seconds)`, ascending by instant.",
        "    pub transitions: &'static [(i64, i32)],",
        "}",
        "",
    ]

    total = 0
    for name, why in ZONES:
        table = tables[name]
        total += len(table)
        constant = name.upper().replace("/", "_").replace("-", "_").replace("+", "_")
        lines.append(f"/// `{name}`: {why}.")
        lines.append(
            f"static {constant}: &[(i64, i32)] = &["
        )
        for instant, offset in table:
            lines.append(f"    ({instant}, {offset}),")
        lines.append("];")
        lines.append("")

    lines.append("/// Whether this knows a zone by that name.")
    lines.append("///")
    lines.append("/// Case-sensitive, as IANA names are: `america/new_york` is")
    lines.append("/// not a zone. An edge that wants to refuse an unknown name")
    lines.append("/// before evaluating anything asks this; the scalar itself")
    lines.append("/// has nowhere to put an error and answers null.")
    lines.append("#[must_use]")
    lines.append("pub fn has(name: &str) -> bool {")
    lines.append("    ZONES.binary_search_by(|zone| zone.name.cmp(name)).is_ok()")
    lines.append("}")
    lines.append("")
    lines.append("/// Every name, sorted, spelled as a caller must spell it.")
    lines.append("pub fn names() -> impl Iterator<Item = &'static str> {")
    lines.append("    ZONES.iter().map(|zone| zone.name)")
    lines.append("}")
    lines.append("")
    lines.append("/// The names, comma-separated — the tail of a refusal.")
    lines.append("///")
    lines.append("/// Here rather than at each edge so the gRPC server, the SQL")
    lines.append("/// front end and the wasm binding cannot drift into three")
    lines.append("/// different lists, which is what happened to the *previous*")
    lines.append("/// refusal: two of the three said \"there is no timezone")
    lines.append("/// database here\" long after one of them had an offset.")
    lines.append("#[must_use]")
    lines.append("pub fn listing() -> String {")
    lines.append("    names().collect::<Vec<_>>().join(\", \")")
    lines.append("}")
    lines.append("")
    lines.append("/// Every zone this knows, sorted by name so a lookup can bisect.")
    lines.append("///")
    lines.append(
        f"/// {len(ZONES)} zones and {total} transitions, which is what the module"
    )
    lines.append("/// docs mean by \"a few kilobytes\".")
    lines.append("pub static ZONES: &[Zone] = &[")
    for name, _ in sorted(ZONES):
        constant = name.upper().replace("/", "_").replace("-", "_").replace("+", "_")
        lines.append(f'    Zone {{ name: "{name}", transitions: {constant} }},')
    lines.append("];")
    lines.append("")

    return "\n".join(lines), tables, version


#: The committed file's tables, parsed back out.
#:
#: A parser rather than a Rust-side check because the comparison has to happen
#: where the fresh tables are, and they are here. The pattern is narrow on
#: purpose: it matches the generator's own output shape, so a file that has
#: drifted into some other shape fails to parse rather than parsing to
#: something plausible.
def committed() -> dict[str, list[tuple[int, int]]]:
    text = OUT.read_text()
    constants: dict[str, list[tuple[int, int]]] = {}
    for match in re.finditer(
        # To the first `];` rather than to one at the start of a line: a
        # single-entry table is `= &[(x, y)];` all on one line after
        # `rustfmt`, and anchoring the close to a line start made that table
        # swallow the next one. UTC has one entry, so this was the difference
        # between 14 zones and 13.
        r"^static ([A-Z0-9_]+): &\[\(i64, i32\)\] = &\[(.*?)\];",
        text,
        re.M | re.S,
    ):
        constants[match.group(1)] = [
            (int(a), int(b))
            for a, b in re.findall(r"\(\s*(-?\d+),\s*(-?\d+),?\s*\)", match.group(2))
        ]
    tables: dict[str, list[tuple[int, int]]] = {}
    # Tolerant of the line breaks `rustfmt` chooses, which are not this
    # script's to predict: the same entry is one line at one width and four at
    # another, and a pattern anchored to one of them fails on a formatter
    # upgrade with nothing actually wrong. It cost one confusing run to learn.
    for match in re.finditer(
        r'name:\s*"([^"]+)",\s*transitions:\s*([A-Z0-9_]+)\s*[,}]', text
    ):
        name, constant = match.group(1), match.group(2)
        if constant not in constants:
            raise SystemExit(
                f"{OUT.name} lists {name} as {constant}, which it does not define"
            )
        tables[name] = constants[constant]
    return tables


def check() -> int:
    """Does the committed table still agree with this system's `tzdata`?"""
    _, fresh, version = build()
    have = committed()
    stale = version != read_recorded_version()

    problems: list[str] = []
    for name in sorted(set(fresh) | set(have)):
        if name not in have:
            problems.append(f"{name}: in the generator's list and not in the file")
            continue
        if name not in fresh:
            problems.append(f"{name}: in the file and not in the generator's list")
            continue
        if have[name] == fresh[name]:
            continue
        # Name the first disagreement rather than the count: "412 differ" is
        # the same message for a rule revision and for a shifted table, and
        # the instant tells them apart at a glance.
        pairs = [
            (a, b)
            for a, b in zip(have[name], fresh[name] + [(0, 0)] * len(have[name]))
            if a != b
        ]
        where = f"first at {pairs[0][0]} in the file against {pairs[0][1]} fresh" if pairs else \
            f"the file has {len(have[name])} entries and a regeneration has {len(fresh[name])}"
        problems.append(f"{name}: {where}")

    if not problems:
        print(
            f"{OUT.name} is current: {len(fresh)} zones agree with tzdata {version}"
        )
        return 0

    print(f"{OUT.name} disagrees with this system's tzdata:", file=sys.stderr)
    for problem in problems:
        print(f"  {problem}", file=sys.stderr)
    print(file=sys.stderr)
    if stale:
        print(
            f"The file records tzdata {read_recorded_version()!r} and this system "
            f"has {version!r}, so a rule revision is the likely cause. Regenerate:",
            file=sys.stderr,
        )
    else:
        print(
            f"Both say tzdata {version!r}, so this is an edit to the generated "
            f"file rather than a tzdata change. Regenerate:",
            file=sys.stderr,
        )
    print("    python3 crates/slate-kernel/scripts/generate_zones.py", file=sys.stderr)
    return 1


def read_recorded_version() -> str:
    """The `tzdata` version the committed file says it came from."""
    match = re.search(
        r'^pub const TZDATA_VERSION: &str = "([^"]*)";$', OUT.read_text(), re.M
    )
    if match is None:
        raise SystemExit(f"{OUT.name} does not record a TZDATA_VERSION")
    return match.group(1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="compare the committed tables against this system's tzdata, and "
        "write nothing",
    )
    if parser.parse_args().check:
        return check()

    text, _, version = build()
    OUT.write_text(text)
    total = sum(len(table) for table in committed().values())
    print(f"wrote {OUT} — {len(ZONES)} zones, {total} transitions, tzdata {version}")
    # `cargo fmt` rather than formatting by hand: the arrays are long and
    # nobody should have to read them unformatted in a diff. The freshness
    # check compares pairs rather than bytes, so the formatter's output does
    # not have to be reproducible across toolchains — see the module docs.
    subprocess.run(
        ["cargo", "fmt", "-p", "slate-kernel"],
        cwd=HERE.parent.parent.parent,
        check=False,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
