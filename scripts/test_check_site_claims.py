#!/usr/bin/env python3
"""`check_site_claims.py`, over trees this file writes.

Against a written tree rather than against `slate-orm`, for the reason the
other guards here give: a check tested only by running it over this repository
can assert that today's tree is clean, which is also what a check that does
nothing asserts. Every case below makes one claim *false* and expects the guard
to say which.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_site_claims as guard

#: A tree where every claim holds, which each case then breaks in one place.
#: Written out rather than copied from `site/`, so a case says exactly what it
#: depends on and a change to the real page cannot quietly make a case vacuous.
ROSTER = {"the closest blueprint": "a claim about lineage, not about the tree"}

PAGE = """<html><body>
<p>the closest blueprint is something a test cannot settle.</p>
<ul class="chips"><li>Python · Go · TypeScript</li><li>No <code>unsafe</code></li></ul>
<p>100,000 real New York yellow-taxi trips, joined to their own 265-zone lookup.</p>
<pre>Table Scan on trips  (rows=100000 cost=13.50)</pre>
</body></html>
"""

CLEAN: dict[str, str] = {
    "site/index.html": PAGE,
    "site/data/make-trips.py": "SAMPLE = 100_000\n",
    "crates/slate-wasm/src/taxi_zones.csv": "id,name\n" + "".join(f"{i},z{i}\n" for i in range(265)),
    "crates/slate-kernel/src/lib.rs": "#![forbid(unsafe_code)]\n",
    "crates/slate-wasm/src/lib.rs": "#![forbid(unsafe_code)]\n",
    "clients/python/x": "",
    "clients/go/x": "",
    "clients/typescript/x": "",
}


def write(root: Path, files: dict[str, str]) -> None:
    for name, body in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")


def case(name: str, changes: dict[str, str | None], failing: set[str]) -> bool:
    """Run the guard over CLEAN with `changes` applied; `failing` names what must fail.

    `failing` holds *fragments* of the check's name, because the names carry
    counts that come from the tree.
    """
    files = dict(CLEAN)
    for path, body in changes.items():
        if body is None:
            files.pop(path, None)
        else:
            files[path] = body
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        write(root, files)
        report = guard.check(root, ROSTER)
        failed = [what for what, ok, _ in report if not ok]
        unexpected = [f for f in failed if not any(frag in f for frag in failing)]
        unseen = [frag for frag in failing if not any(frag in f for f in failed)]
        if unexpected or unseen:
            print(f"FAIL  {name}")
            for f in unexpected:
                print(f"        unexpected failure: {f}")
            for frag in unseen:
                print(f"        expected a failure mentioning {frag!r}, got {failed}")
            return False
    print(f"ok    {name}")
    return True


def main() -> int:
    passed = [
        case("a tree where every claim holds passes", {}, set()),
        case(
            "an unsafe block is reported",
            {"crates/slate-kernel/src/lib.rs": "#![forbid(unsafe_code)]\nfn f() { unsafe { } }\n"},
            {"unsafe block"},
        ),
        case(
            "a crate root that does not forbid unsafe is reported",
            # The defect this guard was written for: no `unsafe` anywhere, so
            # the claim is true, and nothing keeping it true. Four crates were
            # in this state when the guard was written and the page had said
            # "No `unsafe`" for nine days.
            {"crates/slate-sql/src/lib.rs": "//! no lint here\n"},
            {"forbids unsafe"},
        ),
        case(
            "a crate with no root at all is reported rather than skipped",
            {"crates/slate-ghost/src/other.rs": "fn f() {}\n"},
            {"forbids unsafe"},
        ),
        case(
            "the word unsafe in prose is not a use",
            {"crates/slate-kernel/src/lib.rs": "#![forbid(unsafe_code)]\n// no unsafe anywhere\n"},
            set(),
        ),
        case(
            "a trip count that drifted from the generator is reported",
            {"site/data/make-trips.py": "SAMPLE = 250_000\n"},
            {"trip count", "row count"},
        ),
        case(
            "a plan line that disagrees with the prose is reported",
            # The two notations of one number, which is how a reader would
            # notice: `100,000` in the sentence and `rows=99999` in the block.
            {"site/index.html": PAGE.replace("rows=100000", "rows=99999")},
            {"row count"},
        ),
        case(
            "a zone count that drifted from the CSV is reported",
            {
                "crates/slate-wasm/src/taxi_zones.csv": "id,name\n"
                + "".join(f"{i},z{i}\n" for i in range(200))
            },
            {"zone count"},
        ),
        case(
            "a client the page names and the tree does not have is reported",
            {"clients/go/x": None},
            {"named client"},
        ),
        case(
            "a claim the page has stopped making is reported",
            # `UNCHECKED` is a roster, and a roster describing sentences nobody
            # wrote any more is the failure every roster here has had.
            {"site/index.html": PAGE.replace("the closest blueprint is", "we now say")},
            {"UNCHECKED names"},
        ),
        case(
            "a missing landing page is reported rather than passing empty",
            {"site/index.html": None},
            {"landing page exists"},
        ),
    ]
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
