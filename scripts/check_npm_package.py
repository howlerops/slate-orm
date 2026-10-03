#!/usr/bin/env python3
"""The published tarball contains the files `package.json` says it exports.

    python3 scripts/check_npm_package.py

# Why this exists

`clients/typescript/package.json` points `main`, `types` and `exports` at
`./dist/…`, and lists `dist` in `files`. `dist/` is **gitignored** — it is
build output — and nothing in the publish path built it. `release.yml`'s npm
job ran `npm ci`, then `npm test`, then `npm publish`; `npm test` compiles
with `tsconfig.json`, whose `outDir` is `dist-test`.

So on a clean checkout, which is the only kind CI has, the package was five
files:

    README.md
    package.json
    proto/google/rpc/error_details.proto
    proto/google/rpc/status.proto
    proto/slate/v1/records.proto

No JavaScript. `npm i @slate-orm/client` would install something that cannot
be imported, `main` would resolve to a path that is not in the tarball, and
**the version number could never be reused** — npm refuses a republish of a
version that has been used, so the only remedy is to burn the number and
explain.

That is the same unfixable-after-the-fact failure `scripts/check_versions.py`
was written for, one layer down: it holds the version numbers to each other
and says nothing about whether the thing wearing the number has any content.
Nothing had noticed because the job has never run — `vars.PUBLISH_NPM` is
unset, so it reports "not published" and succeeds.

`prepack` now builds `dist`, which fixes it for `npm publish` *and* for a
hand-rolled `npm pack`. This guard is the part that stays true: it asks npm
what it would actually put in the tarball and checks the declared entry
points are in the answer.

# What it checks

Every path `package.json` promises a consumer — `main`, `types`, and every
target under `exports` — appears in `npm pack --dry-run`'s file list. Not
"`dist` is in `files`", which was true the whole time it was broken.

# What it reads, and the one way it can pass when it should not

It asks npm what it would pack **from the tree as it stands**, which is the
right question in CI and a weaker one locally: a stale `dist/` left by an
earlier `npm run build` is on disk, so npm packs it, so the guard passes even
with the build step removed. Found by mutating `prepack` away and watching the
mutation survive in a tree that had been built in — the third cause
`mutate.py`'s survivor message asks you to rule out first, and here it was the
real one.

That is why the rostered mutation is on `files` instead. The exposure is
bounded: every checkout that publishes is clean, so the case this misses
locally cannot reach a registry without CI seeing it first.

# Where it runs

CI's `typescript` job and `release.yml`'s npm job, not `scripts/check.sh`:
it needs `npm` and an installed `node_modules`, which that script promises
never to require. `scripts/test_check_sh.py`'s `ELSEWHERE` says so.
"""

from __future__ import annotations

import json
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGE = ROOT / "clients" / "typescript"


def promised(manifest: dict) -> dict[str, list[str]]:
    """Every path the manifest promises a consumer, and every place it says so.

    `exports` is walked rather than indexed: it nests by condition
    (`types`, `import`, `default`, …) to arbitrary depth, and a target
    reachable only under a condition this function did not think of is
    exactly the one nobody would notice missing.

    A path maps to a **list** because several keys routinely name the same
    file — this package's `main` and `exports…default` are both
    `./dist/index.js`. Keyed to a single name, the report said whichever the
    walk reached last, so the reader was sent to one of two true places with
    no way to tell there was another. Found by this guard's own suite, which
    asserted on `main` and was answered about `types`.
    """
    found: dict[str, list[str]] = {}

    def note(path: object, where: str) -> None:
        if isinstance(path, str) and path.startswith("."):
            found.setdefault(path, []).append(where)

    for key in ("main", "types"):
        note(manifest.get(key), key)

    binaries = manifest.get("bin")
    if isinstance(binaries, str):
        note(binaries, "bin")
    elif isinstance(binaries, dict):
        for name, path in binaries.items():
            note(path, f"bin.{name}")

    def walk(node: object, where: str) -> None:
        if isinstance(node, dict):
            for name, child in node.items():
                walk(child, f"{where}.{name}")
        else:
            note(node, where)

    walk(manifest.get("exports"), "exports")
    return found


def packed(where: Path) -> list[str]:
    """What `npm pack` would put in the tarball, by asking npm."""
    result = subprocess.run(
        ["npm", "pack", "--dry-run", "--json"],
        cwd=where,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(
            f"FAIL  `npm pack --dry-run` exited {result.returncode} in {where}.\n"
            f"      It runs `prepack`, so a build failure arrives here first.\n"
            f"{result.stderr.strip()}"
        )
    # npm prints the JSON on stdout and the `prepack` output on stderr, but a
    # warning can land on stdout too, so the array is found rather than
    # assumed to start at byte zero.
    text = result.stdout[result.stdout.index("[") :]
    return [entry["path"] for entry in json.loads(text)[0]["files"]]


def main(pack: Callable[[Path], list[str]] = packed) -> int:
    """`pack` is a seam, not a convenience.

    The suite passes a written file list rather than reassigning `packed`,
    because asking npm is the one thing `scripts/check.sh` must not do — and
    because an injected argument is a declared seam where a monkeypatched
    module attribute is an undeclared one. `ty` says the same thing by
    refusing the assignment.
    """
    manifest = json.loads((PACKAGE / "package.json").read_text())
    want = promised(manifest)

    if not want:
        # The never-fires case. A manifest with no `main`, no `types` and no
        # `exports` promises nothing, so every check below passes vacuously —
        # and that is a manifest somebody should look at, not a pass.
        print(
            "FAIL  clients/typescript/package.json declares no main, types or "
            "exports, so this compared nothing.",
            file=sys.stderr,
        )
        return 1

    files = set(pack(PACKAGE))
    if not files:
        print("FAIL  `npm pack` would include no files at all.", file=sys.stderr)
        return 1

    problems = []
    for path, wheres in sorted(want.items()):
        # npm reports packed paths without the leading `./`.
        if path.removeprefix("./") not in files:
            named = ", ".join(f"`{where}`" for where in wheres)
            problems.append(
                f"{named} promises {path}, which is not in the tarball.\n"
                "      A consumer who installs this gets a package that cannot be\n"
                "      imported, and the version number cannot be reused. Check that\n"
                "      `prepack` builds it and that `files` includes its directory."
            )

    if problems:
        print(
            f"the npm package would ship without what it promises "
            f"({len(files)} files packed):\n",
            file=sys.stderr,
        )
        for problem in problems:
            print(f"  FAIL  {problem}", file=sys.stderr)
        return 1

    print(f"ok    {len(files)} files packed, and all {len(want)} declared entry points are in them")
    return 0


if __name__ == "__main__":
    sys.exit(main())
