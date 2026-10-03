#!/usr/bin/env python3
"""Tests for `check_generated_is_used.py`, over a tree this one builds.

Run against the real repository, a rule that had stopped checking would pass
for as long as the adapters stayed correct — which is the failure mode the
guard exists to prevent, one level up. The case that matters most is the
caveat's own: a file that is still imported while the call that puts it on the
wire is gone.

Two of these cases are here because they *failed*. `a bare mention is not an
import` and `an importer in another package does not count` each held against
the guard's first draft, which grepped the module stem across the whole tree —
and `\bschema\b` is in 482 of this repository's files, so rule 1 had been
passing on the word rather than on the import. The guard was green for a
reason that had nothing to do with what it checks, which is the same offence
it was written to catch.

Run directly: `python3 scripts/test_check_generated_is_used.py`.
"""

from __future__ import annotations

import contextlib
import io
import pathlib
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_generated_is_used as guard

WORKFLOW = """\
jobs:
  clients:
    steps:
      - name: The generated declarations match the catalog
        run: |
          python3 scripts/codegen.py --check \\
            --config examples/head.toml \\
            --go pkg/gen/schema.go \\
            --web web/gen/catalog.ts
"""

#: The roster the fixture is checked against, replacing the real one. Patched
#: rather than reused: the real `REACHES` names files this miniature does not
#: have, and a test that grew a fixture file per real entry would be tracking
#: the roster rather than the rules.
ROSTER: dict[str, tuple[str, str] | str] = {
    "pkg/gen/schema.go": (r"Declaring\(", "pkg/app/main.go"),
    "web/gen/catalog.ts": "labels in a browser; nothing hashes them",
}

#: Two packages, because the scope is half of rule 1 and a one-package fixture
#: cannot tell a scoped search from an unscoped one. `go.mod` and
#: `package.json` are what `package_of` looks for.
def tree(root: pathlib.Path, *, workflow: str = WORKFLOW, declaring: bool = True) -> None:
    (root / ".github" / "workflows").mkdir(parents=True)
    (root / ".github" / "workflows" / "ci.yml").write_text(workflow)

    (root / "pkg" / "gen").mkdir(parents=True)
    (root / "pkg" / "app").mkdir()
    (root / "pkg" / "go.mod").write_text("module x\n")
    (root / "pkg" / "gen" / "schema.go").write_text("package schema\n")
    attach = "client.Declaring(declared())" if declaring else "client"
    (root / "pkg" / "app" / "main.go").write_text(
        f'import "x/gen/schema"\nfunc main() {{ {attach} }}\n'
    )

    (root / "web" / "gen").mkdir(parents=True)
    (root / "web" / "app").mkdir()
    (root / "web" / "package.json").write_text('{"name":"web"}\n')
    (root / "web" / "gen" / "catalog.ts").write_text("export const CATALOG = {};\n")
    (root / "web" / "app" / "page.ts").write_text(
        'import { CATALOG } from "../gen/catalog.js";\n'
    )

    # `importers` shells out to `git grep`, so the fixture has to be a repo.
    commit(root)


def commit(root: pathlib.Path) -> None:
    for command in (["init", "-q"], ["add", "-A"]):
        if command[0] == "init" and (root / ".git").exists():
            continue
        subprocess.run(["git", *command], cwd=root, check=True, capture_output=True)


def run(root: pathlib.Path) -> tuple[int, str]:
    guard.ROOT = root
    guard.WORKFLOW = root / ".github" / "workflows" / "ci.yml"
    guard.REACHES = dict(ROSTER)
    out, err = io.StringIO(), io.StringIO()
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = guard.main()
    except Exception as raised:  # noqa: BLE001 - any raise is a failing case
        return 70, f"{out.getvalue()}{err.getvalue()}the guard raised {raised!r}"
    return code, out.getvalue() + err.getvalue()


def clean(root: pathlib.Path) -> None:
    tree(root)


def nothing_imports_it(root: pathlib.Path) -> None:
    tree(root)
    (root / "web" / "app" / "page.ts").write_text("export const nothing = 1;\n")
    commit(root)


def a_bare_mention_is_not_an_import(root: pathlib.Path) -> None:
    """Half of what the first draft got wrong: the stem as an English word.

    `catalog.ts` is gone from the import and left in a sentence, which is how
    the real tree looks in the hundred-odd files that mention a schema without
    importing one.
    """
    tree(root)
    (root / "web" / "app" / "page.ts").write_text(
        "// These labels come from the generated catalog, eventually.\n"
    )
    commit(root)


def an_importer_in_another_package(root: pathlib.Path) -> None:
    """The other half: a real import, in a package that cannot reach it.

    Nothing in `pkg/` can import `web/gen/catalog.ts`, so a line there must not
    keep the rule green. Contrived, but the unscoped search counted exactly
    these — 482 files for `schema`, none of them the adapter.
    """
    tree(root)
    (root / "web" / "app" / "page.ts").write_text("export const nothing = 1;\n")
    (root / "pkg" / "app" / "elsewhere.ts").write_text(
        'import { CATALOG } from "../../web/gen/catalog.js";\n'
    )
    commit(root)


def a_self_naming_header(root: pathlib.Path) -> None:
    """A generated file must not count as its own importer.

    None of the five real generated files names itself today, so the
    self-exclusion in `importers` looked like dead code and the mutation
    dropping it survived. But `// Code generated from "…"; DO NOT EDIT.` is a
    codegen convention, and the quoted path in such a header matches the
    import shape exactly. The day `codegen.py` grows one, rule 1 would go
    vacuous for every file at once and nothing would say so.
    """
    tree(root)
    (root / "web" / "gen" / "catalog.ts").write_text(
        '// Code generated from "web/gen/catalog.ts"; DO NOT EDIT.\n'
        "export const CATALOG = {};\n"
    )
    (root / "web" / "app" / "page.ts").write_text("export const nothing = 1;\n")
    commit(root)


