#!/usr/bin/env python3
"""Tests for `check_build_stamp.py`, over trees this one writes.

Run against the real workspace — where the stamp is complete and every
program accounts for it — a mutation to the pattern, the set arithmetic or
the exemption survives, because a correct tree distinguishes none of them.
That is the finding the guards written this week keep producing, so this one
starts with fixtures and keeps the real tree as the last case.

Run directly: `python3 scripts/test_check_build_stamp.py`.
"""

from __future__ import annotations

import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_build_stamp as guard

#: A manifest shaped like the real one: `default` is an alias for the rest.
MANIFEST = """[package]
name = "slate-slatedb"

[features]
dhat-heap = []
default = ["aws", "cache"]
aws = ["slatedb/aws"]
cache = ["slatedb/foyer"]

[dev-dependencies]
flate2 = "1"
"""

#: A stamp shaped like the real one, cut down to what the patterns read.
STAMP = """pub const FEATURES: &[(&str, bool)] = &[
    ("aws", cfg!(feature = "aws")),
    ("cache", cfg!(feature = "cache")),
    ("dhat-heap", cfg!(feature = "dhat-heap")),
];

pub const LOAD_BEARING: &[(&str, &str)] = &[
    (
        "cache",
        "SlateDB's block cache is compiled out",
    ),
];

pub struct Stamp {
    pub version: &'static str,
}
"""

#: A program that measures something and says what build it is.
ANNOUNCING = "fn main() {\n    slate_kernel::build::announce();\n}\n"
#: A program that measures something and does not.
SILENT = "fn main() {\n    println!(\"42 ms\");\n}\n"


def run(
    manifest: str = MANIFEST,
    stamp: str = STAMP,
    programs: dict[str, str] | None = None,
    excused: dict[str, str] | None = None,
) -> tuple[int, list[str]]:
    """Run the real guard over a tree this writes."""
    if programs is None:
        programs = {"a/examples/bench.rs": ANNOUNCING}
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        manifest_path = root / "Cargo.toml"
        manifest_path.write_text(manifest)
        stamp_path = root / "stamp.rs"
        stamp_path.write_text(stamp)
        crates = root / "crates"
        for name, text in programs.items():
            path = crates / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        crates.mkdir(exist_ok=True)
        before = guard.NOT_A_MEASUREMENT.copy()
        was = guard.CRATES
        guard.CRATES = crates
        # Always replaced, never inherited. Leaving the real exemptions in
        # place made five fixture cases report the real repository's files as
        # missing from a temporary directory — a test harness failing in a way
        # that reads as a guard defect, which is the most expensive kind.
        guard.NOT_A_MEASUREMENT.clear()
        guard.NOT_A_MEASUREMENT.update(excused or {})
        try:
            return guard.check(manifest_path, stamp_path, crates)
        finally:
            guard.CRATES = was
            guard.NOT_A_MEASUREMENT.clear()
            guard.NOT_A_MEASUREMENT.update(before)


#: name, kwargs, how many programs, how many problems, a fragment of the first.
CASES: list[tuple[str, dict, int, int, str]] = [
    ("a complete stamp over an announcing program is accepted", {}, 1, 0, ""),
    # The defect this exists for. A feature added to the manifest and not to
    # the stamp makes every measurement from a build that sets it silently
    # incomparable with one that does not — which is #278, exactly.
    (
        "a feature the stamp does not report is a problem",
        {"manifest": MANIFEST.replace("cache = [", "compression = [\"x\"]\ncache = [")},
        1,
        1,
        "`compression` is a feature",
    ),
    # The other direction: a feature removed from the manifest but left in
    # the stamp. `cfg!` on an undeclared feature is always false, so the
    # stamp would report it off for ever and look like information.
    (
        "a feature the manifest no longer declares is a problem",
        {"manifest": MANIFEST.replace("dhat-heap = []\n", "")},
        1,
        1,
        "the stamp reports `dhat-heap`",
    ),
    # `default` is an alias, not a build fact. Reporting it would be noise,
    # and demanding it in FEATURES would be a false failure.
    (
        "default is not treated as a feature",
        {"manifest": MANIFEST.replace("default = [\"aws\", \"cache\"]", "default = []")},
        1,
        0,
        "",
    ),
    (
        "a load-bearing name that is not a feature is a problem",
        {"stamp": STAMP.replace('"cache",\n        "SlateDB', '"chache",\n        "SlateDB')},
        1,
        1,
        "LOAD_BEARING names `chache`",
    ),
    # The roster half.
    (
        "a program with no stamp and no exemption is a problem",
        {"programs": {"a/examples/quiet.rs": SILENT}, "excused": {}},
        1,
        1,
        "prints no build stamp",
    ),
    (
        "an exempted program is accepted",
        {
            "programs": {"a/examples/quiet.rs": SILENT},
            "excused": {"a/examples/quiet.rs": "a demo, it prints rows"},
        },
        1,
        0,
        "",
    ),
    # Both at once means somebody wired a stamp into a program and left the
    # exemption behind. Harmless today, and the reason the next reader
    # believes a stale list.
    (
        "a program that is both stamped and exempted is a problem",
        {
            "programs": {"a/examples/bench.rs": ANNOUNCING},
            "excused": {"a/examples/bench.rs": "a demo, it prints rows"},
        },
        1,
        1,
        "both prints a build stamp and is listed",
    ),
    (
        "an exemption for a file that is gone is a problem",
        {
            "programs": {"a/examples/bench.rs": ANNOUNCING},
            "excused": {"a/examples/ghost.rs": "deleted last month"},
        },
        1,
        1,
        "which does not exist",
    ),
    # `announce_to` is how a daemon writes the stamp to stderr, because its
    # stdout is parsed. A guard that only knew `announce()` would demand the
    # wrong call on exactly the binary the head-node tables measure.
    (
        "a daemon writing the stamp to stderr counts",
        {
            "programs": {
                "a/src/main.rs": "fn main() {\n"
                "    slate_slatedb::announce_to(&mut std::io::stderr());\n}\n"
            },
            "excused": {},
        },
        1,
        0,
        "",
    ),
    (
        "benches are read, not only examples",
        {"programs": {"a/benches/codec.rs": SILENT}, "excused": {}},
        1,
        1,
        "prints no build stamp",
    ),
]


def main() -> int:
    passed = failed = 0
    for name, kwargs, programs, problems, fragment in CASES:
        seen, wrong = run(**kwargs)
        ok = seen == programs and len(wrong) == problems
        if ok and fragment:
            ok = fragment in wrong[0]
        if ok:
            print(f"ok    {name}")
            passed += 1
        else:
            print(f"FAIL  {name}")
            print(f"        wanted {programs} program(s) and {problems} problem(s)")
            print(f"        got    {seen} and {wrong}")
            failed += 1

    # An empty tree must fail rather than report `ok` over nothing — the
    # never-fires guard, which is the failure this repository has met most.
    seen, _ = run(programs={})
    empty = guard.main(
        pathlib.Path("/nonexistent/Cargo.toml"),
        pathlib.Path("/nonexistent/stamp.rs"),
        pathlib.Path("/nonexistent/crates"),
    )
    if seen == 0 and empty == 1:
        print("ok    a tree with no programs fails rather than passing")
        passed += 1
    else:
        print(f"FAIL  a tree with no programs fails rather than passing ({empty})")
        failed += 1

    # The real workspace, last: every program accounts for the stamp.
    if guard.main() == 0:
        print("ok    the real workspace is completely stamped")
        passed += 1
    else:
        print("FAIL  the real workspace is completely stamped")
        failed += 1

    print(f"\n{passed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
