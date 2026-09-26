#!/usr/bin/env python3
"""`check_closed_caveats.py`, over git repositories this file builds.

Against a written tree rather than against `slate-orm`, for the reason the
other guards here give: a check tested only by running it over this repository
asserts that today's tree is clean, which is also what a check that does
nothing asserts.

`present()` shells out to `git grep`, so each case is a real `git init` with a
real commit — the same shape `test_check_build_output.py` uses. Faking it with
a stub would test the roster and not the lookup, and the lookup is where a
pathspec typo lives.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_closed_caveats as guard


def repo(files: dict[str, str], verdicts: list[dict]) -> Path:
    root = Path(tempfile.mkdtemp())
    for name, body in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")
    (root / "docs").mkdir(parents=True, exist_ok=True)
    (root / "docs" / "caveat-status.json").write_text(
        json.dumps({"verdicts": verdicts}, indent=2), encoding="utf-8"
    )
    for argv in (
        ["git", "init", "-q"],
        ["git", "add", "-A"],
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "t"],
    ):
        subprocess.run(argv, cwd=root, check=True, capture_output=True)
    return root


def case(
    name: str,
    files: dict[str, str],
    verdicts: list[dict],
    witness: dict,
    witnessed: dict,
    exempt: dict,
    failing: set[str],
) -> bool:
    root = repo(files, verdicts)
    saved = (guard.WITNESS, guard.WITNESSED, guard.EXEMPT)
    guard.WITNESS, guard.WITNESSED, guard.EXEMPT = witness, witnessed, exempt
    try:
        report = guard.check(root)
    finally:
        guard.WITNESS, guard.WITNESSED, guard.EXEMPT = saved
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


def malformed(name: str) -> bool:
    """A status file that will not parse must fail, not read as zero closures.

    Found by a surviving mutation: replacing `closed()`'s `JSONDecodeError`
    arm's `[]` with a fabricated row changed nothing, because no case wrote an
    unparseable file. The behaviour was already right — the never-fires guard
    catches the empty list — and nothing held it there, which is the
    missing-test cause `scripts/mutate.py`'s survivor message names.
    """
    root = repo(FILES, [])
    (root / "docs" / "caveat-status.json").write_text("{not json", encoding="utf-8")
    saved = (guard.WITNESS, guard.WITNESSED, guard.EXEMPT)
    # A roster that *would* pass if the file had parsed, so the only thing
    # that can fail is the unparseable file itself.
    guard.WITNESS, guard.WITNESSED, guard.EXEMPT = W, R, E
    try:
        failed = [what for what, ok, _ in guard.check(root) if not ok]
    finally:
        guard.WITNESS, guard.WITNESSED, guard.EXEMPT = saved
    if not any("recorded closed at all" in f for f in failed):
        print(f"FAIL  {name}\n        got {failed}")
        return False
    print(f"ok    {name}")
    return True


V = [{"entry": "a.md", "key": "a claim", "verdict": "closed", "by": "x"}]
W: dict[str, tuple[str | None, str]] = {"feature": ("HAVING", "src/sql.rs")}
R = {("a.md", "a claim"): "feature"}
E = {"measured": "closed by a measurement, which leaves no artifact"}
FILES = {"src/sql.rs": "// HAVING is parsed here\n"}


def main() -> int:
    passed = [
        case("a tree whose witness is present passes", FILES, V, W, R, E, set()),
        case(
            "a witness whose string has gone is reported",
            # The whole point: the closure regressed and the verdict did not.
            {"src/sql.rs": "// nothing here any more\n"},
            V, W, R, E,
            {"every witness is still in the tree"},
        ),
        case(
            "a witness whose file has gone is reported",
            {"src/other.rs": "// HAVING\n"},
            V, W, R, E,
            {"every witness is still in the tree"},
        ),
        case(
            "the needle must be in the named file, not merely somewhere",
            # A pathspec typo would otherwise pass on any file in the tree.
            {"src/sql.rs": "// nothing\n", "src/elsewhere.rs": "// HAVING\n"},
            V, W, R, E,
            {"every witness is still in the tree"},
        ),
        case(
            "a file witness needs only the file",
            FILES, V,
            {"feature": (None, "src/sql.rs")}, R, E,
            set(),
        ),
        case(
            "a file witness fails when the file is gone",
            {"src/other.rs": "x\n"}, V,
            {"feature": (None, "src/sql.rs")}, R, E,
            {"every witness is still in the tree"},
        ),
        case(
            "a closed verdict in neither roster is reported",
            FILES,
            [*V, {"entry": "b.md", "key": "another", "verdict": "closed", "by": "y"}],
            W, R, E,
            {"names a witness or an exemption"},
        ),
        case(
            "an open verdict needs no witness",
            FILES,
            [*V, {"entry": "b.md", "key": "another", "verdict": "open"}],
            W, R, E,
            set(),
        ),
        case(
            "an exempt closure passes with no witness",
            FILES, V, W, {("a.md", "a claim"): "=measured"}, E, set(),
        ),
        case(
            "an exemption EXEMPT does not explain is reported",
            # Otherwise `=whatever` is a silent escape hatch.
            FILES, V, W, {("a.md", "a claim"): "=invented"}, E,
            {"every exemption used is one EXEMPT explains"},
        ),
        case(
            "a witness name WITNESS does not define is reported",
            FILES, V, W, {("a.md", "a claim"): "ghost"}, E,
            {"every witness named is one WITNESS defines"},
        ),
        case(
            "a witness row whose verdict is no longer closed is reported",
            # The orphan case: the caveat was reworded or re-verdicted and the
            # row here kept a witness chosen for a different claim.
            FILES,
            [{"entry": "a.md", "key": "a claim", "verdict": "narrowed", "by": "x"}],
            W, R, E,
            {"no witness row outlives", "some verdict is recorded closed at all"},
        ),
        case(
            "a file with no closed verdict at all is reported, not passed",
            # The never-fires guard. A renamed field finds nothing and reads
            # exactly like a repository whose every closure is witnessed.
            FILES, [], W, {}, E,
            {"some verdict is recorded closed at all"},
        ),
        case(
            "a missing status file is reported rather than passing empty",
            FILES, [], W, {}, E,
            {"some verdict is recorded closed at all"},
        ),
        malformed(
            "a status file that is not JSON is reported rather than read as no closures"
        ),
    ]

    # Two properties of the real roster, which the cases above cannot see
    # because they replace it: it must cover this repository's own verdicts,
    # and it must not have grown an entry nothing uses.
    real = guard.check(guard.ROOT)
    for what, ok, detail in real:
        print(f"{'ok   ' if ok else 'FAIL '} the real roster: {what}")
        passed.append(ok)
        if not ok:
            print(f"        {detail}")

    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
