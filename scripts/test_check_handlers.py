#!/usr/bin/env python3
"""Tests for `check_handlers.py`, over files this one writes.

Run against `service.rs`, a check that had stopped checking would pass for as
long as `service.rs` stayed correct — which is the failure mode the guard
exists to prevent, one level up. Each case builds the smallest Rust-shaped file
that exhibits one rule.

The cases that must NOT fire matter as much as the ones that must: a guard
people switch off because it cries wolf is worth less than no guard.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_handlers

#: The two exemptions the real file needs. A case whose fixture has neither
#: would fail on the stale-entry check rather than on what it is testing, so
#: every fixture below defines both.
PREAMBLE = """\
impl Head {
    fn authorized_table(&self, name: &str) -> Result<&TableDef, Status> {
        let table = self.table(name)?;
        Ok(table)
    }

    fn resolve_relation(&self, relation: &Relation) -> Result<Step, Status> {
        let child = self.table(&relation.table)?;
        Ok(child)
    }
"""

CASES: list[tuple[str, str | None, int, str]] = [
    (
        "a handler that authorises before fingerprinting passes",
        """
    async fn insert(&self) -> Result<(), Status> {
        let table = self.authorized_table(&context, &r.table, Action::Insert)?;
        fingerprint::check(table, r.schema.as_ref())?;
    }
""",
        0,
        "",
    ),
    (
        "a handler using the bare resolver fails and names itself",
        """
    async fn query(&self) -> Result<(), Status> {
        let table = self.table(&wire.table)?;
    }
""",
        1,
        "`query` resolves a table",
    ),
    (
        "a fingerprint with no authorisation above it fails",
        """
    async fn insert(&self) -> Result<(), Status> {
        let table = something_else(&r.table)?;
        fingerprint::check(table, r.schema.as_ref())?;
    }
""",
        1,
        "no `authorized_table`",
    ),
    (
        "an authorisation too far above the fingerprint does not count",
        """
    async fn insert(&self) -> Result<(), Status> {
        let table = self.authorized_table(&context, &r.table, Action::Insert)?;
        let a = 1;
        let b = 2;
        let c = 3;
        let d = 4;
        fingerprint::check(table, r.schema.as_ref())?;
    }
""",
        1,
        "no `authorized_table`",
    ),
    (
        "an exemption for a function that no longer exists fails",
        # `resolve_relation` is in UNAUTHORIZED but this fixture drops its
        # bare call, so the entry is stale.
        None,
        1,
        "no longer resolves",
    ),
]


def run(body: str | None) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as directory:
        path = pathlib.Path(directory) / "service.rs"
        if body is None:
            path.write_text(
                "impl Head {\n"
                "    fn authorized_table(&self, name: &str) -> Result<(), Status> {\n"
                "        let table = self.table(name)?;\n"
                "        Ok(())\n"
                "    }\n}\n"
            )
        else:
            path.write_text(PREAMBLE + body + "}\n")
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = check_handlers.main([str(path)])
        return code, out.getvalue()


def main() -> int:
    failed = 0
    for name, body, expected, wanted in CASES:
        code, said = run(body)
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
