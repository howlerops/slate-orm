#!/usr/bin/env python3
"""`check_versions.py`'s own tests, over a tree this one builds.

Run against the real tree every case would be "it passed" — the versions agree
today, and a guard that had stopped comparing would look identical. So the
rules are exercised over a miniature: a root manifest, two client manifests,
and a tag.

The case worth reading is `a tag ahead of the tree`. That is the failure this
guard exists for and the one that cannot be undone: npm and PyPI refuse a
version number that has been used, so a release that publishes `0.0.1` under a
tag named `v0.2.0` cannot be corrected by publishing `0.2.0` from the same
tree — somebody has to bump, re-tag and explain the gap.

Run directly: `python3 scripts/test_check_versions.py`.
"""

from __future__ import annotations

import contextlib
import io
import json
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_versions as guard

ROOT_MANIFEST = """\
[workspace]
members = ["crates/thing"]

[workspace.package]
version = "{version}"
"""

PYPROJECT = """\
[project]
name = "slate-client"
version = "{version}"
"""


def tree(
    root: pathlib.Path,
    *,
    workspace: str = "0.1.0",
    npm: str | None = "0.1.0",
    python: str | None = "0.1.0",
) -> None:
    """A miniature with the three manifests. `None` leaves the version out."""
    (root / "Cargo.toml").write_text(ROOT_MANIFEST.format(version=workspace))

    package: dict[str, object] = {"name": "@slate-orm/client"}
    if npm is not None:
        package["version"] = npm
    where = root / "clients" / "typescript"
    where.mkdir(parents=True)
    (where / "package.json").write_text(json.dumps(package, indent=2))

    where = root / "clients" / "python"
    where.mkdir(parents=True)
    (where / "pyproject.toml").write_text(
        PYPROJECT.format(version=python) if python is not None else "[project]\nname = 'x'\n"
    )


def run(root: pathlib.Path, argv: list[str]) -> tuple[int, str]:
    guard.ROOT = root
    out, err = io.StringIO(), io.StringIO()
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = guard.main(argv)
    except Exception as raised:  # noqa: BLE001 - any raise is a failing case
        return 70, f"{out.getvalue()}{err.getvalue()}the guard raised {raised!r}"
    return code, out.getvalue() + err.getvalue()


#: name, (workspace, npm, python), argv, wanted exit, needle.
CASES: list[tuple[str, tuple[str, str | None, str | None], list[str], int, str]] = [
    ("three manifests agreeing passes", ("0.1.0", "0.1.0", "0.1.0"), [], 0, "ok    0.1.0"),
    (
        "an npm version behind the workspace fails",
        ("0.1.0", "0.0.1", "0.1.0"),
        [],
        1,
        "package.json says 0.0.1",
    ),
    (
        "a pyproject version ahead of the workspace fails",
        ("0.1.0", "0.1.0", "0.2.0"),
        [],
        1,
        "pyproject.toml says 0.2.0",
    ),
    (
        "a published package with no version at all fails",
        ("0.1.0", None, "0.1.0"),
        [],
        1,
        "declares no version",
    ),
    ("a tag matching the tree passes", ("0.1.0", "0.1.0", "0.1.0"), ["v0.1.0"], 0, "and 1 tag(s)"),
    (
        # The one that costs money. A release named v0.2.0 that publishes 0.1.0
        # cannot be taken back: the registries refuse a reused number.
        "a tag ahead of the tree fails",
        ("0.1.0", "0.1.0", "0.1.0"),
        ["v0.2.0"],
        1,
        "tag v0.2.0 carries 0.2.0",
    ),
    (
        "a tag that is not a v-tag fails",
        ("0.1.0", "0.1.0", "0.1.0"),
        ["0.1.0"],
        1,
        "is not a v-tag",
    ),
    (
        # A prerelease suffix is carried whole rather than stripped: npm and
        # PyPI spell those differently, so a release wearing one has to say so
        # in every manifest rather than in the tag alone.
        "a prerelease tag must match the tree exactly",
        ("0.1.0", "0.1.0", "0.1.0"),
        ["v0.1.0-rc.1"],
        1,
        "carries 0.1.0-rc.1",
    ),
]


def main() -> int:
    was = guard.ROOT
    failures: list[str] = []
    try:
        for name, built, argv, wanted, needle in CASES:
            guard.ROOT = was
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                workspace, npm, python = built
                try:
                    tree(root, workspace=workspace, npm=npm, python=python)
                except Exception as raised:  # noqa: BLE001 - so is a bad fixture
                    code, output = 70, f"the fixture raised {raised!r}"
                else:
                    code, output = run(root, argv)
            if code != wanted:
                failures.append(f"FAIL  {name}: exit {code}, wanted {wanted}\n{output}")
            elif needle not in output:
                failures.append(f"FAIL  {name}: no {needle!r} in\n{output}")
            else:
                print(f"ok    {name}")

        # The never-fires case, and it needs a tree with no client manifests
        # rather than a variation on the fixture above.
        guard.ROOT = was
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "Cargo.toml").write_text(ROOT_MANIFEST.format(version="0.1.0"))
            code, output = run(root, [])
        name = "a tree with no publishable manifest fails"
        if code != 1 or "no publishable package manifest" not in output:
            failures.append(f"FAIL  {name}: exit {code}\n{output}")
        else:
            print(f"ok    {name}")
    finally:
        guard.ROOT = was

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) + 1 - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
