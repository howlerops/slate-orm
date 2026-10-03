#!/usr/bin/env python3
"""`check_npm_package.py`'s own tests, over manifests this file writes.

`packed()` shells out to `npm`, which `scripts/check.sh` promises never to
need, so `main()` takes it as an argument and the rules are exercised against
a written file list. An injected seam rather than a reassigned module
attribute: `ty` refuses the assignment, and it is right to — the declared type
of `guard.packed` is that one function, not any function shaped like it. What that cannot prove is that the guard reads npm correctly; the
real-tree demonstration in
`ledger/2026-10-03-the-package-that-would-have-shipped-empty.md` is the other
half, and it is the half that found the defect.

Run directly: `python3 scripts/test_check_npm_package.py`.
"""

from __future__ import annotations

import contextlib
import io
import json
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import check_npm_package as guard

#: What the package really declares today, as the starting point every case
#: varies. Keeping the shape honest matters: a fixture that promises one file
#: would pass rules that the real two-entry `exports` block breaks.
REAL = {
    "name": "@slate-orm/client",
    "main": "./dist/index.js",
    "types": "./dist/index.d.ts",
    "exports": {".": {"types": "./dist/index.d.ts", "default": "./dist/index.js"}},
    "files": ["dist", "proto"],
}

#: What `npm pack` listed on a clean checkout before `prepack` existed. Not
#: invented — this is the real output, and it is why this guard was written.
EMPTY_PACK = [
    "README.md",
    "package.json",
    "proto/google/rpc/error_details.proto",
    "proto/google/rpc/status.proto",
    "proto/slate/v1/records.proto",
]

FULL_PACK = [*EMPTY_PACK, "dist/index.js", "dist/index.d.ts", "dist/client.js"]


def run(manifest: dict, files: list[str]) -> tuple[int, str]:
    out, err = io.StringIO(), io.StringIO()
    with tempfile.TemporaryDirectory() as directory:
        where = pathlib.Path(directory)
        (where / "package.json").write_text(json.dumps(manifest))
        was_package = guard.PACKAGE
        guard.PACKAGE = where
        try:
            with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
                code = guard.main(pack=lambda _: files)
        except SystemExit as raised:  # `packed` raises one; the seam should not
            code, _ = 70, out.write(f"the guard exited: {raised}")
        finally:
            guard.PACKAGE = was_package
    return code, out.getvalue() + err.getvalue()


def without(key: str) -> dict:
    manifest = json.loads(json.dumps(REAL))
    del manifest[key]
    return manifest


#: name, manifest, packed files, wanted exit, needle.
CASES: list[tuple[str, dict, list[str], int, str]] = [
    ("a tarball with every entry point passes", REAL, FULL_PACK, 0, "all 2 declared entry points"),
    (
        # The defect, exactly as it stood: `files` names `dist`, `dist` is
        # gitignored, nothing built it, and npm packs five files happily.
        "the real pre-fix pack fails",
        REAL,
        EMPTY_PACK,
        1,
        "promises ./dist/index.js, which is not in the tarball",
    ),
    (
        # Two keys naming one file report as one line naming both. The first
        # version keyed by path alone and printed whichever the walk reached
        # last, so this case asserted on `main` and was answered about
        # `types` — a reader sent to one of two true places with no sign
        # there was another.
        "two keys promising one missing file name both",
        REAL,
        EMPTY_PACK,
        1,
        "`main`, `exports...default` promises ./dist/index.js",
    ),
    (
        # The walk, not an index: a target reachable only under a condition
        # nobody enumerated is the one that goes unnoticed.
        "a target nested under an unusual condition is still checked",
        {"exports": {".": {"node": {"import": "./dist/deep.js"}}}},
        FULL_PACK,
        1,
        "./dist/deep.js",
    ),
    (
        "a `bin` map is checked too",
        {"bin": {"slate": "./dist/cli.js"}},
        FULL_PACK,
        1,
        "`bin.slate` promises ./dist/cli.js",
    ),
    (
        # A bare `exports: "./x.js"` is legal and must not be skipped for
        # not being a dict.
        "a string `exports` is checked",
        {"exports": "./dist/only.js"},
        FULL_PACK,
        1,
        "./dist/only.js",
    ),
    (
        # The never-fires case. Promising nothing passes every rule, and is
        # not a package anybody meant to publish.
        "a manifest promising nothing fails rather than passing on nothing",
        {"name": "@slate-orm/client", "files": ["dist"]},
        FULL_PACK,
        1,
        "declares no main, types or exports",
    ),
    (
        "an empty tarball fails even when something is promised",
        REAL,
        [],
        1,
        "no files at all",
    ),
]


def main() -> int:
    failures: list[str] = []
    for name, manifest, files, wanted, needle in CASES:
        code, output = run(manifest, files)
        if code != wanted:
            failures.append(f"FAIL  {name}: exit {code}, wanted {wanted}\n{output}")
        elif needle not in output:
            failures.append(f"FAIL  {name}: no {needle!r} in\n{output}")
        else:
            print(f"ok    {name}")

    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"{len(CASES) - len(failures)} passed, {len(failures)} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
