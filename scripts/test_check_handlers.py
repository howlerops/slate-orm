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

#: The three exemptions the real sources need. A case whose fixture omits one
#: fails on the stale-entry check rather than on what it is testing, so every
#: fixture below defines all three.
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

    fn query_from_proto_at(query: &Query, table: &TableDef) -> Result<(), Status> {
        fingerprint::check(table, query.schema.as_ref())?;
        Ok(())
    }
"""

CASES: list[tuple[str, str | dict[str, str] | None, int, str]] = [
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
        "a fingerprint whose table arrives already authorised passes",
        # `query_from_proto_at` is in PREAMBLE and fingerprints with no
        # `authorized_table` above it. It must pass on its exemption, or the
        # exemption list does nothing.
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
        "a file with no fingerprint check at all fails, rather than passing",
        # The check-that-never-fires failure: handlers move, the guard reads a
        # tree with nothing in it, and green means "found nothing wrong" and
        # "stopped looking" at the same time.
        "NO_PREAMBLE",
        1,
        "no `fingerprint::check` anywhere",
    ),
    (
        "every file in the directory is read, not only the first",
        {
            "a_first.rs": PREAMBLE + "}\n",
            "z_last.rs": """
impl Head {
    async fn query(&self) -> Result<(), Status> {
        let table = self.table(&wire.table)?;
    }
}
""",
        },
        1,
        "`query` resolves a table",
    ),
    (
        "a stale FINGERPRINT_BY_CALLER entry is reported too",
        # `resolve_relation` is here so UNAUTHORIZED stays current; the
        # by-caller exemption is the only stale one, so the finding can only
        # come from the second list.
        {
            "only.rs": """
impl Head {
    fn authorized_table(&self, name: &str) -> Result<&TableDef, Status> {
        let table = self.table(name)?;
        Ok(table)
    }

    fn resolve_relation(&self, relation: &Relation) -> Result<Step, Status> {
        let child = self.table(&relation.table)?;
        Ok(child)
    }

    async fn insert(&self) -> Result<(), Status> {
        let table = self.authorized_table(&context, &r.table, Action::Insert)?;
        fingerprint::check(table, r.schema.as_ref())?;
    }
}
""",
        },
        1,
        "FINGERPRINT_BY_CALLER lists `query_from_proto_at`",
    ),
    (
        "an exemption for a function that no longer exists fails",
        # `resolve_relation` is in UNAUTHORIZED but this fixture drops its
        # bare call, so the entry is stale.
        None,
        1,
        "UNAUTHORIZED lists `resolve_relation`",
    ),
]


def run(body: str | dict[str, str] | None) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as directory:
        path = pathlib.Path(directory) / "service.rs"
        if isinstance(body, dict):
            # Several files, handed over as a *directory*. Every other case
            # passes one file, which never exercises the walk — and the walk is
            # the whole point of scanning a tree rather than the one file the
            # first version read.
            for name, text in body.items():
                (pathlib.Path(directory) / name).write_text(text)
            out = io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
                code = check_handlers.main([directory])
            return code, out.getvalue()
        if body == "NO_PREAMBLE":
            # Resolutions and exemptions present, no fingerprint anywhere.
            path.write_text(
                PREAMBLE.replace(
                    "        fingerprint::check(table, query.schema.as_ref())?;\n", ""
                )
                + "}\n"
            )
        elif body is None:
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
