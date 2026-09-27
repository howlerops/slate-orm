#!/usr/bin/env python3
"""`check_build_output.py`, over git repositories this file creates.

Against a written tree rather than against `slate-orm`, for the reason the
other guards here give: a check tested only by running it over this repository
asserts that today's tree is clean, which is also what a check that does
nothing asserts.

These cases need a *real* repository, not a directory — the guard asks git what
is tracked and what is ignored, and faking either would test a mock instead of
the mechanism. `git init` in a temporary directory is cheap and exercises the
same `ls-files` and `check-ignore` the real run uses, including the
exit-code-1-means-not-ignored convention that is the easiest thing here to get
backwards.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import check_build_output as guard

#: A repository where both checks pass: one package root, output ignored, and
#: an untracked `dist/` on disk to prove the guard reads git rather than the
#: filesystem.
CLEAN: dict[str, str] = {
    ".gitignore": "app/node_modules/\napp/dist/\n",
    "app/package.json": '{"name": "app"}\n',
    "app/src/main.ts": "export const x = 1;\n",
    "app/dist/main.js": "// built, and not tracked\n",
    "README.md": "a repository\n",
}


def build(root: Path, files: dict[str, str]) -> None:
    for name, body in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")
    run = lambda *args: subprocess.run(  # noqa: E731 — three calls, one shape
        ["git", "-C", str(root), *args], check=True, capture_output=True
    )
    run("init", "-q")
    run("config", "user.email", "test@example.invalid")
    run("config", "user.name", "test")
    run("add", "-A")


def case(
    name: str, changes: dict[str, str | None], failing: set[str], force: tuple[str, ...] = ()
) -> bool:
    """Build CLEAN with `changes` applied; `failing` names what must fail.

    `force` names paths to `git add -f`, which is how a case commits something
    `.gitignore` covers — the shape of the failure that actually happened here,
    where the ignore rule came *after* the commit.
    """
    files = dict(CLEAN)
    for path, body in changes.items():
        if body is None:
            files.pop(path, None)
        else:
            files[path] = body
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        build(root, files)
        for path in force:
            subprocess.run(
                ["git", "-C", str(root), "add", "-f", path], check=True, capture_output=True
            )
        report = guard.check(root)
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
        case("a clean repository passes, with untracked build output on disk", {}, set()),
        case(
            # The failure that happened: output committed before the ignore
            # rule existed. `add -f` reproduces it exactly.
            "a tracked file under dist/ is reported",
            {},
            {"no tracked file lives under a build directory"},
            force=("app/dist/main.js",),
        ),
        case(
            "a tracked file under node_modules/ is reported",
            {"app/node_modules/left-pad/index.js": "module.exports = 1;\n"},
            {"no tracked file lives under a build directory"},
            force=("app/node_modules/left-pad/index.js",),
        ),
        case(
            "a tracked file under target/ is reported",
            {"crates/x/target/debug/x": "binary-ish\n"},
            {"no tracked file lives under a build directory"},
        ),
        case(
            # The earlier failure: a new package whose output nothing ignores.
            # Nothing is committed yet, so the first check passes and this one
            # is the only thing standing between the tree and the next mistake.
            "a package root whose output is not ignored is reported",
            {"other/package.json": '{"name": "other"}\n'},
            {"every package root has its node_modules and dist ignored"},
        ),
        case(
            "a package root with node_modules ignored but not dist is reported",
            {
                "other/package.json": '{"name": "other"}\n',
                ".gitignore": "app/node_modules/\napp/dist/\nother/node_modules/\n",
            },
            {"every package root has its node_modules and dist ignored"},
        ),
        case(
            # A `package.json` vendored inside node_modules is not one of this
            # repository's packages. Reading the roots from git's tracked list
            # is what keeps thousands of them out; this case pins that.
            "a package.json inside node_modules is not treated as a package root",
            {"app/node_modules/left-pad/package.json": '{"name": "left-pad"}\n'},
            set(),
        ),
        case(
            # `path.split("/")[:-1]` drops the filename before matching, and
            # this is the only case that can tell. The first version used
            # `app/src/dist.ts`, whose last segment is `dist.ts` and not
            # `dist` — so including the filename changed nothing, the mutation
            # survived, and the case was asserting something no code path
            # could get wrong. A file named *exactly* `build`, which a shell
            # script with no extension plausibly is, is the discriminating one.
            "a file whose own name is a build directory's name is not one",
            {"app/scripts/build": "#!/bin/sh\nnpm run compile\n"},
            set(),
        ),
        case(
            "a repository with no package.json at all is reported, not passed",
            {"app/package.json": None},
            {"at least one package root"},
        ),
        case(
            "a repository tracking nothing is reported, not passed",
            {name: None for name in CLEAN},
            {"tracks some files", "at least one package root"},
        ),
    ]
    print(f"\n{sum(passed)} passed, {len(passed) - sum(passed)} failed")
    return 0 if all(passed) else 1


if __name__ == "__main__":
    sys.exit(main())
