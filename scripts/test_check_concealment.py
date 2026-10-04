#!/usr/bin/env python3
"""Tests for `check_concealment.py`, over trees this file writes.

Run directly: `python3 scripts/test_check_concealment.py`.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_concealment as guard

ROSTER = {("crates/k/src/lib.rs", "cursor_next"): "conceals"}

CLEAN = """\
pub fn cursor_next(snapshot: &Snap) -> Row {
    decode_row(table, key, body)
}
"""


def tree(root: Path, source: str) -> None:
    (root / "crates" / "k" / "src").mkdir(parents=True)
    (root / "crates" / "k" / "src" / "lib.rs").write_text(source)


CASES = [
    ("a rostered call passes", CLEAN, ROSTER, None),
    (
        "a call in an unrostered function fails, and names it",
        CLEAN + "pub async fn leaky(s: &Snap) -> Row {\n    read_row_unchecked(s, table, key).await\n}\n",
        ROSTER,
        "`leaky` calls `read_row_unchecked`",
    ),
    (
        "every primitive is a primitive",
        CLEAN + "fn leaky() {\n    row_from_index_entry(t, i, e, k, c);\n}\n",
        ROSTER,
        "`leaky` calls `row_from_index_entry`",
    ),
    (
        "a stale roster entry fails",
        CLEAN,
        {**ROSTER, ("crates/k/src/lib.rs", "gone"): "was here"},
        "ROSTER names `gone`",
    ),
    (
        "a primitive's own definition is not a call",
        CLEAN + "pub fn decode_row(table: &T) -> Row {\n    todo()\n}\n",
        ROSTER,
        None,
    ),
    (
        "a mention in a comment is not a call",
        CLEAN + "fn quiet() {\n    // decode_row(table, key, body) would be wrong here\n}\n",
        ROSTER,
        None,
    ),
    (
        "finding no call at all fails rather than passes",
        "pub fn nothing() {}\n",
        {},
        "checked nothing",
    ),
]


def main() -> int:
    failures = []
    for name, source, roster, wanted in CASES:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tree(root, source)
            found = guard.problems(root, roster)
        text = "\n".join(found)
        if wanted is None and found:
            failures.append(f"FAIL  {name}: expected no problem, got\n{text}")
        elif wanted is not None and wanted not in text:
            failures.append(f"FAIL  {name}: no {wanted!r} in\n{text or '(nothing)'}")
        else:
            print(f"ok    {name}")

    real = guard.problems()
    if real:
        failures.append("FAIL  the real tree passes:\n" + "\n".join(real))
    else:
        print("ok    the real tree passes")

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) + 1 - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
