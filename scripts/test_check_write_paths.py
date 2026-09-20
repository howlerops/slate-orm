#!/usr/bin/env python3
"""Tests for `check_write_paths.py`, over files this one writes.

Run against the real `record.rs`, a check that had stopped checking would pass
for as long as the roster happened to be right — which is the failure mode the
guard exists to prevent, one level up.

Run directly: `python3 scripts/test_check_write_paths.py`.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_write_paths

SOURCE = """\
impl RecordTransaction<'_> {
    pub async fn insert(&self, row: &Row) -> Result<()> {
        self.write_row(table, row, None).await
    }

    pub async fn insert_many(&self, rows: &[Row]) -> Result<()> {
        self.write_many(context, table, rows, BulkMode::Insert).await
    }

    pub async fn purge_deleted(&self) -> Result<u64> {
        self.erase_row(table, row)?;
        Ok(0)
    }

    pub async fn query(&self, context: &SecurityContext) -> Result<Cursor> {
        self.execute(context, table, query).await
    }

    /// A private helper that writes and is *not* one of the primitives. The
    /// `public` clause is the only thing keeping it out of the roster, so
    /// without it this fixture has four write paths and a three-name roster
    /// fails. An earlier version of this file had no such helper, and a
    /// mutation dropping that clause survived.
    async fn stamp_and_write(&self) -> Result<()> {
        self.write_row(table, row, None).await
    }

    async fn write_many(&self) -> Result<()> {
        self.write_row_with(table, row, None).await
    }

    async fn write_row(&self) -> Result<()> {
        self.write_row_with(table, row, None).await
    }
}
"""

ROSTER = 'const WRITE_PATHS: [&str; 3] = ["insert", "insert_many", "purge_deleted"];\n'

CASES = [
    (
        "a roster matching the source passes",
        SOURCE,
        ROSTER,
        0,
        "3 write paths",
    ),
    (
        "a write path missing from the roster fails and names it",
        SOURCE,
        'const WRITE_PATHS: [&str; 2] = ["insert", "insert_many"];\n',
        1,
        "`purge_deleted` can modify storage and is not in WRITE_PATHS",
    ),
    (
        "a roster naming a path that no longer writes fails too",
        # Both directions. A stale name is a probe looping over something gone,
        # which passes while covering one path fewer than it claims.
        SOURCE,
        'const WRITE_PATHS: [&str; 4] = ["insert", "insert_many", "purge_deleted", "departed"];\n',
        1,
        "WRITE_PATHS names `departed`",
    ),
    (
        "write paths with no roster anywhere fails",
        SOURCE,
        "// nothing here\n",
        1,
        "no WRITE_PATHS roster",
    ),
    (
        "a read-only method is not a write path",
        # `query` is in the source and must stay out of the roster; if the
        # criterion drifted to "takes a context", this case fails.
        SOURCE,
        ROSTER,
        0,
        "3 write paths",
    ),
    (
        "a source that reaches no primitive fails, rather than passing",
        # The never-fires case: rename `write_row` and the rule matches
        # nothing, which reads identically to "every path is rostered".
        "impl RecordTransaction<'_> {\n    pub async fn insert(&self) -> Result<()> {\n"
        "        self.persist(table, row).await\n    }\n}\n",
        ROSTER,
        1,
        "so this checked nothing",
    ),
    (
        "a private helper that writes is machinery, not a path",
        # `write_many` calls `write_row_with` and must not be rostered; the
        # roster is the *public* surface a caller can reach.
        SOURCE,
        ROSTER,
        0,
        "3 write paths",
    ),
]


def run(source: str, probe: str) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as directory:
        home = pathlib.Path(directory)
        (home / "record.rs").write_text(source)
        (home / "probe.rs").write_text(probe)
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = check_write_paths.main([str(home / "record.rs"), str(home / "probe.rs")])
        return code, out.getvalue()


def main() -> int:
    failed = 0
    for name, source, probe, expected, wanted in CASES:
        code, said = run(source, probe)
        ok = code == expected and (not wanted or wanted in said)
        failed += not ok
        print(f"{'ok  ' if ok else 'FAIL'}  {name}")
        if not ok:
            print(f"        expected exit {expected} and {wanted!r}, got {code}")
            for line in said.splitlines():
                print(f"      {line}")
    print(f"\n{len(CASES) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
