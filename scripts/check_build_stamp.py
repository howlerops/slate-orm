#!/usr/bin/env python3
"""The build stamp is complete, and everything that measures anything prints it.

`POINT_READ_COST` was recorded as 3.0 and could not be explained for nine
tasks. #278 found the answer: the recorded run had SlateDB's block cache
compiled out — a cargo feature — and nothing in the output said so. The
numbers were right about a build nobody could identify.

`slate_kernel::build` and `slate_slatedb::stamp` fix that going forward, but
only while two things stay true, and neither is true by construction:

1. **The stamp names every feature.** `cfg!` cannot enumerate — it answers
   only for a name somebody already wrote down — so `FEATURES` is a hand-
   written list beside a `[features]` table that changes. A feature added to
   the manifest and not to the list makes the stamp wrong by omission, which
   is the exact shape of the original defect: a build difference that no
   output mentions. #267 found three lists in this repository maintained by
   hand; this is the check that stops a fourth.

2. **Every program that measures something prints it.** A benchmark that
   skips the line is a transcript that cannot be placed, and the next reader
   will place it wrongly rather than not at all — the 3.0 figure was read for
   nine tasks as a measurement of the shipping build.

WHY THE ROSTER IS A LIST OF EXEMPTIONS RATHER THAN A LIST OF BENCHMARKS

Every example, bench and binary under `crates/` must either call an announce
or appear in `NOT_A_MEASUREMENT` with a reason. A new benchmark therefore
fails this check until somebody decides which it is, and the decision is
recorded where the next person reads it. A roster of *included* programs
would silently omit anything added later, which is how
`scripts/run_examples.sh` came to have unchecked floors (#274).

Run directly: `python3 scripts/check_build_stamp.py`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "crates/slate-slatedb/Cargo.toml"
STAMP = ROOT / "crates/slate-slatedb/src/stamp.rs"
CRATES = ROOT / "crates"

#: `("aws", cfg!(feature = "aws")),` — the name is taken from the string that
#: `cfg!` is given, not from the label beside it, because those two
#: disagreeing is itself a defect this should catch.
DECLARED = re.compile(r'cfg!\(feature\s*=\s*"([^"]+)"\)')
#: `("cache", "SlateDB's block cache …")` inside LOAD_BEARING.
BEARING = re.compile(r'^\s*\(\s*\n?\s*"([^"]+)",', re.MULTILINE)
#: A `[features]` entry: `cache = ["slatedb/foyer"]`.
FEATURE = re.compile(r"^([A-Za-z][A-Za-z0-9_-]*)\s*=\s*\[", re.MULTILINE)
#: Any announce call: `announce()` on stdout for a benchmark,
#: `announce_to(&mut stderr())` for a daemon whose stdout is parsed.
ANNOUNCES = re.compile(r"\bannounce(?:_to)?\s*\(")

#: Programs that measure nothing, and why. A reason, not a name: the next
#: person has to be able to tell whether it is still true.
NOT_A_MEASUREMENT: dict[str, str] = {
    "slate-slatedb/examples/s3_server.rs": (
        "a server, not a measurement: it prints a LISTENING line and the "
        "credentials for `examples/deployed` and waits. Nothing it prints is "
        "a number anybody records."
    ),
    "slate-slatedb/examples/replicas.rs": (
        "a demonstration of replica reads. It prints what each replica saw, "
        "never a timing or a request count."
    ),
    "slate-orm/examples/multi_tenant.rs": (
        "the tour in the README, run as code. It prints rows, and its own "
        "doc comment does not even ask for --release."
    ),
}


def features(manifest: Path = MANIFEST) -> set[str]:
    """The features the manifest declares, other than `default`.

    `default` is excluded because it is an alias for the others rather than
    a thing a build either has or does not: the stamp reports what `default`
    resolved *to*, which is the question a reader of a measurement has.
    """
    # Fails soft rather than raising. A missing manifest is a moved file, and
    # the report for that is this guard saying which file it wanted — not a
    # traceback out of a CI step whose name says "build stamp". The first run
    # of the tests beside this file crashed exactly here, which is the third
    # guard this week whose *failure* path had never been executed.
    try:
        text = manifest.read_text(encoding="utf-8")
    except OSError:
        return set()
    start = text.find("[features]")
    if start < 0:
        return set()
    end = text.find("\n[", start + 1)
    block = text[start : end if end > 0 else len(text)]
    return {name for name in FEATURE.findall(block) if name != "default"}


def stamped(stamp: Path = STAMP) -> tuple[set[str], set[str]]:
    """The features the stamp reports, and the ones it calls load-bearing."""
    try:
        text = stamp.read_text(encoding="utf-8")
    except OSError:
        return set(), set()
    bearing = text[text.find("LOAD_BEARING") : text.find("pub struct Stamp")]
    return set(DECLARED.findall(text)), set(BEARING.findall(bearing))


def programs(crates: Path = CRATES) -> list[Path]:
    """Every example, bench and binary under `crates/`, in a stable order."""
    found = [
        path
        for pattern in ("*/examples/*.rs", "*/benches/*.rs", "*/src/main.rs")
        for path in crates.glob(pattern)
    ]
    return sorted(found)


def named(path: Path) -> str:
    """A path as the roster spells it: `crate/examples/thing.rs`."""
    try:
        return str(path.relative_to(CRATES))
    except ValueError:
        return str(path)


def check(
    manifest: Path = MANIFEST, stamp: Path = STAMP, crates: Path = CRATES
) -> tuple[int, list[str]]:
    """Returns how many programs were read, and everything wrong."""
    wrong = []

    declared = features(manifest)
    reported, bearing = stamped(stamp)
    if not declared:
        wrong.append(
            f"{manifest} declares no [features] this can read, so the feature "
            "half of this checked nothing. Move the pattern with them, or fix "
            "the path."
        )
    for missing in sorted(declared - reported):
        wrong.append(
            f"`{missing}` is a feature of slate-slatedb and the stamp does "
            "not report it. Add it to FEATURES in stamp.rs — a build "
            "difference the stamp omits is the defect #278 is about."
        )
    for extra in sorted(reported - declared):
        wrong.append(
            f"the stamp reports `{extra}`, which is not a feature in "
            f"{manifest.name}. `cfg!(feature = ...)` on an undeclared "
            "feature is always false, so the stamp would call it off for ever."
        )
    for invented in sorted(bearing - declared):
        wrong.append(
            f"LOAD_BEARING names `{invented}`, which is not a feature. Its "
            "warning can never fire."
        )

    seen = 0
    for path in programs(crates):
        name = named(path)
        seen += 1
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        announces = bool(ANNOUNCES.search(text))
        excused = NOT_A_MEASUREMENT.get(name)
        if announces and excused:
            wrong.append(
                f"{name} both prints a build stamp and is listed as not a "
                "measurement. One of the two is out of date."
            )
        elif not announces and not excused:
            wrong.append(
                f"{name} prints no build stamp. If it measures anything, "
                "call `slate_kernel::build::announce()` (or "
                "`slate_slatedb::announce()`, which adds the feature set) "
                "before its first number. If it does not, add it to "
                "NOT_A_MEASUREMENT with the reason."
            )
    for name in sorted(NOT_A_MEASUREMENT):
        if not (crates / name).exists():
            wrong.append(
                f"NOT_A_MEASUREMENT lists {name}, which does not exist. A "
                "stale exemption excuses the next file that takes its name."
            )
    return seen, wrong


def main(
    manifest: Path = MANIFEST, stamp: Path = STAMP, crates: Path = CRATES
) -> int:
    seen, wrong = check(manifest, stamp, crates)
    for problem in wrong:
        print(problem, file=sys.stderr)
    # The never-fires guard. Both halves walk trees, and a rename would leave
    # this printing `ok` over nothing at all — which is how a check that has
    # never fired comes to be trusted.
    if seen == 0:
        print(
            "no examples, benches or binaries were found under crates/, "
            "which means this is looking in the wrong place rather than that "
            "the repository has stopped measuring things.",
            file=sys.stderr,
        )
        return 1
    if wrong:
        print(f"\n{len(wrong)} problem(s) across {seen} programs", file=sys.stderr)
        return 1
    print(f"ok    the build stamp is complete, and {seen} programs account for it")
    return 0


if __name__ == "__main__":
    sys.exit(main())