def a_package_with_no_manifest(root: pathlib.Path) -> None:
    """The fallback arm of `package_of`, which the real tree reaches.

    `examples/retention` holds no manifest, so its scope is the generated
    file's own directory. Written because the mutation returning `"."` from
    that arm survived: both other fixture packages carry a manifest, so the
    fallback was never taken and a scope of the whole repository would have
    looked identical. The decoy is what makes the two answers differ — it is
    a real import, outside the directory, that must not count.
    """
    tree(root, workflow=WORKFLOW.replace(
        "--web web/gen/catalog.ts", "--python loose/extra.py"
    ))
    (root / "loose").mkdir()
    (root / "loose" / "extra.py").write_text("EXTRA = 1\n")
    (root / "loose" / "beside.py").write_text("NOTHING = 1\n")
    (root / "pkg" / "app" / "decoy.py").write_text("from loose.extra import EXTRA\n")
    commit(root)


def imported_but_not_declared(root: pathlib.Path) -> None:
    """The caveat's own case: still imported, no longer put on the wire."""
    tree(root, declaring=False)


def a_generated_file_with_no_roster_entry(root: pathlib.Path) -> None:
    tree(root, workflow=WORKFLOW.replace("--web web/gen/catalog.ts", "--python pkg/gen/extra.py"))
    (root / "pkg" / "gen" / "extra.py").write_text("EXTRA = 1\n")
    (root / "pkg" / "app" / "use.py").write_text("from gen.extra import EXTRA\n")
    commit(root)


def a_roster_entry_for_a_file_no_longer_generated(root: pathlib.Path) -> None:
    tree(root, workflow=WORKFLOW.replace("            --web web/gen/catalog.ts\n", ""))


def a_generated_file_that_is_not_committed(root: pathlib.Path) -> None:
    tree(root)
    (root / "pkg" / "gen" / "schema.go").unlink()


def a_keyword_import_with_nothing_quoted(root: pathlib.Path) -> None:
    """The first arm of the pattern, which the fixture otherwise never reaches.

    Both importers `tree` writes quote a path, so they match the second arm and
    the first — `from .schema import`, the only shape the Python adapter in the
    real tree uses — went untested. It was written with `\\b`, a GNU extension
    macOS's regex library does not honour, and on a Mac the guard found no
    importer for any keyword import while this suite stayed green.
    """
    tree(root)
    (root / "web" / "app" / "page.ts").write_text("export const nothing = 1;\n")
    (root / "web" / "gen" / "labels.py").write_text("from .catalog import CATALOG\n")
    commit(root)


def a_longer_name_with_the_stem_in_front(root: pathlib.Path) -> None:
    """What the boundary after the stem is for: `catalog_v1` is not `catalog`."""
    tree(root)
    (root / "web" / "app" / "page.ts").write_text("export const nothing = 1;\n")
    (root / "web" / "gen" / "labels.py").write_text("from .catalog_v1 import CATALOG\n")
    commit(root)


def the_workflow_names_none(root: pathlib.Path) -> None:
    """The never-fires case: an empty roster checks nothing and passes."""
    tree(root, workflow="jobs:\n  clients:\n    steps:\n      - run: echo nothing\n")


CASES = [
    ("the real shape passes", clean, 0, "2 generated declarations, all imported"),
    ("a generated file nothing imports fails", nothing_imports_it, 1, "nothing in web imports"),
    (
        "a bare mention is not an import",
        a_bare_mention_is_not_an_import,
        1,
        "nothing in web imports",
    ),
    (
        "an importer in another package does not count",
        an_importer_in_another_package,
        1,
        "nothing in web imports",
    ),
    (
        "a self-naming header is not an importer",
        a_self_naming_header,
        1,
        "nothing in web imports",
    ),
    (
        "a package with no manifest scopes to its own directory",
        a_package_with_no_manifest,
        1,
        "nothing in loose imports",
    ),
    (
        "imported but no longer declared fails",
        imported_but_not_declared,
        1,
        "no longer matches",
    ),
    (
        "a generated file with no roster entry fails",
        a_generated_file_with_no_roster_entry,
        1,
        "REACHES has no entry for it",
    ),
    (
        "a roster entry for a file no longer generated fails",
        a_roster_entry_for_a_file_no_longer_generated,
        1,
        "no longer generates",
    ),
    (
        "a generated file that is not committed fails",
        a_generated_file_that_is_not_committed,
        1,
        "is not in the tree",
    ),
    (
        "a keyword import with nothing quoted counts",
        a_keyword_import_with_nothing_quoted,
        0,
        "2 generated declarations, all imported",
    ),
    (
        "a module whose name only starts with the stem is not an import",
        a_longer_name_with_the_stem_in_front,
        1,
        "nothing in web imports",
    ),
    ("a workflow naming no outputs fails", the_workflow_names_none, 1, "no generated files"),
]


def main() -> int:
    was = (guard.ROOT, guard.WORKFLOW, guard.REACHES)
    failures = []
    try:
        for name, build, wanted, needle in CASES:
            with tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                build(root)
                code, output = run(root)
            if code != wanted:
                failures.append(f"FAIL  {name}: exit {code}, wanted {wanted}\n{output}")
            elif needle not in output:
                failures.append(f"FAIL  {name}: no {needle!r} in\n{output}")
            else:
                print(f"ok    {name}")
    finally:
        guard.ROOT, guard.WORKFLOW, guard.REACHES = was

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
